# Keep the Craft protocol language-neutral

A Jet Craft may be any executable that implements the documented language-neutral Craft protocol and ships a valid `.jet/craft-spec.toml`. `jet-craft-sdk` is the preferred Rust convenience SDK and is used by bundled Crafts, but Rust is not an installation requirement. The SDK exposes protocol handling and presentation builders without granting access to `jet-core` internals.
