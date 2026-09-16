#!/usr/bin/env bash
set -euo pipefail

tag=$GITHUB_REF_NAME
# Reruns use the published formula and its checksums, never a second build.
gh release download "$tag" --repo apexgang/jet --pattern jet.rb --dir "$RUNNER_TEMP/jet-formula" --clobber
cd homebrew-tap
# A rerun of an old release must not downgrade the stable formula.
latest=$(gh release view --repo apexgang/jet --json tagName --jq .tagName)
if [[ "$tag" != "$latest" ]]; then
  echo "Skipped $tag: the latest stable release is $latest"
  exit 0
fi
mkdir -p Formula
if [[ -f Formula/jet.rb ]]; then
  update=$(ruby -e '
    require "rubygems"
    current = File.read(ARGV[1]).match(/^\s*version "([0-9]+\.[0-9]+\.[0-9]+)"$/)
    abort "Cannot read the current Jet formula version" unless current
    puts Gem::Version.new(ARGV[0].delete_prefix("v")) >= Gem::Version.new(current[1])
  ' "$tag" Formula/jet.rb)
  if [[ "$update" != true ]]; then
    echo "Skipped $tag: the tap already contains a newer version"
    exit 0
  fi
fi
cp "$RUNNER_TEMP/jet-formula/jet.rb" Formula/jet.rb
ruby -c Formula/jet.rb
if [[ -z $(git status --porcelain -- Formula/jet.rb) ]]; then
  exit 0
fi
bot_id=$(gh api 'users/ape-bonker[bot]' --jq .id)
git config user.name 'ape-bonker[bot]'
git config user.email "${bot_id}+ape-bonker[bot]@users.noreply.github.com"
git add Formula/jet.rb
git commit -m "Updated Jet to $tag"
git push origin HEAD:main
