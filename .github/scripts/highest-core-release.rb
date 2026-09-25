# Prints the highest stable core release: the greatest vMAJOR.MINOR.PATCH tag
# among the published, non-prerelease releases that
# `gh release list --json tagName,isDraft,isPrerelease` writes to stdin, and
# the tags given as arguments. Versions compare numerically, and Swift app
# releases (swift-v*) and prerelease tags never count.
require "json"
require "rubygems"

STABLE = /\Av(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\z/

published = JSON.parse(STDIN.read)
                .reject { |release| release["isDraft"] || release["isPrerelease"] }
                .map { |release| release["tagName"] }
tags = (published + ARGV).grep(STABLE)
abort "No stable core release is published" if tags.empty?
puts tags.max_by { |tag| Gem::Version.new(tag.delete_prefix("v")) }
