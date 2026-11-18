# Match the displayed SSH transport before inspecting its foreground agent.
same_connection() {
  [ -r "$P_ROOT/$1/environ" ] || return 1
  connection=$(tr '\000' '\n' < "$P_ROOT/$1/environ" 2>/dev/null | sed -n 's/^SSH_CONNECTION=//p')
  set -- $connection
  [ "$#" = 4 ] && [ "$2" = "$client_port" ] && [ "$4" = "$server_port" ] || return 1
  [ -z "${probe_peer:-}" ] || [ "$1" = "$probe_peer" ]
}

probe_plain() {
  slot=$1
  client_port=$2
  server_port=$3
  previous=$4
  offset=$5
  probe_peer=${SSH_CONNECTION%% *}
  if [ "$client_port" = 0 ]; then emit_slice "$slot" "" "" "" 0; return; fi
  client_panes=$(tmux list-clients -F '#{client_pid}|#{session_name}|#{pane_id}' 2>/dev/null)
  matching=$(printf '%s\n' "$client_panes" | while IFS='|' read -r pid name pane; do
    case "$pid" in ''|*[!0-9]*) continue ;; esac
    same_connection "$pid" && printf '%s|%s\n' "$name" "$pane"
  done)
  case "$matching" in
    *'
'*) emit_slice "$slot" "" "" "" 0; return ;;
    *'|'*) probe_tmux "$slot" "${matching%|*}" "${matching##*|}" "" "" "$previous" "" "$offset"; return ;;
  esac
  selected=""
  kind=""
  shell_pid=""
  foreground_groups=""
  for dir in "$P_ROOT"/[0-9]*; do
    pid=${dir##*/}
    [ -d "$dir" ] && [ -O "$dir" ] || continue
    case " ${TMUX_PIDS:-} " in *" $pid "*) continue ;; esac
    same_connection "$pid" || continue
    if [ -r "$dir/stat" ]; then
      stat=$(cat "$dir/stat" 2>/dev/null)
      set -- ${stat##*) }
      [ "$3" = "$6" ] || continue
      case " $foreground_groups " in *" $6 "*) ;; *) foreground_groups="${foreground_groups:+$foreground_groups }$6" ;; esac
      if [ "$pid" = "$6" ]; then
        executable=$(tr '\000' '\n' < "$dir/cmdline" 2>/dev/null | head -n 1)
        case "${executable##*/}" in
          bash|-bash|zsh|-zsh|sh|-sh|fish|-fish|dash|-dash|ksh|-ksh|tcsh|-tcsh|csh|-csh|nu|-nu) shell_pid=$pid ;;
        esac
      fi
    fi
    candidate=$(proc_kind "$pid") || continue
    cwd=$(readlink "$dir/cwd" 2>/dev/null)
    file=$(resolve_pane_transcript "$candidate" "$cwd" "$pid") || continue
    if [ -n "$selected" ] && [ "$selected" != "$file" ]; then
      emit_slice "$slot" "" "" "" 0
      return
    fi
    selected=$file
    kind=$candidate
  done
  if [ -z "$selected" ] && [ -n "$shell_pid" ] && [ "$shell_pid" = "$foreground_groups" ]; then
    printf 'AKHTERM_RTELEM1|%s|SHELL|-|0|0|0|0|\n\nAKHTERM_RTELEM_END\n' "$slot"
  else
    emit_slice "$slot" "$kind" "$selected" "$previous" "$offset"
  fi
}
