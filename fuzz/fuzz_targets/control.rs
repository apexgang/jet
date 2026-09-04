#![no_main]

use jet_protocol::{
    ClientHello, ClientMessage, ServerHello, ServerMessage, StreamControl, decode_control,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let _ = decode_control::<serde_json::Value>(input);
    let _ = decode_control::<ClientHello>(input);
    let _ = decode_control::<ServerHello>(input);
    let _ = decode_control::<ClientMessage>(input);
    let _ = decode_control::<ServerMessage>(input);
    let _ = decode_control::<StreamControl>(input);
});
