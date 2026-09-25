#!/usr/bin/env bash
set -euo pipefail

tag=$GITHUB_REF_NAME
# The core formula for macOS and Linux and the Linux desktop cask that depends
# on it, as tap paths. The Swift app's release owns Casks/jet.rb, and writes a
# macOS-only Formula/jet.rb only for a core version newer than the tap's, so
# a core release's formula replaces that one at the same version and stays.
files=(Formula/jet.rb Casks/jet-app.rb)
published="$RUNNER_TEMP/jet-homebrew"
highest_core_release="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/highest-core-release.rb"
# Every run brings the tap to the highest published stable core release, not
# to the tag that started it: the tap jobs share one concurrency group, and
# GitHub cancels a pending job when another queues, so the run that goes
# ahead may belong to an older tag. It is the release publish-release.sh
# marks Latest.
release=$(gh release list --repo apexgang/jet --limit 1000 --json tagName,isDraft,isPrerelease |
  ruby "$highest_core_release")
if [[ "$release" != "$tag" ]]; then
  echo "Updating the tap to $release, the latest stable core release, for $tag"
fi
# Reruns use the published formula and cask, never a second build.
gh release download "$release" --repo apexgang/jet --pattern jet.rb --pattern jet-app.rb \
  --dir "$published" --clobber
cd homebrew-tap
# One newer file skips both, so the tap never pairs the app with another
# release's daemon. An equal version replaces the Swift release's formula.
for file in "${files[@]}"; do
  if [[ -f $file ]]; then
    update=$(ruby -e '
      require "rubygems"
      current = File.read(ARGV[1])[/^  version "([0-9]+\.[0-9]+\.[0-9]+)"$/, 1]
      abort "Cannot read the current version of #{ARGV[1]}" unless current
      puts Gem::Version.new(ARGV[0].delete_prefix("v")) >= Gem::Version.new(current)
    ' "$release" "$file")
    if [[ "$update" != true ]]; then
      echo "Skipped $release: the tap's $file is newer"
      exit 0
    fi
  fi
done
for file in "${files[@]}"; do
  mkdir -p "$(dirname "$file")"
  cp "$published/$(basename "$file")" "$file"
  ruby -c "$file"
done
if [[ -z $(git status --porcelain -- "${files[@]}") ]]; then
  echo "Skipped $release: the tap already has this release"
  exit 0
fi
bot_id=$(gh api 'users/ape-bonker[bot]' --jq .id)
git config user.name 'ape-bonker[bot]'
git config user.email "${bot_id}+ape-bonker[bot]@users.noreply.github.com"
# One commit, so the tap never pairs the app with another release's daemon.
git add -- "${files[@]}"
git commit -m "Updated Jet to $release"
# The Swift app's release writes the tap under another concurrency group.
# Take its commits, and push only while the rebase kept this release's files
# as published; a rerun decides again against whatever it wrote.
if ! git pull --rebase origin main; then
  echo "The tap changed while $release was published; rerun the job" >&2
  exit 1
fi
for file in "${files[@]}"; do
  if ! cmp -s "$published/$(basename "$file")" "$file"; then
    echo "The tap's $file changed while $release was published; rerun the job" >&2
    exit 1
  fi
done
git push origin HEAD:main
