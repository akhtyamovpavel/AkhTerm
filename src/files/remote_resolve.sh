resolve_unique() {
  case "$1" in /) p='';; *) p="$1";; esac
  t="$p/$2"
  [ ! -e "$t" ] && [ ! -L "$t" ] && { printf '%s' "$t"; return; }
  case "$2" in ?*.*) b=${2%.*}; e=.${2##*.};; *) b="$2"; e='';; esac
  i=1
  while :; do
    t="$p/$b ($i)$e"
    [ ! -e "$t" ] && [ ! -L "$t" ] && { printf '%s' "$t"; return; }
    i=$((i + 1))
  done
}
