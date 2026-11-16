//! The Rust end of wire protocol v1 agrees with the reference conversation the Node helper
//! is tested against (`helper/test/fixtures/contract.jsonl`).

use serde_json::{Value, json};
use tandem_claude::protocol::{self, Event, Incoming, Outgoing};

const CONTRACT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/helper/test/fixtures/contract.jsonl"
));

fn frames(direction: &str) -> Vec<Value> {
    CONTRACT
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("fixture line is JSON"))
        .filter(|entry| entry["dir"] == direction)
        .map(|entry| entry["frame"].clone())
        .collect()
}

#[test]
fn every_helper_frame_decodes_to_a_known_frame() {
    let frames = frames("from_helper");
    assert!(
        frames.len() > 20,
        "the fixture covers the whole conversation"
    );
    for frame in frames {
        let line = serde_json::to_vec(&frame).unwrap();
        let decoded = protocol::decode(&line)
            .unwrap_or_else(|error| panic!("{frame} does not decode: {error}"));
        assert!(
            !matches!(decoded, Incoming::Event(Event::Unknown)),
            "{frame} decodes to an unknown event"
        );
    }
}

#[test]
fn every_tandem_frame_round_trips_through_outgoing() {
    let frames = frames("to_helper");
    assert!(
        frames.len() >= 10,
        "the fixture covers the whole conversation"
    );
    for frame in frames {
        let outgoing: Outgoing = serde_json::from_value(frame.clone())
            .unwrap_or_else(|error| panic!("{frame} is not an Outgoing frame: {error}"));
        let mut expected = frame.clone();
        expected.as_object_mut().unwrap().remove("v");
        assert_eq!(serde_json::to_value(&outgoing).unwrap(), expected);
    }
}

#[test]
fn helper_frames_carry_the_expected_values() {
    let decoded: Vec<Incoming> = frames("from_helper")
        .iter()
        .map(|frame| protocol::decode(&serde_json::to_vec(frame).unwrap()).unwrap())
        .collect();
    let Incoming::Reply {
        id: 2,
        ok: true,
        data,
        ..
    } = &decoded[1]
    else {
        panic!("second frame is the open_session reply");
    };
    let session: protocol::SessionData = serde_json::from_value(data.clone()).unwrap();
    assert!(session.auth.ok);
    assert_eq!(session.account.provider.as_deref(), Some("firstParty"));
    assert_eq!(session.models[1].name, "Haiku");
    assert!(session.models[1].efforts.is_empty());
    assert!(decoded.iter().any(|frame| matches!(
        frame,
        Incoming::Request { id, request: protocol::HostRequest::Permission { command: Some(command), path: None, .. } }
            if id == "r2" && command == "npm install left-pad"
    )));
    assert!(decoded.iter().any(|frame| matches!(
        frame,
        Incoming::Event(Event::TurnFinished {
            turn: 5,
            status: protocol::TurnStatus::Interrupted,
            error: None
        })
    )));
}

#[test]
fn encode_adds_the_version_and_a_newline() {
    let bytes = protocol::encode(&Outgoing::Hello { id: 1 }).unwrap();
    assert_eq!(bytes.last(), Some(&b'\n'));
    assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 1);
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value, json!({"v": 1, "type": "hello", "id": 1}));
}

#[test]
fn decode_rejects_another_protocol_version() {
    let line = br#"{"v":2,"type":"reply","id":1,"ok":true,"data":{}}"#;
    assert!(protocol::decode(line).is_err());
}
