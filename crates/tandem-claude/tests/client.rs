//! `HelperClient` against a real child process: the scripted fake helper, or `/bin/sh`.

mod support;

use serde_json::json;
use std::ffi::OsString;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use support::{collect, fake_launch, write_script};
use tandem_claude::{
    Event, HelloData, Incoming, Launch, MAX_FRAME_BYTES, Outgoing, Received, ResponseBody,
    TurnStatus,
};

const WAIT: Duration = Duration::from_secs(10);

fn next(receiver: &Receiver<Received>) -> Received {
    receiver
        .recv_timeout(WAIT)
        .expect("the helper client delivers something")
}

fn next_frame(receiver: &Receiver<Received>) -> Incoming {
    match next(receiver) {
        Received::Frame(frame) => frame,
        Received::Closed { error, stderr_tail } => {
            panic!("closed early ({error:?}); stderr: {stderr_tail}")
        }
    }
}

fn closed(receiver: &Receiver<Received>) -> (Option<String>, String) {
    match next(receiver) {
        Received::Closed { error, stderr_tail } => (error, stderr_tail),
        Received::Frame(frame) => panic!("expected the end of the stream, got {frame:?}"),
    }
}

fn hello_reply() -> serde_json::Value {
    json!({"send": {"type": "reply", "id": "$id", "ok": true, "data": {
        "protocol": 1, "helper": "1.0.0", "sdk": "0.3.325", "claude_code": "2.1.325", "node": "v24.22.1"
    }}})
}

#[test]
fn hello_round_trip_with_increasing_ids() {
    let temp = tempfile::tempdir().unwrap();
    let script = write_script(
        temp.path(),
        &[
            json!({"expect": {"type": "hello", "id": 1}}),
            hello_reply(),
            json!({"expect": {"type": "hello", "id": 2}}),
            hello_reply(),
        ],
    );
    let (client, receiver) = collect(&fake_launch(&script));

    assert_eq!(client.command(|id| Outgoing::Hello { id }).unwrap(), 1);
    let Incoming::Reply {
        id,
        ok,
        data,
        error,
    } = next_frame(&receiver)
    else {
        panic!("expected a reply");
    };
    assert_eq!((id, ok, error), (1, true, None));
    let hello: HelloData = serde_json::from_value(data).unwrap();
    assert_eq!(hello.protocol, 1);
    assert_eq!(hello.claude_code, "2.1.325");

    assert_eq!(client.command(|id| Outgoing::Hello { id }).unwrap(), 2);
    assert!(matches!(
        next_frame(&receiver),
        Incoming::Reply {
            id: 2,
            ok: true,
            ..
        }
    ));
}

#[test]
fn events_arrive_in_the_order_sent() {
    let temp = tempfile::tempdir().unwrap();
    let script = write_script(
        temp.path(),
        &[
            json!({"send": {"type": "event", "event": "turn_started", "turn": 3}}),
            json!({"send": {"type": "event", "event": "text_delta", "text": "Here"}}),
            json!({"send": {"type": "event", "event": "activity", "kind": "read", "label": "src/cli.rs"}}),
            json!({"send": {"type": "event", "event": "assistant_text", "text": "Here is the plan."}}),
            json!({"send": {"type": "event", "event": "turn_finished", "turn": 3, "status": "completed", "error": null}}),
        ],
    );
    let (_client, receiver) = collect(&fake_launch(&script));

    let events: Vec<Incoming> = (0..5).map(|_| next_frame(&receiver)).collect();
    assert_eq!(
        events,
        [
            Incoming::Event(Event::TurnStarted { turn: 3 }),
            Incoming::Event(Event::TextDelta {
                text: "Here".into()
            }),
            Incoming::Event(Event::Activity {
                kind: "read".into(),
                label: "src/cli.rs".into()
            }),
            Incoming::Event(Event::AssistantText {
                text: "Here is the plan.".into()
            }),
            Incoming::Event(Event::TurnFinished {
                turn: 3,
                status: TurnStatus::Completed,
                error: None
            }),
        ]
    );
}

#[test]
fn respond_answers_a_helper_request() {
    let temp = tempfile::tempdir().unwrap();
    let script = write_script(
        temp.path(),
        &[
            json!({"send": {"type": "request", "id": "r1", "request": "tool", "name": "propose_plan", "input": {"goal": "g"}}}),
            json!({"expect": {"type": "response", "id": "r1", "ok": true, "text": "shown"}}),
            json!({"send": {"type": "request", "id": "r2", "request": "permission", "tool": "Bash",
                "title": "t", "detail": "", "command": "npm install left-pad", "path": null}}),
            json!({"expect": {"type": "response", "id": "r2", "allow": false}}),
            json!({"send": {"type": "event", "event": "notice", "text": "answered"}}),
        ],
    );
    let (client, receiver) = collect(&fake_launch(&script));

    assert!(matches!(next_frame(&receiver), Incoming::Request { id, .. } if id == "r1"));
    client
        .respond(
            "r1",
            ResponseBody::Tool {
                ok: true,
                text: "shown".into(),
            },
        )
        .unwrap();
    assert!(matches!(next_frame(&receiver), Incoming::Request { id, .. } if id == "r2"));
    client
        .respond("r2", ResponseBody::Permission { allow: false })
        .unwrap();
    assert_eq!(
        next_frame(&receiver),
        Incoming::Event(Event::Notice {
            text: "answered".into()
        })
    );
}

#[test]
fn a_line_that_is_not_json_closes_with_the_invalid_frame_error() {
    let temp = tempfile::tempdir().unwrap();
    let script = write_script(
        temp.path(),
        &[
            json!({"send": {"type": "event", "event": "notice", "text": "before"}}),
            json!({"send_raw": "not json\n"}),
            json!({"send": {"type": "event", "event": "notice", "text": "after"}}),
        ],
    );
    let (_client, receiver) = collect(&fake_launch(&script));

    assert!(matches!(
        next_frame(&receiver),
        Incoming::Event(Event::Notice { .. })
    ));
    let (error, _) = closed(&receiver);
    assert_eq!(
        error.as_deref(),
        Some("invalid frame from the Claude helper")
    );
    assert!(
        receiver.recv_timeout(Duration::from_millis(300)).is_err(),
        "nothing is delivered after Closed"
    );
}

#[test]
fn a_line_over_eight_mebibytes_closes_with_the_oversize_error() {
    let temp = tempfile::tempdir().unwrap();
    let huge = format!("{}\n", "x".repeat(MAX_FRAME_BYTES + 1));
    let script = write_script(temp.path(), &[json!({"send_raw": huge})]);
    let (_client, receiver) = collect(&fake_launch(&script));

    let (error, _) = closed(&receiver);
    assert_eq!(
        error.as_deref(),
        Some("frame from the Claude helper exceeds 8 MiB")
    );
}

#[test]
fn an_exit_after_writing_to_stderr_closes_cleanly_with_the_stderr_tail() {
    let launch = Launch::custom(
        "/bin/sh".into(),
        vec![
            OsString::from("-c"),
            OsString::from("echo 'helper crashed: boom' >&2; exit 7"),
        ],
    );
    let (_client, receiver) = collect(&launch);

    let (error, stderr_tail) = closed(&receiver);
    assert_eq!(error, None);
    assert!(
        stderr_tail.contains("helper crashed: boom"),
        "stderr tail: {stderr_tail:?}"
    );
}

#[test]
fn shutdown_ends_a_helper_that_hangs() {
    let temp = tempfile::tempdir().unwrap();
    let script = write_script(temp.path(), &[json!({"hang": true})]);
    let (mut client, _receiver) = collect(&fake_launch(&script));
    let pid = client.pid() as libc::pid_t;
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "the helper is running");

    let started = Instant::now();
    client.shutdown();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "the helper is gone");

    client.shutdown();
    assert!(client.command(|id| Outgoing::Hello { id }).is_err());
}

#[test]
fn the_helper_environment_has_no_credentials_or_provider_switches() {
    let temp = tempfile::tempdir().unwrap();
    let script = write_script(
        temp.path(),
        &[
            json!({"send_env": "ANTHROPIC_API_KEY"}),
            json!({"send_env": "ANTHROPIC_AUTH_TOKEN"}),
            json!({"send_env": "CLAUDE_CODE_USE_BEDROCK"}),
            json!({"send_env": "KEEP_ME"}),
        ],
    );
    let mut launch = fake_launch(&script);
    for (name, value) in [
        ("ANTHROPIC_API_KEY", "x"),
        ("ANTHROPIC_AUTH_TOKEN", "y"),
        ("CLAUDE_CODE_USE_BEDROCK", "1"),
        ("KEEP_ME", "z"),
    ] {
        launch.env.push((name.into(), value.into()));
    }
    let (_client, receiver) = collect(&launch);

    let notices: Vec<String> = (0..4)
        .map(|_| match next_frame(&receiver) {
            Incoming::Event(Event::Notice { text }) => text,
            other => panic!("expected a notice, got {other:?}"),
        })
        .collect();
    assert_eq!(notices, ["<unset>", "<unset>", "<unset>", "z"]);
}

#[test]
fn a_failed_expectation_is_reported_on_stderr() {
    let temp = tempfile::tempdir().unwrap();
    let script = write_script(
        temp.path(),
        &[json!({"expect": {"type": "hello", "id": 9}})],
    );
    let (client, receiver) = collect(&fake_launch(&script));

    client.command(|id| Outgoing::Hello { id }).unwrap();
    let (error, stderr_tail) = closed(&receiver);
    assert_eq!(error, None);
    assert!(
        stderr_tail.contains("fake helper: expected {")
            && stderr_tail.contains(r#"got {"v":1,"type":"hello","id":1}"#),
        "stderr tail: {stderr_tail:?}"
    );
}
