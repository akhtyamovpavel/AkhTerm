# A process-owned transcript takes precedence. A shared working directory
# cannot identify the agent when another process of the same kind uses it.
resolve_pane_transcript() {
  kind=$1
  dir=$2
  agent_pid=$3
  case "$kind" in
    opencode) opencode_fd "$agent_pid"; return ;;
    codex) owned=$(codex_fd "$agent_pid") ;;
    agy) owned=$(agy_fd "$agent_pid" "$HOME/.gemini/antigravity-cli/brain") ;;
    claude) owned=$(claude_fd "$agent_pid") ;;
    *) return 1 ;;
  esac
  if [ -n "$owned" ]; then printf '%s' "$owned"; return; fi
  [ -n "$dir" ] && [ -d "$dir" ] || return 1
  own_tree=$(tree_pids "$agent_pid")
  for other in "$P_ROOT"/[0-9]*; do
    [ "$other/cwd" -ef "$dir" ] || continue
    case " $own_tree " in *" ${other##*/} "*) continue ;; esac
    [ "$(proc_kind "${other##*/}")" = "$kind" ] && return 1
  done
  resolve_for_kind "$kind" "$dir" "$agent_pid"
}

claude_fd() {
  for pid in $(tree_pids "$1"); do
    for fd in "$P_ROOT/$pid/fd/"*; do
      path=$(readlink "$fd" 2>/dev/null) || continue
      case "$path" in
        */subagents/*) continue ;;
        "$HOME"/.claude/projects/*/*.jsonl) [ -f "$path" ] && { printf '%s' "$path"; return; } ;;
      esac
    done
  done
  return 1
}
pane_rank() {
  _tw="$1"; _wa="$2"; _pa="$3"; _wi="$4"; _wn="$5"; _twi="${_tw%%:*}"; _r=-1
  case "$_tw" in
    "") ;;
    *:*) if [ -n "$_wi" ] && [ "$_wi:$_wn" = "$_tw" ]; then _r=4; [ "$_pa" = "1" ] && _r=5
         elif [ -n "$_wi" ] && [ "$_wi" = "$_twi" ]; then _r=2; [ "$_pa" = "1" ] && _r=3
         elif [ "$_wn" = "$_tw" ]; then _r=4; [ "$_pa" = "1" ] && _r=5; fi ;;
    *)   if [ "$_wn" = "$_tw" ] || [ "$_wi" = "$_tw" ]; then _r=4; [ "$_pa" = "1" ] && _r=5; fi ;;
  esac
  [ "$_r" -lt 0 ] && { _r=0; [ "$_pa" = "1" ] && _r=1; [ "$_wa" = "1" ] && [ "$_pa" = "1" ] && _r=2; }
  printf '%s' "$_r"
}
pick_pane() {
  _ts="$1"; _tw="$2"; _bp=""; _bc=""; _bd=""; _br=-1
  while IFS='|' read -r _sn _wa _pa _ppid _pcmd _ppath _wi _wn _pi; do
    [ "$_sn" = "$_ts" ] || continue
    case "$_tw" in
      %*) [ "$_tw" = "$_pi" ] || continue ;;
      "") ;;
      *) if [ -n "$_wi" ] && [ "$_tw" != "$_wn" ] && [ "${_tw%%:*}" != "$_wi" ]; then continue; fi ;;
    esac
    _r=$(pane_rank "$_tw" "$_wa" "$_pa" "$_wi" "$_wn")
    [ "$_r" -gt "$_br" ] && { _br="$_r"; _bp="$_ppid"; _bc="$_pcmd"; _bd="$_ppath"; }
  done
  [ "$_br" -ge 0 ] && printf '%s|%s|%s' "$_bp" "$_bc" "$_bd"
}
tmux_hint() {
  _hl=$(printf '%s %s' "$1" "$2" | tr 'A-Z' 'a-z')
  _hk=$(detect_kind "$1" "$_hl" || true)
  [ -n "$_hk" ] || return 1
  _hf=$(resolve_pane_transcript "$_hk" "$3" "$4" || true)
  [ -n "$_hf" ] && printf '%s|%s' "$_hk" "$_hf"
}
probe_tmux() {
  _s="$1"; _ts="$2"; _tw="$3"; _ch="$4"; _ah="$5"; _pp="$6"; _pk="$7"; _po="$8"
  _fk=""; _ff=""; _rw=$(printf '%s\n' "${TMUX_PANES:-}" | pick_pane "$_ts" "$_tw")
  if [ -n "$_rw" ]; then
    _ppid="${_rw%%|*}"; _rs="${_rw#*|}"; _pcmd="${_rs%%|*}"; _ppath="${_rs#*|}"
    _groups=$(process_groups "$_ppid")
    _ag=$(find_tree_agent "$_ppid" "${_groups#*|}")
    if [ -n "$_ag" ]; then
      _ap="${_ag%%|*}"; _fk="${_ag#*|}"
      _ac=$(readlink "$P_ROOT/$_ap/cwd" 2>/dev/null); [ -n "$_ac" ] || _ac="${_ppath:-$_ch}"
      _ff=$(resolve_pane_transcript "$_fk" "$_ac" "$_ap" || true)
    fi
    if [ -z "$_ag" ] && [ -n "$_groups" ] && [ "${_groups%%|*}" = "${_groups#*|}" ]; then
      case "${_pcmd#-}" in
        bash|zsh|sh|fish|dash|ksh|tcsh|csh|nu)
          printf 'AKHTERM_RTELEM1|%s|SHELL|-|0|0|0|0|\n\nAKHTERM_RTELEM_END\n' "$_s"
          return ;;
      esac
    fi
    if [ -z "$_ag" ]; then
      _hm=$(tmux_hint "$_pcmd" "$_ah" "${_ppath:-$_ch}" "$_ppid" || true)
      [ -n "$_hm" ] && { _fk="${_hm%%|*}"; _ff="${_hm#*|}"; }
    fi
  fi
  emit_slice "$_s" "$_fk" "$_ff" "$_pp" "$_po"
}
tmux_roots() {
  [ -n "${TMUX_PANES:-}" ] || return 0
  printf '%s\n' "$TMUX_PANES" | while IFS='|' read -r _s1 _s2 _s3 _pp _r; do
    [ -n "$_pp" ] && printf '%s ' "$_pp"
  done
}
