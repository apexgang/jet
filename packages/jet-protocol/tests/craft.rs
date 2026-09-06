//! Craft wire behavior through the public codec.
use jet_protocol::{
	CraftCommand, CraftEvent, Presentation, decode_control, encode_control,
};
use pretty_assertions::assert_eq;

#[test]
fn native_events_and_unknown_presentation_survive_the_wire() {
	let bytes = br#"{"kind":"output","native_event": { "vendor": "delta", "huge": 9007199254740993, "extra": [true,null] },"presentation":[{"kind":"text","text":"Hello","future":true},{"kind":"vendor_chart","points":[1,2],"extra":{"x":3}}],"future":1}"#;
	let event: CraftEvent = decode_control(bytes).unwrap();
	let CraftEvent::Output {
		native_event,
		presentation,
	} = &event
	else {
		panic!("output")
	};
	assert_eq!(
		native_event.get(),
		r#"{ "vendor": "delta", "huge": 9007199254740993, "extra": [true,null] }"#
	);
	assert_eq!(
		presentation[0].known().unwrap(),
		Some(Presentation::Text {
			text: "Hello".into()
		})
	);
	assert_eq!(presentation[1].known().unwrap(), None);
	let decoded: CraftEvent =
		decode_control(&encode_control(&event).unwrap()).unwrap();
	assert_eq!(
		encode_control(&decoded).unwrap(),
		encode_control(&event).unwrap()
	);
	assert_eq!(
		presentation[1].raw().get(),
		r#"{"kind":"vendor_chart","points":[1,2],"extra":{"x":3}}"#
	);
}

#[test]
fn commands_and_sensitive_variants_fail_closed() {
	for invalid in [
		r#"{"kind":"future_command"}"#,
		r#"{"kind":"action","id":"1","action":{"kind":"approval","request_id":"r","decision":"always_allow"}}"#,
		r#"{"kind":"action","id":"1","action":{"kind":"future_action"}}"#,
	] {
		assert!(decode_control::<CraftCommand>(invalid.as_bytes()).is_err());
	}
	assert!(
		decode_control::<CraftEvent>(br#"{"kind":"future_event"}"#).is_err()
	);
	assert!(decode_control::<CraftEvent>(br#"{"kind":"output","native_event":{},"presentation":[{"kind":"text","text":3}]}"#).is_err());
}
