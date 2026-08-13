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

mark_release_pr_tagged() {
  release_title="chore: release Mux $version"
  release_pr=$(gh pr list --state merged --base main --limit 100 \
    --json number,title \
    --jq ".[] | select(.title == \"$release_title\") | .number" | head -n 1)
  if [ -z "$release_pr" ]; then
    echo "could not find the merged release PR for $tag" >&2
    return 1
  fi

  gh label create 'autorelease: tagged' \
    --color ededed \
    --description 'Release has been tagged' \
    --force >/dev/null
  if gh pr view "$release_pr" --json labels --jq '.labels[].name' |
    grep -Fxq 'autorelease: pending'; then
    gh pr edit "$release_pr" --remove-label 'autorelease: pending' >/dev/null
  fi
  gh pr edit "$release_pr" --add-label 'autorelease: tagged' >/dev/null
}

if gh release view "$tag" >/dev/null 2>&1; then
  echo "release $tag already exists"
  mark_release_pr_tagged
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

release_commit=${MUX_RELEASE_COMMIT:-$(git rev-parse HEAD)}
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
mark_release_pr_tagged
echo "created=true" >> "${GITHUB_OUTPUT:-/dev/null}"
echo "artifacts_needed=true" >> "${GITHUB_OUTPUT:-/dev/null}"
echo "tag=$tag" >> "${GITHUB_OUTPUT:-/dev/null}"
