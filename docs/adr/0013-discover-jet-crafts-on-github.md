# Discover Jet Crafts on GitHub

Jet discovers public GitHub repositories named `jet-craft-<slug>` only when they contain `.jet/craft-spec.toml`, whose Craft specification declares its own schema version. Normal installation uses declared prebuilt GitHub Release assets, verifies their hashes, pins the repository tag and commit, and requests declared capabilities; source builds and local repositories require Developer Mode, while private repositories are installed by explicit authenticated URL rather than public discovery.
