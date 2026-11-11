#!/usr/bin/env bash
set -euo pipefail

packaging_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd -- "$packaging_dir/.."

package_id="$(cargo pkgid --offline --locked)"
version="${package_id##*#}"
version="${version##*@}"
arch="$(rpm --eval '%{_arch}')"
output_dir="$PWD/target/rpm"
package_file="akhterm-${version}-1.${arch}.rpm"

cargo build --release --offline --locked --target-dir target
mkdir -p "$output_dir"
build_dir="$(mktemp -d "$output_dir/.build.XXXXXX")"
trap 'rm -r -f -- "$build_dir"' EXIT

mkdir -p "$build_dir"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}

rpmbuild -bb \
    --define "_topdir $build_dir" \
    --define "pkg_version $version" \
    --define "repo_root $PWD" \
    "$packaging_dir/akhterm.spec"

shopt -s nullglob
built_rpms=("$build_dir/RPMS/$arch"/akhterm-"${version}"-*.rpm)
shopt -u nullglob

if [[ "${#built_rpms[@]}" -ne 1 ]]; then
    echo "Expected one RPM under $build_dir/RPMS/$arch, found ${#built_rpms[@]}" >&2
    exit 1
fi

mv -f -- "${built_rpms[0]}" "$output_dir/$package_file"
echo "$output_dir/$package_file"
