//! Regression test: oneof variants must serialize to JSON using the
//! proto field name (snake_case), not serde's default Rust variant
//! identifier (PascalCase).
//!
//! The Desktop renderer bridges Core events over Tauri as JSON and keys
//! on the proto field name — e.g. `event.body.session_io`. If the
//! `rename_all = "snake_case"` attribute in `crates/proto/build.rs` is
//! dropped, `Event.body` serializes as `{"SessionIo": …}` and the
//! renderer silently never matches, so session terminal output and live
//! workspace/workarea events never reach the UI.

use concerto_proto::v1::{event::Body, Event, SessionIoChunk};

#[test]
fn event_body_oneof_serializes_with_snake_case_field_name() {
    let ev = Event {
        offset: 1,
        at: None,
        body: Some(Body::SessionIo(SessionIoChunk {
            session_id: "sid".into(),
            stream: "stdout".into(),
            data: vec![104, 105],
        })),
        // Task 316: additive non-oneof carrier (unset here).
        checks_opaque: None,
    };
    let json = serde_json::to_string(&ev).unwrap();
    assert!(
        json.contains("\"session_io\""),
        "expected snake_case oneof key, got: {json}"
    );
    assert!(
        !json.contains("\"SessionIo\""),
        "found PascalCase oneof key (rename_all missing?), got: {json}"
    );
}
