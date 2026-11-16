//! Serde types for wire protocol v1 between Tandem and the Node Claude helper: one JSON
//! object per line, every frame carrying `"v":1` and a `"type"`. The helper's end lives in
//! `helper/src/frames.mjs` and `helper/src/host.mjs`; change both ends together.

use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tandem_core::chat::PermissionMode;

pub const PROTOCOL_VERSION: u32 = 1;
/// Upper bound for one whole line, newline included.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenSession {
    pub cwd: String,
    /// Exactly one of `session_id` and `resume` is set.
    pub session_id: Option<String>,
    pub resume: Option<String>,
    pub use_my_setup: bool,
    pub model: String,
    pub effort: Option<String>,
    pub permission_mode: PermissionMode,
    pub system_append: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub text: String,
    pub policy: Policy,
    pub model: String,
    pub effort: Option<String>,
    pub permission_mode: PermissionMode,
}

/// Which tool permissions the helper applies during a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    Discuss,
    Guided,
    Check,
    Takeover,
}

/// Frames Tandem writes to the helper.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outgoing {
    Hello {
        id: u64,
    },
    OpenSession {
        id: u64,
        #[serde(flatten)]
        open: OpenSession,
    },
    Turn {
        id: u64,
        #[serde(flatten)]
        turn: Turn,
    },
    Interrupt {
        id: u64,
    },
    CloseSession {
        id: u64,
    },
    Shutdown {
        id: u64,
    },
    /// Answers a helper `request` with the same id.
    Response {
        id: String,
        #[serde(flatten)]
        body: ResponseBody,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseBody {
    Tool { ok: bool, text: String },
    Permission { allow: bool },
}

/// Frames the helper writes to Tandem.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Incoming {
    Reply {
        id: u64,
        ok: bool,
        #[serde(default)]
        data: Value,
        #[serde(default)]
        error: Option<WireError>,
    },
    Event(Event),
    Request {
        id: String,
        #[serde(flatten)]
        request: HostRequest,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WireError {
    /// One of `bad_request`, `no_session`, `busy`, `auth_refused`, `resume_failed`,
    /// `spawn_failed`, `internal`.
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    TurnStarted {
        turn: u64,
    },
    TextDelta {
        text: String,
    },
    AssistantText {
        text: String,
    },
    /// `kind` is one of `read`, `search`, `web_search`, `web_fetch`, `command`, `edit`, `tool`.
    Activity {
        kind: String,
        label: String,
    },
    Notice {
        text: String,
    },
    AuthRefused {
        reason: String,
        detail: String,
    },
    PermissionCancelled {
        id: String,
    },
    TurnFinished {
        turn: u64,
        status: TurnStatus,
        error: Option<String>,
    },
    SessionClosed {
        reason: String,
    },
    /// Claude Code refused the turn's permission mode; the helper runs the turn in Ask.
    PermissionModeFallback {
        wanted: PermissionMode,
        reason: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Completed,
    Interrupted,
    Error,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum HostRequest {
    Tool {
        name: String,
        input: Value,
    },
    Permission {
        tool: String,
        title: String,
        detail: String,
        command: Option<String>,
        path: Option<String>,
    },
}

/// Reply data for `hello`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct HelloData {
    pub protocol: u32,
    pub helper: String,
    pub sdk: String,
    pub claude_code: String,
    pub node: String,
}

/// Reply data for `open_session`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SessionData {
    pub auth: Auth,
    pub account: Account,
    pub claude_code_version: Option<String>,
    pub models: Vec<ModelChoice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Auth {
    pub ok: bool,
    /// One of `signed_out`, `provider`, `not_subscription` when `ok` is false.
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Account {
    pub subscription: Option<String>,
    pub provider: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ModelChoice {
    pub value: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub efforts: Vec<String>,
}

/// One protocol line: the frame as JSON with `"v":1` first, then a newline.
pub fn encode(frame: &Outgoing) -> Result<Vec<u8>> {
    let body = serde_json::to_vec(frame)?;
    // Every Outgoing variant is a non-empty object that starts with its "type" tag, so
    // the version can be spliced in front of the first key.
    let fields = body
        .strip_prefix(b"{")
        .filter(|rest| rest.first() != Some(&b'}'))
        .ok_or_else(|| anyhow!("an outgoing frame must be a non-empty JSON object"))?;
    let mut line = Vec::with_capacity(body.len() + 8);
    line.extend_from_slice(format!("{{\"v\":{PROTOCOL_VERSION},").as_bytes());
    line.extend_from_slice(fields);
    line.push(b'\n');
    ensure!(
        line.len() <= MAX_FRAME_BYTES,
        "frame for the Claude helper exceeds 8 MiB"
    );
    Ok(line)
}

/// Parses one line from the helper. Error texts never include the payload, which may hold
/// the user's code or conversation.
pub fn decode(line: &[u8]) -> Result<Incoming> {
    let value: Value =
        serde_json::from_slice(line).map_err(|_| anyhow!("frame from the helper is not JSON"))?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("frame from the helper is not a JSON object"))?;
    ensure!(
        object.get("v").and_then(Value::as_u64) == Some(u64::from(PROTOCOL_VERSION)),
        "frame from the helper has an unsupported protocol version"
    );
    serde_json::from_value(value)
        .map_err(|_| anyhow!("frame from the helper does not match protocol v1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_refuses_a_frame_over_the_limit() {
        let frame = Outgoing::Response {
            id: "r1".into(),
            body: ResponseBody::Tool {
                ok: true,
                text: "x".repeat(MAX_FRAME_BYTES),
            },
        };
        assert!(encode(&frame).is_err());
    }

    #[test]
    fn encode_writes_the_version_first() {
        let line = encode(&Outgoing::Response {
            id: "r2".into(),
            body: ResponseBody::Permission { allow: true },
        })
        .unwrap();
        assert_eq!(
            line,
            b"{\"v\":1,\"type\":\"response\",\"id\":\"r2\",\"allow\":true}\n"
        );
    }

    #[test]
    fn decode_errors_never_echo_the_payload() {
        for line in [
            &b"secret-token"[..],
            br#"["secret-token"]"#,
            br#"{"v":1,"type":"reply","id":"secret-token","ok":true}"#,
            br#"{"v":1,"type":"secret-token"}"#,
            br#"{"v":"secret-token","type":"reply","id":1,"ok":true}"#,
        ] {
            let error = format!("{:#}", decode(line).unwrap_err());
            assert!(!error.contains("secret-token"), "{error}");
        }
    }

    #[test]
    fn decode_keeps_unknown_events_readable() {
        let frame = decode(br#"{"v":1,"type":"event","event":"later_feature","x":1}"#).unwrap();
        assert_eq!(frame, Incoming::Event(Event::Unknown));
    }

    #[test]
    fn a_tool_request_keeps_its_input() {
        let frame = decode(
            br#"{"v":1,"type":"request","id":"r1","request":"tool","name":"show_step","input":{"step":2}}"#,
        )
        .unwrap();
        let Incoming::Request {
            id,
            request: HostRequest::Tool { name, input },
        } = frame
        else {
            panic!("not a tool request: {frame:?}");
        };
        assert_eq!((id.as_str(), name.as_str()), ("r1", "show_step"));
        assert_eq!(input["step"], 2);
    }

    #[test]
    fn turn_and_open_session_carry_the_permission_mode() {
        for (mode, wire) in [
            (PermissionMode::Ask, "ask"),
            (PermissionMode::Auto, "auto"),
            (PermissionMode::Bypass, "bypass"),
        ] {
            let turn = Outgoing::Turn {
                id: 3,
                turn: Turn {
                    text: "hi".into(),
                    policy: Policy::Discuss,
                    model: "default".into(),
                    effort: None,
                    permission_mode: mode,
                },
            };
            let open = Outgoing::OpenSession {
                id: 2,
                open: OpenSession {
                    cwd: "/tmp/project".into(),
                    session_id: Some("s".into()),
                    resume: None,
                    use_my_setup: false,
                    model: "default".into(),
                    effort: None,
                    permission_mode: mode,
                    system_append: "x".into(),
                },
            };
            for frame in [turn, open] {
                let line: Value = serde_json::from_slice(&encode(&frame).unwrap()).unwrap();
                assert_eq!(line["permission_mode"], wire, "{line}");
            }
        }
    }

    #[test]
    fn the_permission_mode_fallback_event_decodes() {
        for (wire, wanted) in [
            ("auto", PermissionMode::Auto),
            ("bypass", PermissionMode::Bypass),
        ] {
            let line = format!(
                r#"{{"v":1,"type":"event","event":"permission_mode_fallback","wanted":"{wire}","reason":"x"}}"#
            );
            assert_eq!(
                decode(line.as_bytes()).unwrap(),
                Incoming::Event(Event::PermissionModeFallback {
                    wanted,
                    reason: "x".into(),
                })
            );
        }
    }
}
