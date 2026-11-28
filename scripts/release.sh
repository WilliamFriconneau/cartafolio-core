#!/bin/sh
set -eu

usage() {
    echo "Usage: scripts/release.sh MAJOR.MINOR.PATCH" >&2
    exit 1
}
[ "$#" -eq 1 ] || usage
case "$1" in
    *[!0-9.]*) usage ;;
esac
printf '%s\n' "$1" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' || usage

version=$1
tag=v$version
cd "$(dirname "$0")/.."

status=$(git status --porcelain)
if [ -n "$status" ]; then
    echo "The working tree must be clean." >&2
    exit 1
fi
if git show-ref --verify --quiet "refs/tags/$tag"; then
    echo "Tag $tag already exists." >&2
    exit 1
fi
if grep -Fxq "version = \"$version\"" Cargo.toml; then
    echo "The workspace is already at version $version." >&2
    exit 1
fi

scripts/check.sh
addon=ha-addon/pensine
sed -i "s/^version = \"[^\"]*\"$/version = \"$version\"/" Cargo.toml
sed -i "s/^version: \"[^\"]*\"$/version: \"$version\"/" "$addon/config.yaml"
sed -i "s/^  PENSINE_VERSION: \"[^\"]*\"$/  PENSINE_VERSION: \"$version\"/" "$addon/build.yaml"
cargo update --workspace --offline

git add Cargo.toml Cargo.lock "$addon/config.yaml" "$addon/build.yaml"
git commit -m "Release $version"
git tag "$tag"
printf 'git push --atomic origin HEAD refs/tags/%s\n' "$tag"
