#!/usr/bin/env bash
set -euo pipefail

packaging_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd -- "$packaging_dir/.."

package_id="$(cargo pkgid --offline --locked)"
version="${package_id##*#}"
version="${version##*@}"

machine="$(uname -m)"
case "$machine" in
    x86_64|aarch64)
        arch="$machine"
        ;;
    *)
        echo "Unsupported architecture for portable tarball: $machine" >&2
        exit 1
        ;;
esac

if [[ "${AKHTERM_SKIP_CARGO_BUILD:-0}" != "1" ]]; then
    cargo build --release --offline --locked --target-dir target
fi

output_dir="$PWD/target/tarball"
stage_name="akhterm-${version}-linux-${arch}"
package_file="akhterm_${version}_linux_${arch}.tar.gz"

mkdir -p "$output_dir"
build_dir="$(mktemp -d "$output_dir/.build.XXXXXX")"
trap 'rm -r -f -- "$build_dir"' EXIT

stage_dir="$build_dir/$stage_name"
install -Dm755 target/release/akhterm "$stage_dir/akhterm"
install -Dm644 LICENSE "$stage_dir/LICENSE"
install -Dm644 "$packaging_dir/NOTICE" "$stage_dir/NOTICE"
install -Dm644 assets/fonts/OFL-Geist.txt "$stage_dir/OFL-Geist.txt"
install -Dm644 config.example.conf "$stage_dir/config.example.conf"
install -Dm644 "$packaging_dir/org.akhterm.AkhTerm.desktop" "$stage_dir/org.akhterm.AkhTerm.desktop"
install -Dm644 "$packaging_dir/org.akhterm.AkhTerm.svg" "$stage_dir/org.akhterm.AkhTerm.svg"

tar -C "$build_dir" -czf "$build_dir/$package_file" "$stage_name"
mv -f -- "$build_dir/$package_file" "$output_dir/$package_file"
echo "$output_dir/$package_file"
