#!/usr/bin/env bash
set -euo pipefail

version=${1:?expected app version}
if [[ ! $version =~ ^1\.0\.[1-9][0-9]*$ ]]; then
  echo 'Unexpected app version' >&2
  exit 1
fi
cd homebrew-tap
mkdir -p Casks Formula
core_version=$(ruby -e 'source = File.read(ARGV[0]); match = source.match(/^  version "([0-9]+\.[0-9]+\.[0-9]+)"$/); abort "Invalid bundled core version" unless match; puts match[1]' ../swift-dist/jet-core.rb)
newer() {
  ruby -e 'require "rubygems"; exit(Gem::Version.new(ARGV[0]) > Gem::Version.new(ARGV[1]) ? 0 : 1)' "$1" "$2"
}
if [[ -f Formula/jet.rb ]]; then
  current_core=$(ruby -e 'source = File.read(ARGV[0]); match = source.match(/^  version "([0-9]+\.[0-9]+\.[0-9]+)"$/); abort "Invalid Jet formula version" unless match; puts match[1]' Formula/jet.rb)
else
  current_core=0.0.0
fi
if newer "$core_version" "$current_core"; then
  cp ../swift-dist/jet-core.rb Formula/jet.rb
  ruby -c Formula/jet.rb
fi
if [[ -f Casks/jet.rb ]]; then
  current_app=$(ruby -e 'source = File.read(ARGV[0]); match = source.match(/^  version "(1\.0\.[0-9]+)"$/); abort "Invalid Jet cask version" unless match; puts match[1]' Casks/jet.rb)
else
  current_app=0.0.0
fi
if newer "$version" "$current_app"; then
  cp ../swift-dist/jet.rb Casks/jet.rb
fi
ruby -c Casks/jet.rb
if [[ -z $(git status --porcelain -- Casks/jet.rb Formula/jet.rb) ]]; then
  echo "Skipped $version: the tap already has these versions"
  exit 0
fi
bot_id=$(gh api 'users/ape-bonker[bot]' --jq .id)
git config user.name 'ape-bonker[bot]'
git config user.email "${bot_id}+ape-bonker[bot]@users.noreply.github.com"
git add Casks/jet.rb Formula/jet.rb
git commit -m "Published Jet app $version and core $core_version"
git pull --rebase origin main
git push origin HEAD:main
