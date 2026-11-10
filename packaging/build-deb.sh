#!/usr/bin/env bash
set -euo pipefail

packaging_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd -- "$packaging_dir/.."

package_id="$(cargo pkgid --offline --locked)"
version="${package_id##*#}"
version="${version##*@}-1"
architecture="$(dpkg --print-architecture)"
output_dir="$PWD/target/deb"
package_file="akhterm_${version}_${architecture}.deb"

cargo build --release --offline --locked --target-dir target
mkdir -p "$output_dir"
build_dir="$(mktemp -d "$output_dir/.build.XXXXXX")"
trap 'rm -rf -- "$build_dir"' EXIT

package_dir="$build_dir/debian/akhterm"
doc_dir="$package_dir/usr/share/doc/akhterm"
install -Dm755 target/release/akhterm "$package_dir/usr/bin/akhterm"
install -Dm644 "$packaging_dir/org.akhterm.AkhTerm.desktop" \
    "$package_dir/usr/share/applications/org.akhterm.AkhTerm.desktop"
install -Dm644 "$packaging_dir/org.akhterm.AkhTerm.svg" \
    "$package_dir/usr/share/icons/hicolor/scalable/apps/org.akhterm.AkhTerm.svg"
install -Dm644 config.example.conf "$doc_dir/config.example.conf"
install -m644 LICENSE "$doc_dir/copyright"
install -m644 assets/fonts/OFL-Geist.txt "$doc_dir/"
install -m644 "$packaging_dir/NOTICE" /usr/share/common-licenses/{LGPL-2.1,LGPL-3,GPL-3} "$doc_dir/"

# dpkg-shlibdeps reads source metadata and scans the staged executable.
mkdir "$package_dir/DEBIAN"
echo "Source: akhterm" > "$build_dir/debian/control"
shlibs="$(cd -- "$build_dir" && dpkg-shlibdeps -O debian/akhterm/usr/bin/akhterm)"
depends="${shlibs#shlibs:Depends=}"

{
    echo "Package: akhterm"
    echo "Version: $version"
    echo "Architecture: $architecture"
    echo "Section: utils"
    echo "Priority: optional"
    echo "Maintainer: AkhTerm contributors"
    echo "Depends: $depends, librsvg2-common, openssh-client"
    echo "Recommends: xdg-utils"
    echo "Suggests: htop"
    echo "Description: Tabbed terminal for local shells and remote tmux sessions"
    echo " AkhTerm keeps local and SSH tabs in one window and restores their layout."
} > "$package_dir/DEBIAN/control"

dpkg-deb --root-owner-group --build "$package_dir" "$build_dir/$package_file"
mv -f -- "$build_dir/$package_file" "$output_dir/$package_file"
echo "$output_dir/$package_file"
