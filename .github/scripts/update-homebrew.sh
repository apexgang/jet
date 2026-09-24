#!/usr/bin/env bash
set -euo pipefail

tag=$GITHUB_REF_NAME
# The daemon formula and the desktop cask that depends on it, as tap paths.
files=(Formula/jetd.rb Casks/jet-app.rb)
# Reruns use the published formula, cask and checksums, never a second build.
gh release download "$tag" --repo apexgang/jet --pattern jetd.rb --pattern jet-app.rb \
  --dir "$RUNNER_TEMP/jet-homebrew" --clobber
cd homebrew-tap
# A rerun of an old release must not downgrade the stable formula or cask.
latest=$(gh release view --repo apexgang/jet --json tagName --jq .tagName)
if [[ "$tag" != "$latest" ]]; then
  echo "Skipped $tag: the latest stable release is $latest"
  exit 0
fi
# One newer file skips both, so the tap never pairs the app with another
# release's daemon.
for file in "${files[@]}"; do
  if [[ -f $file ]]; then
    update=$(ruby -e '
      require "rubygems"
      # The cask names its version; the formula carries it in its release URLs.
      source = File.read(ARGV[1])
      current = source[/^\s*version "([0-9]+\.[0-9]+\.[0-9]+)"$/, 1] ||
                source[%r{/releases/download/v([0-9]+\.[0-9]+\.[0-9]+)/}, 1]
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
  cp "$RUNNER_TEMP/jet-homebrew/$(basename "$file")" "$file"
  ruby -c "$file"
done
if [[ -z $(git status --porcelain -- "${files[@]}") ]]; then
  exit 0
fi
bot_id=$(gh api 'users/ape-bonker[bot]' --jq .id)
git config user.name 'ape-bonker[bot]'
git config user.email "${bot_id}+ape-bonker[bot]@users.noreply.github.com"
# One commit, so the tap never pairs the app with another release's daemon.
git add -- "${files[@]}"
git commit -m "Updated Jet to $tag"
git push origin HEAD:main
