#!/usr/bin/env bash
set -euo pipefail

tag=$GITHUB_REF_NAME
dist=packages/target/dist
python3 .github/scripts/release_assets.py --tag "$tag" --dist "$dist"
ruby -c "$dist/jet.rb"

# Assemble in a draft so users never see an incomplete release. Published
# assets are immutable here; a rerun proceeds straight to repairing the tap.
if gh release view "$tag" --json isDraft > "$RUNNER_TEMP/jet-release.json" 2>/dev/null; then
  if ! jq -e '.isDraft' "$RUNNER_TEMP/jet-release.json" >/dev/null; then
    exit 0
  fi
else
  gh release create "$tag" --verify-tag --draft --title "Jet $tag" --generate-notes
fi
gh release upload "$tag" "$dist"/*.tar.gz "$dist/SHA256SUMS" "$dist/jet.rb" --clobber
if [[ "$tag" == *-* ]]; then
  gh release edit "$tag" --draft=false --prerelease --latest=false
else
  gh release edit "$tag" --draft=false --latest
fi
