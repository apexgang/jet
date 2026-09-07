use pretty_assertions::assert_eq;

use super::git_object;

#[test]
fn git_object_accepts_sha1_and_sha256_names() {
	assert_eq!(
		[
			"a".repeat(40),
			"b".repeat(64),
			"c".repeat(39),
			"d".repeat(65),
			"A".repeat(40),
			"g".repeat(40),
		]
		.map(|object| git_object(&object)),
		[true, true, false, false, false, false]
	);
}
