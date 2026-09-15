//! What free text may reach the Diagnostic log, and in what shape.
//!
//! The record shape already keeps most content out: a message is a static
//! template and structured fields carry only counts, identities, and codes.
//! The one opening is failure text, because an operating-system or native
//! error explains itself in prose. That prose is scrubbed here before it
//! is kept, on the assumption that it may quote anything at all: a
//! credential in a header, an authentication callback with its code, a
//! prompt body or terminal output quoted by a Harness, or a raw native
//! payload. The scrub is structural, not a list of known secrets, so it
//! errs toward dropping (ADR-0061, ADR-0076).

/// How much failure text one record may keep, in bytes.
pub(super) const FAILURE_LIMIT: usize = 256;

/// How much of a stable code one record may keep, in bytes.
const CODE_LIMIT: usize = 96;

/// A quoted span at least this long is treated as quoted content, not
/// as a short quoted name.
const QUOTED_CONTENT: usize = 32;

/// An opaque word at least this long is treated as key material.
const OPAQUE_WORD: usize = 20;

const REDACTED: &str = "[redacted]";
const PAYLOAD_REDACTED: &str = "[payload redacted]";
const TRUNCATED: &str = "[truncated]";

/// Words that, before `=` or `:`, name a value nobody should read back.
const SENSITIVE_KEYS: &[&str] = &[
	"token",
	"secret",
	"password",
	"passwd",
	"pwd",
	"key",
	"auth",
	"bearer",
	"cookie",
	"credential",
	"session",
	"signature",
	"otp",
];

/// Words after which the next word is a credential.
const CREDENTIAL_SCHEMES: &[&str] = &["bearer", "basic", "token"];

/// Reduces failure prose to a single bounded line with nothing worth
/// stealing left in it.
pub(super) fn sanitize_failure(text: &str) -> String {
	let flat = flatten(text);
	let without_payload = cut_payload(&flat);
	let without_quotes = redact_quoted(&without_payload);
	let scrubbed = scrub_words(&without_quotes);
	truncate(&scrubbed, FAILURE_LIMIT)
}

/// Keeps a stable code as-is when it looks like one, so that a Harness or
/// client cannot smuggle prose through the code position.
pub(super) fn sanitize_code(code: &str) -> String {
	let mut kept: String = code
		.chars()
		.filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
		.take(CODE_LIMIT)
		.collect();
	if kept.is_empty() {
		kept.push_str(REDACTED);
	}
	kept
}

/// Control characters, including line breaks, become single spaces, so
/// multi-line output cannot spread across records or hide behind the
/// bound.
fn flatten(text: &str) -> String {
	let mut flat = String::with_capacity(text.len().min(FAILURE_LIMIT * 4));
	let mut pending_space = false;
	for c in text.chars() {
		if c.is_control() || c.is_whitespace() {
			pending_space = !flat.is_empty();
		} else {
			if pending_space {
				flat.push(' ');
				pending_space = false;
			}
			flat.push(c);
		}
		if flat.len() > FAILURE_LIMIT * 4 {
			break;
		}
	}
	flat
}

/// A brace or bracket begins a structured payload, and native envelopes
/// are what carry prompts and outputs. Everything from the first one on
/// is dropped.
fn cut_payload(text: &str) -> String {
	match text.find(['{', '[']) {
		Some(0) => PAYLOAD_REDACTED.to_owned(),
		Some(at) => format!("{} {PAYLOAD_REDACTED}", text[..at].trim_end()),
		None => text.to_owned(),
	}
}

/// Long quoted spans are content somebody wrote, not a name. Apostrophes
/// are left alone, because prose is full of them.
fn redact_quoted(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	let mut rest = text;
	while let Some(open) = rest.find(['"', '`']) {
		let quote = rest.as_bytes()[open] as char;
		out.push_str(&rest[..=open]);
		rest = &rest[open + 1..];
		let Some(close) = rest.find(quote) else {
			// An unterminated quote runs to the end; keep nothing of it.
			out.push_str(REDACTED);
			return out;
		};
		let quoted = &rest[..close];
		if quoted.len() >= QUOTED_CONTENT {
			out.push_str(REDACTED);
		} else {
			out.push_str(quoted);
		}
		out.push(quote);
		rest = &rest[close + 1..];
	}
	out.push_str(rest);
	out
}

fn scrub_words(text: &str) -> String {
	let mut out = Vec::new();
	let mut previous = String::new();
	for word in text.split(' ') {
		let scrubbed = scrub_word(word, &previous);
		previous = word.to_ascii_lowercase();
		out.push(scrubbed);
	}
	out.join(" ")
}

fn scrub_word(word: &str, previous: &str) -> String {
	if CREDENTIAL_SCHEMES.contains(&previous) {
		return REDACTED.to_owned();
	}
	if let Some(at) = sensitive_key(previous)
		&& at + 1 == previous.len()
		&& !CREDENTIAL_SCHEMES.contains(&word.to_ascii_lowercase().as_str())
	{
		// `password: hunter2`: the value follows its key as its own word.
		return REDACTED.to_owned();
	}
	if let Some(url) = scrub_url(word) {
		return url;
	}
	if let Some(at) = sensitive_key(word)
		&& at + 1 < word.len()
	{
		return format!("{}{REDACTED}", &word[..=at]);
	}
	if looks_opaque(word) {
		return REDACTED.to_owned();
	}
	word.to_owned()
}

/// Where a `key=` or `key:` prefix naming a value nobody should read
/// back ends, when the word has one.
fn sensitive_key(word: &str) -> Option<usize> {
	let at = word.find(['=', ':'])?;
	let key = word[..at].to_ascii_lowercase();
	(at > 0 && SENSITIVE_KEYS.iter().any(|name| key.contains(name)))
		.then_some(at)
}

/// Keeps a URL's scheme, host and path, dropping user information, query
/// and fragment: that is where callbacks carry codes and tokens. A path
/// segment that looks like key material goes the same way.
fn scrub_url(word: &str) -> Option<String> {
	let scheme_end = word.find("://")?;
	let rest = &word[scheme_end + 3..];
	let authority_end = rest.find('/').unwrap_or(rest.len());
	let authority = &rest[..authority_end];
	let host = authority
		.rsplit_once('@')
		.map_or(authority, |(_, host)| host);
	let path_and_more = &rest[authority_end..];
	let path_end = path_and_more
		.find(['?', '#'])
		.unwrap_or(path_and_more.len());
	let path = path_and_more[..path_end]
		.split('/')
		.map(|segment| {
			if looks_opaque(segment) {
				REDACTED
			} else {
				segment
			}
		})
		.collect::<Vec<_>>()
		.join("/");
	let suffix = if path_end < path_and_more.len() {
		format!("?{REDACTED}")
	} else {
		String::new()
	};
	Some(format!("{}://{host}{path}{suffix}", &word[..scheme_end]))
}

/// Base64-, hex-, URL-safe and dotted words of key length with mixed
/// letters and digits, apart from the UUIDs that identify things in this
/// log. Paths with digits in them are lost this way; that is the side to
/// err on.
fn looks_opaque(word: &str) -> bool {
	let trimmed = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
	if trimmed.len() < OPAQUE_WORD || is_uuid(trimmed) {
		return false;
	}
	let charset = trimmed.bytes().all(|b| {
		b.is_ascii_alphanumeric()
			|| matches!(b, b'+' | b'/' | b'=' | b'_' | b'-' | b'.')
	});
	let letters = trimmed.bytes().any(|b| b.is_ascii_alphabetic());
	let digits = trimmed.bytes().any(|b| b.is_ascii_digit());
	charset && letters && digits
}

fn is_uuid(word: &str) -> bool {
	word.len() == 36
		&& word.bytes().enumerate().all(|(index, b)| match index {
			8 | 13 | 18 | 23 => b == b'-',
			_ => b.is_ascii_hexdigit(),
		})
}

fn truncate(text: &str, limit: usize) -> String {
	if text.len() <= limit {
		return text.to_owned();
	}
	let mut end = limit;
	while !text.is_char_boundary(end) {
		end -= 1;
	}
	format!("{}{TRUNCATED}", text[..end].trim_end())
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	#[test]
	fn plain_failures_pass_through() {
		assert_eq!(
			sanitize_failure("store.unavailable: the Plane store is locked"),
			"store.unavailable: the Plane store is locked"
		);
	}

	#[test]
	fn credentials_are_redacted_by_shape() {
		let cases = [
			(
				"request failed: Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.abc",
				"request failed: Authorization: Bearer [redacted]",
			),
			(
				"bad api_key=sk-live-0123456789abcdef",
				"bad api_key=[redacted]",
			),
			(
				"refused password: hunter2 for user jet",
				"refused password: [redacted] for user jet",
			),
			(
				"leaked AKIAIOSFODNN7EXAMPLE0123 in reply",
				"leaked [redacted] in reply",
			),
			(
				"Run 018f6f5e-0f2a-7c3b-8e1d-5a6b7c8d9e0f failed",
				"Run 018f6f5e-0f2a-7c3b-8e1d-5a6b7c8d9e0f failed",
			),
			(
				"rejected eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2ln outright",
				"rejected [redacted] outright",
			),
		];
		for (input, expected) in cases {
			assert_eq!(sanitize_failure(input), expected, "{input}");
		}
	}

	#[test]
	fn authentication_callbacks_keep_only_their_route() {
		assert_eq!(
			sanitize_failure(
				"callback https://user:pw@auth.example/cb?code=abc123&state=xyz#frag rejected"
			),
			"callback https://auth.example/cb?[redacted] rejected"
		);
		assert_eq!(
			sanitize_failure(
				"reset https://auth.example/reset/abcdef0123456789abcdef/confirm"
			),
			"reset https://auth.example/reset/[redacted]/confirm"
		);
	}

	#[test]
	fn native_payloads_and_quoted_content_are_dropped() {
		let prompt = "x".repeat(40);
		assert_eq!(
			sanitize_failure(&format!(
				"Craft rejected \"{prompt}\" and {{\"prompt\":\"{prompt}\"}}"
			)),
			"Craft rejected \"[redacted]\" and [payload redacted]"
		);
		assert_eq!(
			sanitize_failure("cannot open \"notes.md\""),
			"cannot open \"notes.md\""
		);
		assert_eq!(
			sanitize_failure(&format!("Harness said `{prompt}` and [1, 2]")),
			"Harness said `[redacted]` and [payload redacted]"
		);
		assert_eq!(
			sanitize_failure("can't open the user's file"),
			"can't open the user's file"
		);
		assert_eq!(
			sanitize_failure("unterminated \"prompt text goes on"),
			"unterminated \"[redacted]"
		);
	}

	#[test]
	fn terminal_output_is_flattened_and_bounded() {
		let output = "line one\r\nline two\tend\n".repeat(100);
		let kept = sanitize_failure(&output);
		assert!(kept.len() <= FAILURE_LIMIT + TRUNCATED.len());
		assert!(!kept.contains('\n'));
		assert!(kept.ends_with(TRUNCATED));
		assert!(kept.starts_with("line one line two end"));
	}

	#[test]
	fn large_payloads_are_bounded_without_reading_them_whole() {
		let huge = "a".repeat(64 * 1024 * 1024);
		let kept = sanitize_failure(&huge);
		assert_eq!(kept.len(), FAILURE_LIMIT + TRUNCATED.len());
	}

	#[test]
	fn codes_keep_only_identifier_characters() {
		assert_eq!(sanitize_code("store.unavailable"), "store.unavailable");
		assert_eq!(sanitize_code("a b\n{c}"), "abc");
		assert_eq!(sanitize_code(" {}"), "[redacted]");
	}
}
