# Core POSIX shell helpers for remote agent discovery and transcript slicing.
LC_ALL=C; export LC_ALL
P_ROOT="${AKHTERM_TEST_PROC:-/proc}"

detect_kind() {
  case "$1" in
    ssh|sshd) return 1 ;;
    agy) printf 'agy'; return 0 ;;
    opencode) printf 'opencode'; return 0 ;;
    codex|codex-*) printf 'codex'; return 0 ;;
    claude|claude-*) printf 'claude'; return 0 ;;
  esac
  case "$2" in
    *akhterm_rtelem*) return 1 ;;
    */agy|*/agy\ *|agy\ *|*antigravity*) printf 'agy'; return 0 ;;
    *codex*) printf 'codex'; return 0 ;;
    *claude*) printf 'claude'; return 0 ;;
  esac
  return 1
}

proc_kind() {
  _p="$1"; [ "$_p" = "$$" ] && return 1; [ "$_p" = "${PPID:-0}" ] && return 1
  [ -r "$P_ROOT/$_p/cmdline" ] || [ -r "$P_ROOT/$_p/comm" ] || return 1
  _raw=$(tr '\000\n\t' '   ' < "$P_ROOT/$_p/cmdline" 2>/dev/null)
  _cm=$(tr -d '\n\r' < "$P_ROOT/$_p/comm" 2>/dev/null)
  [ -z "$_raw" ] && _raw="$_cm"
  _low=$(printf '%s' "$_raw" | tr 'A-Z' 'a-z')
  _f1="${_low%% *}"; _ex="${_f1##*/}"
  [ -z "$_ex" ] && _ex=$(printf '%s' "$_cm" | tr 'A-Z' 'a-z')
  detect_kind "$_ex" "$_low"
}

tree_pids() {
  _q="$1"; _sn=" "; _n=0; _ps=""
  if [ "$P_ROOT" = "/proc" ] && [ -n "$_q" ]; then
    _f1="${_q%% *}"; set -- "$P_ROOT/$_f1/task"/*/children
    [ -r "$1" ] || [ -r "$P_ROOT/$_f1/children" ] || _ps=$(ps -eo pid=,ppid= 2>/dev/null || true)
  fi
  while [ -n "$_q" ] && [ "$_n" -lt 64 ]; do
    _c="${_q%% *}"
    if [ "$_c" = "$_q" ]; then _q=""; else _q="${_q#* }"; fi
    case "$_c" in ''|*[!0-9]*) continue ;; esac
    case "$_sn" in *" $_c "*) continue ;; esac
    _sn="$_sn$_c "; _n=$((_n + 1))
    for _cf in "$P_ROOT/$_c/task"/*/children "$P_ROOT/$_c/children"; do
      [ -r "$_cf" ] || continue
      for _kp in $(tr '\n\t' '  ' < "$_cf" 2>/dev/null); do _q="${_q:+$_q }$_kp"; done
    done
    if [ -n "$_ps" ]; then
      for _kp in $(printf '%s\n' "$_ps" | awk -v p="$_c" '$2==p{print $1}'); do _q="${_q:+$_q }$_kp"; done
    fi
  done
  printf '%s' "$_sn"
}

process_groups() {
  _stat=$(cat "$P_ROOT/$1/stat" 2>/dev/null) || return 1
  set -- ${_stat##*) }; [ "$#" -ge 6 ] || return 1
  [ "$6" -gt 0 ] 2>/dev/null || return 1
  printf '%s|%s' "$3" "$6"
}

find_tree_agent() {
  for _c in $(tree_pids "$1"); do
    if [ -n "${2:-}" ]; then
      _groups=$(process_groups "$_c") || continue
      [ "${_groups%%|*}" = "$2" ] || continue
    fi
    if _k=$(proc_kind "$_c"); then printf '%s|%s' "$_c" "$_k"; return 0; fi
  done; return 1
}

newest_in() {
  _b=""
  for _x in "$1"/*.jsonl; do
    [ -f "$_x" ] || continue
    [ -z "$_b" ] || [ "$_x" -nt "$_b" ] && _b="$_x"
  done
  [ -n "$_b" ] && printf '%s' "$_b"
}

resolve_claude() {
  _cur="${1%/}"; _pj="$HOME/.claude/projects"; [ -d "$_pj" ] || return 1
  _i=0
  while [ "$_i" -le 3 ]; do
    [ -z "$_cur" ] || [ "$_cur" = "/" ] || [ "$_cur" = "$HOME" ] && break
    _sl=$(printf '%s' "$_cur" | tr '/.' '--')
    _hit=$(newest_in "$_pj/$_sl")
    if [ -n "$_hit" ]; then printf '%s' "$_hit"; return 0; fi
    _pa="${_cur%/*}"; [ "$_pa" = "$_cur" ] && break
    _cur="$_pa"; _i=$((_i + 1))
  done
  return 1
}

codex_is_sub() {
  case "$1" in *',"source":{"subagent'*|*', "source": {"subagent'*|*'{"source":{"subagent'*|*'{"source": {"subagent'*) return 0 ;; esac
  return 1
}

codex_fd() {
  [ -n "$1" ] || return 1; _sr="${CODEX_HOME:-$HOME/.codex}/sessions"
  for _cp in $(tree_pids "$1"); do
    [ -d "$P_ROOT/$_cp/fd" ] || continue
    for _fd in "$P_ROOT/$_cp/fd"/*; do
      _t=$(readlink "$_fd" 2>/dev/null) || continue
      case "$_t" in "$_sr/"*.jsonl) [ -f "$_t" ] || continue
        _hd=$(head -c 32768 "$_t" 2>/dev/null); codex_is_sub "$_hd" && continue
        printf '%s' "$_t"; return 0 ;; esac
    done
  done; return 1
}

codex_anc() {
  _hd="$1"; _an="$2"; _d=0
  while [ "$_d" -lt 3 ]; do
    _pa="${_an%/*}"; [ -z "$_pa" ] || [ "$_pa" = "$_an" ] || [ "$_pa" = "/" ] || [ "$_pa" = "$HOME" ] && break
    _an="$_pa"; _d=$((_d + 1))
    case "$_hd" in *'"cwd":"'"$_an"'"'*| *'"cwd": "'"$_an"'"'*) return 0 ;; esac
  done; return 1
}

codex_has_tok() { tail -c 65536 "$1" 2>/dev/null | grep -q '"token_count"'; }

resolve_codex() {
  _cw="${1%/}"; _pd="$2"
  _hit=$(codex_fd "$_pd") && [ -n "$_hit" ] && { printf '%s' "$_hit"; return 0; }
  _sr="${CODEX_HOME:-$HOME/.codex}/sessions"; [ -d "$_sr" ] || return 1
  set --; for _g in "$_sr"/*/*/*/*.jsonl "$_sr"/*.jsonl; do [ -f "$_g" ] && set -- "$@" "$_g"; done
  [ "$#" -gt 0 ] || return 1
  _lst=$(ls -1t "$@" 2>/dev/null | head -n 48)
  _bet=""; _be=""; _bst=""; _bs=""; _ba=""; _oifs="$IFS"; IFS='
'
  set -f
  for _f in $_lst; do
    IFS="$_oifs"; set +f; [ -f "$_f" ] || continue
    _hd=$(head -c 32768 "$_f" 2>/dev/null); codex_is_sub "$_hd" && continue
    if [ -z "$_cw" ]; then [ -z "$_ba" ] && _ba="$_f"; codex_has_tok "$_f" && { _ba="$_f"; break; }; continue; fi
    case "$_hd" in *'"cwd":"'"$_cw"'"'*|*'"cwd": "'"$_cw"'"'*)
      [ -z "$_be" ] && _be="$_f"
      codex_has_tok "$_f" && { _bet="$_f"; break; }; continue ;; esac
    if [ -z "$_bst" ] && codex_anc "$_hd" "$_cw"; then
      [ -z "$_bs" ] && _bs="$_f"; codex_has_tok "$_f" && _bst="$_f"
    fi
  done
  IFS="$_oifs"; set +f
  for _r in "$_bet" "$_be" "$_bst" "$_bs"; do [ -n "$_r" ] && { printf '%s' "$_r"; return 0; }; done
  [ -z "$_cw" ] && [ -n "$_ba" ] && { printf '%s' "$_ba"; return 0; }
  return 1
}

ws_match() {
  _tx="$1"; _ws="${2%/}"; [ -n "$_ws" ] || return 2; _sb=1
  while :; do
    case "$_tx" in *"$_ws"*)
      _tx="${_tx#*"$_ws"}"; _ch=$(printf '%.1s' "$_tx")
      case "$_ch" in /) _sb=0 ;; [A-Za-z0-9_.-]) ;; *) return 0 ;; esac ;;
      *) break ;; esac
  done
  [ "$_sb" -eq 0 ] && return 1
  return 2
}

agy_is_sub() {
  [ -n "$2" ] || return 1
  for _sf in "$1"/*/.system_generated/subagents/"$2".json; do
    [ -f "$_sf" ] && return 0
  done
  return 1
}

agy_pids() {
  _pl="$1"
  for _cf in "$P_ROOT/$1/task"/*/children "$P_ROOT/$1/children"; do
    [ -r "$_cf" ] && _pl="$_pl $(tr '\n\t' '  ' < "$_cf" 2>/dev/null)"
  done
  printf '%s' "$_pl"
}

agy_fd() {
  _pd="$1"; _br="$2"; [ -n "$_pd" ] || return 1; _fb=""
  for _ap in $(agy_pids "$_pd"); do
    [ -d "$P_ROOT/$_ap/fd" ] || continue
    for _fd in "$P_ROOT/$_ap/fd"/*; do
      _t=$(readlink "$_fd" 2>/dev/null) || continue
      case "$_t" in "$_br/"*) _rl="${_t#"$_br/"}"; _id="${_rl%%/*}"; [ -n "$_id" ] || continue
        agy_is_sub "$_br" "$_id" && continue
        _cd="$_br/$_id/.system_generated/logs/transcript.jsonl"
        [ -f "$_cd" ] || _cd="$_br/$_id/transcript.jsonl"; [ -f "$_cd" ] || continue
        case "${_t##*/}" in transcript*) printf '%s' "$_cd"; return 0 ;; esac
        [ -z "$_fb" ] || [ "$_cd" -nt "$_fb" ] && _fb="$_cd" ;; esac
    done
  done
  [ -n "$_fb" ] && { printf '%s' "$_fb"; return 0; }
  return 1
}

resolve_agy() {
  _cw="${1%/}"; _pd="$2"; _br="$HOME/.gemini/antigravity-cli/brain"
  [ -d "$_br" ] || return 1
  _hit=$(agy_fd "$_pd" "$_br") && [ -n "$_hit" ] && { printf '%s' "$_hit"; return 0; }
  [ -n "$_cw" ] || return 1; _be=""; _bs=""
  for _cd in "$_br"/*/.system_generated/logs/transcript.jsonl "$_br"/*/transcript.jsonl; do
    [ -f "$_cd" ] || continue
    _rl="${_cd#"$_br/"}"; _id="${_rl%%/*}"; agy_is_sub "$_br" "$_id" && continue
    _hd=$(head -c 65536 "$_cd" 2>/dev/null)
    _rc=0; ws_match "$_hd" "$_cw" || _rc=$?
    [ "$_rc" -eq 0 ] && { [ -z "$_be" ] || [ "$_cd" -nt "$_be" ] && _be="$_cd"; }
    [ "$_rc" -eq 1 ] && { [ -z "$_bs" ] || [ "$_cd" -nt "$_bs" ] && _bs="$_cd"; }
  done
  for _r in "$_be" "$_bs"; do [ -n "$_r" ] && { printf '%s' "$_r"; return 0; }; done
  return 1
}

opencode_fd() {
  for p in $(tree_pids "$1"); do
    for fd in "$P_ROOT/$p/fd/"*; do
      path=$(readlink "$fd" 2>/dev/null) || continue
      case "$path" in */akhterm/opencode/client-*/ses_*.jsonl) [ -f "$path" ] && { printf '%s' "$path"; return 0; } ;; esac
    done
  done
  return 1
}

resolve_for_kind() {
  case "$1" in
    opencode) opencode_fd "$3" ;;
    claude) resolve_claude "$2" ;;
    codex) resolve_codex "$2" "$3" ;;
    agy) resolve_agy "$2" "$3" ;;
    *) return 1 ;;
  esac
}

emit_slice() {
  _s="$1"; _k="$2"; _f="$3"; _pp="$4"; _po="$5"
  if [ -z "$_f" ] || [ ! -f "$_f" ]; then
    printf 'AKHTERM_RTELEM1|%s|NONE|-|0|0|0|0|\n\nAKHTERM_RTELEM_END\n' "$_s"; return 0
  fi
  _sz=$(wc -c < "$_f" 2>/dev/null | tr -d ' \t\r\n'); [ -n "$_sz" ] || _sz=0
  _mt=$(stat -c '%Y' "$_f" 2>/dev/null || stat -f '%m' "$_f" 2>/dev/null || echo 0)
  _mt=$(printf '%s' "$_mt" | tr -d ' \t\r\n'); [ -n "$_mt" ] || _mt=0
  if [ "$_f" != "$_pp" ] || [ "$_po" -le 0 ] || [ "$_sz" -lt "$_po" ]; then
    if [ "$_sz" -le "$AKHTERM_MAX_FULL" ]; then
      printf 'AKHTERM_RTELEM1|%s|FULL|%s|%s|%s|0|%s|%s\n' "$_s" "$_k" "$_sz" "$_mt" "$_sz" "$_f"
      [ "$_sz" -gt 0 ] && head -c "$_sz" "$_f"; printf '\nAKHTERM_RTELEM_END\n'
    else
      printf 'AKHTERM_RTELEM1|%s|HEAD_TAIL|%s|%s|%s|%s|%s|%s\n' "$_s" "$_k" "$_sz" "$_mt" "$AKHTERM_HEAD_B" "$AKHTERM_TAIL_B" "$_f"
      head -c "$AKHTERM_HEAD_B" "$_f"; tail -c "$AKHTERM_TAIL_B" "$_f" | head -c "$AKHTERM_TAIL_B"; printf '\nAKHTERM_RTELEM_END\n'
    fi
  elif [ "$_sz" -eq "$_po" ]; then
    printf 'AKHTERM_RTELEM1|%s|UNCHANGED|%s|%s|%s|0|0|%s\n\nAKHTERM_RTELEM_END\n' "$_s" "$_k" "$_sz" "$_mt" "$_f"
  else
    _dl=$((_sz - _po))
    if [ "$_dl" -le "$AKHTERM_MAX_FULL" ]; then
      _st=$((_po + 1))
      printf 'AKHTERM_RTELEM1|%s|DELTA|%s|%s|%s|0|%s|%s\n' "$_s" "$_k" "$_sz" "$_mt" "$_dl" "$_f"
      tail -c +"$_st" "$_f" | head -c "$_dl"; printf '\nAKHTERM_RTELEM_END\n'
    else
      printf 'AKHTERM_RTELEM1|%s|TAIL_DELTA|%s|%s|%s|0|%s|%s\n' "$_s" "$_k" "$_sz" "$_mt" "$AKHTERM_TAIL_B" "$_f"
      tail -c "$AKHTERM_TAIL_B" "$_f" | head -c "$AKHTERM_TAIL_B"; printf '\nAKHTERM_RTELEM_END\n'
    fi
  fi
}
