#!/usr/bin/env bash
set -euo pipefail

packaging_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd -- "$packaging_dir/.."

tag="${1:?Usage: generate-release-notes.sh <tag> <owner/repo> <output-file>}"
repo="${2:-akhtyamovpavel/AkhTerm}"
out_file="${3:-dist/RELEASE_NOTES.md}"
version="${tag#v}"

mkdir -p "$(dirname -- "$out_file")"

curated_file="$packaging_dir/releases/${tag}.md"
if [[ -s "$curated_file" ]]; then
    cp -- "$curated_file" "$out_file"
    exit 0
fi

prev_tag="$(git describe --tags --abbrev=0 --match 'v*' "${tag}^" 2>/dev/null || true)"
if [[ -n "$prev_tag" ]]; then
    log_range="${prev_tag}..${tag}"
else
    log_range="$tag"
fi

feats=()
fixes=()
others=()

while IFS=$'\t' read -r sha subject; do
    if [[ -z "$sha" ]]; then
        continue
    fi
    case "$subject" in
        "chore(release):"*|"Merge "*)
            continue
            ;;
        feat*)
            feats+=("- ${subject} (\`${sha}\`)")
            ;;
        fix*)
            fixes+=("- ${subject} (\`${sha}\`)")
            ;;
        *)
            others+=("- ${subject} (\`${sha}\`)")
            ;;
    esac
done < <(git log --no-merges --format=$'%h\t%s' "$log_range")

{
    if [[ "${#feats[@]}" -gt 0 ]]; then
        echo "## Features in AkhTerm ${tag}"
        echo ""
        for item in "${feats[@]}"; do
            echo "$item"
        done
        echo ""
    fi
    if [[ "${#fixes[@]}" -gt 0 ]]; then
        echo "## Bug Fixes in AkhTerm ${tag}"
        echo ""
        for item in "${fixes[@]}"; do
            echo "$item"
        done
        echo ""
    fi
    if [[ "${#others[@]}" -gt 0 ]]; then
        echo "## Maintenance & Refactoring"
        echo ""
        for item in "${others[@]}"; do
            echo "$item"
        done
        echo ""
    fi
    if [[ "${#feats[@]}" -eq 0 && "${#fixes[@]}" -eq 0 && "${#others[@]}" -eq 0 ]]; then
        echo "## AkhTerm ${tag}"
        echo ""
        echo "- Maintenance release ${tag}."
        echo ""
    fi
    echo "## Installation"
    echo ""
    echo "### Ubuntu 24.04 & 26.04 (\`amd64\`)"
    echo ""
    echo '```sh'
    echo "sudo apt install ./akhterm_${version}-1_amd64.deb"
    echo '```'
    echo ""
    echo "### Fedora 41 & 42 (\`x86_64\`)"
    echo ""
    echo '```sh'
    echo "sudo dnf install ./akhterm-${version}-1.x86_64.rpm"
    echo '```'
    echo ""
    echo "### Portable Linux Tarball (\`x86_64\`, supports one-click in-app self-update)"
    echo ""
    echo '```sh'
    echo "tar -xzf akhterm_${version}_linux_x86_64.tar.gz"
    echo "install -m 0755 akhterm-${version}-linux-x86_64/akhterm ~/.local/bin/akhterm"
    echo '```'
    if [[ -n "$prev_tag" ]]; then
        echo ""
        echo "**Full Changelog:** https://github.com/${repo}/compare/${prev_tag}...${tag}"
    fi
} > "$out_file"
