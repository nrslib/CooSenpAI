#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository_dir=$(CDPATH= cd -- "$script_dir/../.." && pwd)
source_dir=${1:?usage: import.sh /absolute/path/to/habitua-checkout}
case "$source_dir" in
  /*) ;;
  *) printf 'habitua の絶対パスを指定してください\n' >&2; exit 2 ;;
esac
source_dir=$(CDPATH= cd -- "$source_dir" && pwd)
commit=$(git -C "$source_dir" rev-parse --verify 'a3d83f2^{commit}')
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT HUP INT TERM
git -C "$source_dir" archive "$commit" -- \
  Cargo.toml Cargo.lock README.md LICENSE-MIT LICENSE-APACHE \
  crates/habitua crates/habitua-connectome \
  dist/connectome/malecns-frozen-response-learning-artifact-r14.json \
  dist/connectome/malecns-frozen-response-learning-artifact-r14.json.sha256 | tar -x -C "$staging"
(cd "$staging/dist/connectome" && shasum -a 256 -c malecns-frozen-response-learning-artifact-r14.json.sha256 >/dev/null)
destination="$repository_dir/third_party/habitua"
rm -rf "$destination"
mkdir -p "$destination"
mkdir -p "$destination/crates"
cp -R "$staging/crates/." "$destination/crates/"
cp "$staging/Cargo.toml" "$staging/Cargo.lock" "$staging/README.md" \
  "$staging/LICENSE-MIT" "$staging/LICENSE-APACHE" "$destination/"
mkdir -p "$repository_dir/dist/connectome"
cp "$staging/dist/connectome/malecns-frozen-response-learning-artifact-r14.json" \
  "$staging/dist/connectome/malecns-frozen-response-learning-artifact-r14.json.sha256" \
  "$repository_dir/dist/connectome/"
printf 'Upstream commit: %s\nSnapshot: git archive of pinned commit\nRefresh: tools/connectome-helper/import.sh /absolute/path/to/habitua-checkout\n' \
  "$commit" > "$destination/SOURCE.txt"
