#!/usr/bin/env bash
# Publishes the tag's release from the downloaded artifacts. With --check, it
# only writes `published=true|false` to GITHUB_OUTPUT, so the publish job can
# skip downloading artifacts, which expire after seven days, for a release
# that is already published.
set -euo pipefail

tag=$GITHUB_REF_NAME
dist=packages/target/dist
desktop=$RUNNER_TEMP/jet-desktop
highest_core_release="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/highest-core-release.rb"

# published, draft, or missing.
release_state() {
  local json
  if json=$(gh release view "$tag" --json isDraft 2>/dev/null); then
    jq -r 'if .isDraft then "draft" else "published" end' <<<"$json"
  else
    echo missing
  fi
}

# The highest published stable core release, with any tags given counted as
# published. update-homebrew.sh brings the tap to the same release.
highest_stable() {
  gh release list --repo "$GITHUB_REPOSITORY" --limit 1000 --json tagName,isDraft,isPrerelease |
    ruby "$highest_core_release" "$@"
}

# The Linux app's updater reads releases/latest/download/latest.json, and the
# jet-app cask's livecheck reads the latest release, so GitHub's Latest must be
# the highest stable core release. A patch for an older version never takes
# it, and other workflows (the Swift app's) create releases with
# --latest=false; fail loudly if another release still holds it.
require_latest() {
  [[ "$tag" == *-* ]] && return 0
  local latest highest
  latest=$(gh api "repos/$GITHUB_REPOSITORY/releases/latest" --jq .tag_name)
  highest=$(highest_stable)
  if [[ "$latest" != "$highest" ]]; then
    echo "GitHub's latest release is $latest, not the highest stable core release $highest: the desktop updater and the jet-app cask would offer $latest. Run: gh release edit $highest --latest" >&2
    exit 1
  fi
}

state=$(release_state)
if [[ "${1:-}" == --check ]]; then
  if [[ "$state" == published ]]; then
    echo published=true >> "$GITHUB_OUTPUT"
  else
    echo published=false >> "$GITHUB_OUTPUT"
  fi
  exit 0
fi
# Published assets are immutable here; a rerun only checks Latest and
# proceeds to repairing the tap.
if [[ "$state" == published ]]; then
  require_latest
  exit 0
fi

# The updater manifest dates the release by its tagged commit, so a rerun
# writes the same manifest.
pub_date=$(git show -s --format=%cI "$GITHUB_SHA")
python3 .github/scripts/release_assets.py --tag "$tag" --dist "$dist" --desktop "$desktop" --pub-date "$pub_date"
ruby -c "$dist/jet.rb"
ruby -c "$dist/jet-app.rb"

# Assemble in a draft so users never see an incomplete release.
if [[ "$state" == missing ]]; then
  gh release create "$tag" --verify-tag --draft --title "Jet $tag" --generate-notes
fi
gh release upload "$tag" "$dist"/*.tar.gz "$desktop"/* "$dist/jet.rb" "$dist/jet-app.rb" \
  "$dist/latest.json" "$dist/SHA256SUMS" --clobber
if [[ "$tag" == *-* ]]; then
  gh release edit "$tag" --draft=false --prerelease --latest=false
else
  highest=$(highest_stable "$tag")
  if [[ "$tag" == "$highest" ]]; then
    gh release edit "$tag" --draft=false --latest
  else
    gh release edit "$tag" --draft=false --latest=false
  fi
fi
require_latest
