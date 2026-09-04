# Jet protocol fuzzing

Install `cargo-fuzz`, then exercise both untrusted protocol boundaries:

```sh
cargo fuzz run control
cargo fuzz run frames
```

The control target drives every strict JSON control envelope. The frame
target drives both legacy and multiplexed bounded frame readers over a closed
in-memory transport so incomplete declarations terminate instead of waiting.
