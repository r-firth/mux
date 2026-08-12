#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_dir"

version=$(cargo metadata --no-deps --format-version 1 |
  jq -r '.packages[] | select(.name == "mux") | .version' |
  head -n 1)
if [ -z "$version" ]; then
  echo "could not determine the mux package version" >&2
  exit 1
fi

tag="v$version"
release_subject="chore(mux): release v$version"
release_commit=$(git log --format='%H%x09%s' |
  awk -F '\t' -v subject="$release_subject" '
    $2 == subject || index($2, subject " (#") == 1 { print $1; exit }
  ')
if [ -z "$release_commit" ]; then
  echo "release commit not found: $release_subject" >&2
  exit 1
fi

notes_file=$(mktemp)
trap 'rm -f "$notes_file"' EXIT HUP INT TERM
awk -v heading="## [$version]" '
  index($0, heading) == 1 { found = 1; next }
  found && /^## \[/ { exit }
  found { print }
' CHANGELOG.md > "$notes_file"
if ! grep -q '[^[:space:]]' "$notes_file"; then
  echo "CHANGELOG.md has no release notes for $version" >&2
  exit 1
fi

if gh release view "$tag" >/dev/null 2>&1; then
  echo "release $tag already exists"
  asset_count=$(gh release view "$tag" --json assets --jq '.assets | length')
  echo "created=false" >> "${GITHUB_OUTPUT:-/dev/null}"
  if [ "$asset_count" -lt 4 ]; then
    echo "artifacts_needed=true" >> "${GITHUB_OUTPUT:-/dev/null}"
  else
    echo "artifacts_needed=false" >> "${GITHUB_OUTPUT:-/dev/null}"
  fi
  echo "tag=$tag" >> "${GITHUB_OUTPUT:-/dev/null}"
  exit 0
fi

if gh api "repos/{owner}/{repo}/git/ref/tags/$tag" >/dev/null 2>&1; then
  gh release create "$tag" \
    --verify-tag \
    --title "Mux v$version" \
    --notes-file "$notes_file"
else
  gh release create "$tag" \
    --target "$release_commit" \
    --title "Mux v$version" \
    --notes-file "$notes_file"
fi

gh release view "$tag" >/dev/null
echo "created=true" >> "${GITHUB_OUTPUT:-/dev/null}"
echo "artifacts_needed=true" >> "${GITHUB_OUTPUT:-/dev/null}"
echo "tag=$tag" >> "${GITHUB_OUTPUT:-/dev/null}"
