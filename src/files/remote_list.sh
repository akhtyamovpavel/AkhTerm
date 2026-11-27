if command -v python3 >/dev/null 2>&1; then
  python3 -c 'import os,stat
for e in os.scandir("."):
 n=e.name
 if "\n" in n or "\r" in n or "\t" in n: continue
 try: st=e.stat(follow_symlinks=False)
 except OSError: continue
 k="l" if stat.S_ISLNK(st.st_mode) else ("d" if stat.S_ISDIR(st.st_mode) else "f")
 s=0 if stat.S_ISDIR(st.st_mode) else st.st_size
 print(f"{k}\t{s}\t{int(st.st_mtime)}\t{n}")'
else
  for e in .* *; do
    case "$e" in .|..) continue;; esac
    [ -e "$e" ] || [ -L "$e" ] || continue
    k=f; [ -L "$e" ] && k=l; [ ! -L "$e" ] && [ -d "$e" ] && k=d
    printf '%s\t0\t0\t%s\n' "$k" "$e"
  done
fi
