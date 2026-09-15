//! Hostile input at both untrusted protocol boundaries, in the ordinary
//! suite: a fixed corpus of malformed control envelopes and frame headers,
//! and deterministic mutation of every shared contract fixture through the
//! same decoders `jet-protocol-fuzz` drives under libFuzzer. Every decoder
//! must refuse or accept without panicking, and the fixed corpus must be
//! refused outright (ADR-0089, ADR-0094).
use jet_protocol::{
	ClientHello, ClientMessage, CraftCommand, CraftEvent, Frame, FrameError,
	FrameKind, FrameLimits, FrameReader, ProtocolOffer, ServerHello,
	ServerMessage, StreamControl, decode_control,
};
use pretty_assertions::assert_eq;
use serde::Deserialize;
use tokio::io::{AsyncWriteExt, duplex};

#[derive(Deserialize)]
struct Fixture {
	payload: String,
}

/// Every strict control decoder, as the `control` fuzz target lists them,
/// plus the Craft family. Returns how many accepted the input.
fn decode_everywhere(input: &[u8]) -> usize {
	let _ = decode_control::<serde_json::Value>(input);
	[
		decode_control::<ClientHello>(input).is_ok(),
		decode_control::<ServerHello>(input).is_ok(),
		decode_control::<ClientMessage>(input).is_ok(),
		decode_control::<ServerMessage>(input).is_ok(),
		decode_control::<StreamControl>(input).is_ok(),
		decode_control::<CraftCommand>(input).is_ok(),
		decode_control::<CraftEvent>(input).is_ok(),
		decode_control::<ProtocolOffer>(input).is_ok(),
	]
	.into_iter()
	.filter(|accepted| *accepted)
	.count()
}

fn fixture_payloads() -> Vec<Vec<u8>> {
	[
		include_str!("../contracts/jet-fixtures.json"),
		include_str!("../contracts/craft-fixtures.json"),
	]
	.into_iter()
	.flat_map(|corpus| serde_json::from_str::<Vec<Fixture>>(corpus).unwrap())
	.map(|fixture| fixture.payload.into_bytes())
	.collect()
}

/// A small deterministic generator, so a failure reproduces from the seed.
struct XorShift(u64);

impl XorShift {
	fn next(&mut self) -> u64 {
		let mut x = self.0;
		x ^= x << 13;
		x ^= x >> 7;
		x ^= x << 17;
		self.0 = x;
		x
	}

	fn below(&mut self, bound: usize) -> usize {
		usize::try_from(self.next() % u64::try_from(bound.max(1)).unwrap())
			.unwrap()
	}
}

/// One structural mutation of `input`, drawing splice material from
/// `others`.
fn mutate(input: &[u8], others: &[Vec<u8>], rng: &mut XorShift) -> Vec<u8> {
	let mut bytes = input.to_vec();
	match rng.below(6) {
		0 if !bytes.is_empty() => {
			let at = rng.below(bytes.len());
			bytes[at] ^= u8::try_from(1 << rng.below(8)).unwrap();
		}
		1 if !bytes.is_empty() => {
			bytes.remove(rng.below(bytes.len()));
		}
		2 => {
			let at = rng.below(bytes.len() + 1);
			const ALPHABET: &[u8] = b"{}[]\":,\\0\x00\xff";
			let byte = ALPHABET[rng.below(ALPHABET.len())];
			bytes.insert(at, byte);
		}
		3 => bytes.truncate(rng.below(bytes.len() + 1)),
		4 if !bytes.is_empty() => {
			let start = rng.below(bytes.len());
			let end = start + rng.below(bytes.len() - start + 1);
			let chunk = bytes[start..end].to_vec();
			let at = rng.below(bytes.len() + 1);
			bytes.splice(at..at, chunk);
		}
		_ => {
			let other = &others[rng.below(others.len())];
			let cut = rng.below(bytes.len() + 1);
			let from = rng.below(other.len() + 1);
			bytes.truncate(cut);
			bytes.extend_from_slice(&other[from..]);
		}
	}
	bytes
}

const HOSTILE_CONTROL: &[&[u8]] = &[
	b"",
	b"null",
	b"true",
	b"0",
	b"\"kind\"",
	b"{}",
	b"[]",
	b"{\"kind\":\"hello\"",
	b"{\"kind\":\"hello\"} trailing",
	b"{\"kind\":\"hello\"}{\"kind\":\"hello\"}",
	b"{\"kind\":\"no_such_kind\"}",
	b"{\"kind\":\"hello\",\"kind\":\"command\"}",
	b"{\"kind\":null}",
	b"{\"kind\":\"hello\",\"major\":-1,\"minor\":-1}",
	b"{\"kind\":\"hello\",\"major\":99999999999999999999,\"minor\":0}",
	b"{\"kind\":\"hello\",\"major\":1.5,\"minor\":0}",
	b"{\"kind\":\"hello\",\"major\":1e999999}",
	b"{\"kind\":\"hello\",\"major\":NaN}",
	b"\xef\xbb\xbf{\"kind\":\"hello\"}",
	b"{\"kind\":\"hel\xfflo\"}",
	b"{\"kind\":\"hello\x00\"}",
	b"{\"kind\":\"hel\nlo\"}",
	b"{\"kind\":\"\\ud800\"}",
	b"{\"kind\":\"event\",\"sequence\":\"007\"}",
	b"{\"kind\":\"event\",\"sequence\":\"+7\"}",
	b"{\"kind\":\"event\",\"sequence\":7}",
];

#[test]
fn every_fixed_hostile_control_envelope_is_refused() {
	let nested_arrays = "[".repeat(100_000).into_bytes();
	let nested_objects = "{\"kind\":".repeat(100_000).into_bytes();
	let long_kind =
		format!("{{\"kind\":\"{}\"}}", "k".repeat(1 << 21)).into_bytes();
	let mut corpus: Vec<&[u8]> = HOSTILE_CONTROL.to_vec();
	corpus.extend([
		nested_arrays.as_slice(),
		nested_objects.as_slice(),
		long_kind.as_slice(),
	]);
	let accepted: Vec<&[u8]> = corpus
		.into_iter()
		.filter(|input| decode_everywhere(input) > 0)
		.collect();
	assert_eq!(accepted, Vec::<&[u8]>::new());
}

#[test]
fn mutated_contract_fixtures_never_panic_a_control_decoder() {
	let fixtures = fixture_payloads();
	let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
	let mut decoded = 0usize;
	for payload in &fixtures {
		for _ in 0..48 {
			let mut mutant = mutate(payload, &fixtures, &mut rng);
			for _ in 0..rng.below(3) {
				mutant = mutate(&mutant, &fixtures, &mut rng);
			}
			decode_everywhere(&mutant);
			decoded += 1;
		}
	}
	assert_eq!(decoded, fixtures.len() * 48);
}

/// Reads one frame from `bytes` through a closed in-memory transport, the
/// way the `frames` fuzz target does.
async fn read_frame(
	bytes: &[u8],
	multiplexed: bool,
) -> Result<Frame, FrameError> {
	let (mut peer, transport) = duplex(bytes.len().max(16));
	peer.write_all(bytes).await.unwrap();
	peer.shutdown().await.unwrap();
	let mut reader = FrameReader::new(transport);
	if multiplexed {
		reader.enable_multiplexing();
	}
	reader.read().await
}

/// The bytes a peer sends for one frame: the kind, the stream identity
/// once multiplexing is negotiated, the big-endian length, the payload.
fn framed(kind: FrameKind, stream_id: Option<u32>, payload: &[u8]) -> Vec<u8> {
	let mut bytes = vec![kind as u8];
	if let Some(stream_id) = stream_id {
		bytes.extend_from_slice(&stream_id.to_be_bytes());
	}
	bytes.extend_from_slice(
		&u32::try_from(payload.len()).unwrap().to_be_bytes(),
	);
	bytes.extend_from_slice(payload);
	bytes
}

#[tokio::test]
async fn hostile_frame_headers_are_refused_before_allocation() {
	let control_limit = FrameLimits::default().control;
	let oversized = [
		vec![FrameKind::Control as u8, 0xff, 0xff, 0xff, 0xff],
		[
			vec![FrameKind::Control as u8],
			u32::try_from(control_limit + 1)
				.unwrap()
				.to_be_bytes()
				.to_vec(),
		]
		.concat(),
	];
	for header in oversized {
		assert!(matches!(
			read_frame(&header, false).await,
			Err(FrameError::Oversized {
				kind: FrameKind::Control,
				..
			})
		));
	}
	assert!(matches!(
		read_frame(&[0x7f, 0, 0, 0, 0], false).await,
		Err(FrameError::UnknownKind(0x7f))
	));
	assert!(matches!(
		read_frame(&framed(FrameKind::Data, Some(0), b""), true).await,
		Err(FrameError::InvalidStream {
			kind: FrameKind::Data,
			..
		})
	));
	assert!(matches!(
		read_frame(&[], false).await,
		Err(FrameError::Closed)
	));
	let truncated =
		[vec![FrameKind::Control as u8, 0, 0, 0, 8], b"{}".to_vec()].concat();
	assert!(matches!(
		read_frame(&truncated, false).await,
		Err(FrameError::Io(_))
	));
	let short_header = [FrameKind::Control as u8, 0, 0];
	assert!(matches!(
		read_frame(&short_header, true).await,
		Err(FrameError::Io(_))
	));
}

#[tokio::test]
async fn mutated_frames_never_panic_a_frame_reader() {
	let mut rng = XorShift(0x2545_F491_4F6C_DD1D);
	let hello: &[u8] = b"{\"kind\":\"hello\"}";
	let originals = [
		framed(FrameKind::Control, None, hello),
		framed(FrameKind::Control, Some(0), hello),
		framed(FrameKind::Data, Some(3), &[7; 64]),
	];
	let mut read = 0usize;
	for original in &originals {
		for _ in 0..256 {
			let mutant = mutate(original, &originals, &mut rng);
			let multiplexed = mutant.first().is_some_and(|byte| byte & 1 == 1);
			let _ = read_frame(&mutant, multiplexed).await;
			let _ = read_frame(&mutant, !multiplexed).await;
			read += 1;
		}
	}
	assert_eq!(read, originals.len() * 256);
}
