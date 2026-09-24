#!/usr/bin/env bash
set -euo pipefail

tag=$GITHUB_REF_NAME
# The core formula for macOS and Linux and the Linux desktop cask that depends
# on it, as tap paths. The Swift app's release owns Casks/jet.rb, and writes a
# macOS-only Formula/jet.rb only for a core version newer than the tap's, so
# this release's formula replaces that one at the same version and stays.
files=(Formula/jet.rb Casks/jet-app.rb)
published="$RUNNER_TEMP/jet-homebrew"
# Reruns use the published formula, cask and checksums, never a second build.
gh release download "$tag" --repo apexgang/jet --pattern jet.rb --pattern jet-app.rb \
  --dir "$published" --clobber
cd homebrew-tap
# A rerun of an old release must not downgrade the stable formula or cask.
# Compare with the highest published stable core release, not the release
# GitHub marks Latest: publish-release.sh marks each stable release Latest as
# it publishes, so a patch for an older version can hold it.
latest=$(gh release list --repo apexgang/jet --limit 1000 --json tagName,isDraft,isPrerelease | ruby -rjson -e '
  require "rubygems"
  tags = JSON.parse(STDIN.read).reject { |release| release["isDraft"] || release["isPrerelease"] }
             .map { |release| release["tagName"] }
             .grep(/\Av(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\z/)
  abort "No stable core release is published" if tags.empty?
  puts tags.max_by { |tag| Gem::Version.new(tag.delete_prefix("v")) }
')
if [[ "$tag" != "$latest" ]]; then
  echo "Skipped $tag: the latest stable core release is $latest"
  exit 0
fi
# One newer file skips both, so the tap never pairs the app with another
# release's daemon. An equal version replaces the Swift release's formula.
for file in "${files[@]}"; do
  if [[ -f $file ]]; then
    update=$(ruby -e '
      require "rubygems"
      current = File.read(ARGV[1])[/^  version "([0-9]+\.[0-9]+\.[0-9]+)"$/, 1]
      abort "Cannot read the current version of #{ARGV[1]}" unless current
      puts Gem::Version.new(ARGV[0].delete_prefix("v")) >= Gem::Version.new(current)
    ' "$tag" "$file")
    if [[ "$update" != true ]]; then
      echo "Skipped $tag: the tap's $file is newer"
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
  echo "Skipped $tag: the tap already has this release"
  exit 0
fi
bot_id=$(gh api 'users/ape-bonker[bot]' --jq .id)
git config user.name 'ape-bonker[bot]'
git config user.email "${bot_id}+ape-bonker[bot]@users.noreply.github.com"
# One commit, so the tap never pairs the app with another release's daemon.
git add -- "${files[@]}"
git commit -m "Updated Jet to $tag"
# The Swift app's release writes the tap under another concurrency group.
# Take its commits, and push only while the rebase kept this release's files
# as published; a rerun decides again against whatever it wrote.
if ! git pull --rebase origin main; then
  echo "The tap changed while $tag was published; rerun the job" >&2
  exit 1
fi
for file in "${files[@]}"; do
  if ! cmp -s "$published/$(basename "$file")" "$file"; then
    echo "The tap's $file changed while $tag was published; rerun the job" >&2
    exit 1
  fi
done
git push origin HEAD:main
