#!/usr/bin/env bash
set -euo pipefail

tag=$GITHUB_REF_NAME
dist=packages/target/dist
desktop=$RUNNER_TEMP/jet-desktop
# The updater manifest dates the release by its tagged commit, so a rerun
# writes the same manifest.
pub_date=$(git show -s --format=%cI "$GITHUB_SHA")
python3 .github/scripts/release_assets.py --tag "$tag" --dist "$dist" --desktop "$desktop" --pub-date "$pub_date"
ruby -c "$dist/jet.rb"
ruby -c "$dist/jet-app.rb"

# Assemble in a draft so users never see an incomplete release. Published
# assets are immutable here; a rerun proceeds straight to repairing the tap.
if gh release view "$tag" --json isDraft > "$RUNNER_TEMP/jet-release.json" 2>/dev/null; then
  if ! jq -e '.isDraft' "$RUNNER_TEMP/jet-release.json" >/dev/null; then
    exit 0
  fi
else
  gh release create "$tag" --verify-tag --draft --title "Jet $tag" --generate-notes
fi
gh release upload "$tag" "$dist"/*.tar.gz "$desktop"/* "$dist/jet.rb" "$dist/jet-app.rb" \
  "$dist/latest.json" "$dist/SHA256SUMS" --clobber
if [[ "$tag" == *-* ]]; then
  gh release edit "$tag" --draft=false --prerelease --latest=false
else
  gh release edit "$tag" --draft=false --latest
fi
