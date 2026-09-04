#![no_main]

use jet_protocol::FrameReader;
use libfuzzer_sys::fuzz_target;
use tokio::io::{AsyncWriteExt, duplex};

fuzz_target!(|input: &[u8]| {
    let bytes = input.to_vec();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("the fuzz runtime must initialize");
    runtime.block_on(async move {
        let capacity = bytes.len().saturating_add(1).max(16);
        let (mut peer, transport) = duplex(capacity);
        peer.write_all(&bytes)
            .await
            .expect("the in-memory peer must accept fuzz bytes");
        peer.shutdown()
            .await
            .expect("the in-memory peer must close");
        let mut reader = FrameReader::new(transport);
        if bytes.first().is_some_and(|byte| byte & 1 == 1) {
            reader.enable_multiplexing();
        }
        let _ = reader.read().await;
    });
});
