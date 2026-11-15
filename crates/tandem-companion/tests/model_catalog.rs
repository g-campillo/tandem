use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::Duration,
};
use tandem_codex::ProtocolExchange;
use tandem_companion::{CompanionSession, ConnectionKind, resolve_model};
use tandem_core::*;

fn model(name: &str, default: bool, effort: &str) -> Value {
    json!({"id":format!("entry-{name}"),"model":name,"displayName":format!("Display {name}"),"description":"Text model","hidden":false,"isDefault":default,"inputModalities":["text"],"supportedReasoningEfforts":[{"reasoningEffort":effort,"description":"Default"}],"defaultReasoningEffort":effort})
}
#[derive(Default)]
struct Peer {
    pages: VecDeque<Value>,
    requests: Vec<(String, Value)>,
    events: VecDeque<Value>,
    account: Option<String>,
    availability_lost: bool,
    substituted_model: Option<String>,
    reroute: bool,
    threads: usize,
    turns: usize,
    shutdown: bool,
}
#[derive(Clone)]
struct Fake(Arc<Mutex<Peer>>);
impl ProtocolExchange for Fake {
    fn request(&mut self, method: &str, params: Value, _: Duration) -> Result<Value> {
        let mut p = self.0.lock().unwrap();
        p.requests.push((method.into(), params.clone()));
        if p.availability_lost && method == "account/read" {
            bail!("app-server protocol closed");
        }
        Ok(match method {
            "account/read" => {
                json!({"account":{"type":p.account.as_deref().unwrap_or("chatgpt"),"planType":"plus"}})
            }
            "account/rateLimits/read" => json!({"ordinaryUsageAllowed":true}),
            "model/list" => p.pages.pop_front().expect("unexpected catalog refresh"),
            "thread/start" | "thread/resume" => {
                let id = if method == "thread/start" {
                    p.threads += 1;
                    format!("thread-{}", p.threads)
                } else {
                    params["threadId"].as_str().unwrap().into()
                };
                json!({"thread":{"id":id},"model":p.substituted_model.as_deref().unwrap_or(params["model"].as_str().unwrap()),"modelProvider":"openai","reasoningEffort":params.pointer("/config/model_reasoning_effort"),"instructionSources":[]})
            }
            "turn/start" => {
                p.turns += 1;
                let id = format!("turn-{}", p.turns);
                let thread = params["threadId"].clone();
                if p.reroute {
                    p.events.push_back(json!({"method":"model/rerouted","params":{"threadId":thread,"turnId":id,"fromModel":params["model"],"toModel":"substitute","reason":"highRiskCyberActivity"}}));
                }
                p.events.extend([
                    json!({"method":"turn/started","params":{"threadId":thread,"turnId":id}}),
                    json!({"method":"item/completed","params":{"threadId":thread,"turnId":id,"item":{"type":"agentMessage","text":json!({"summary":"Saved explanation","sections":[],"next_action":"Continue","requested_context":[]}).to_string()}}}),
                    json!({"method":"turn/completed","params":{"threadId":thread,"turnId":id,"turn":{"id":id,"status":"completed"}}}),
                ]);
                json!({"turn":{"id":id}})
            }
            "turn/interrupt" => json!({}),
            _ => bail!("unexpected method {method}"),
        })
    }
    fn poll_event(&mut self, _: Duration) -> Result<Option<Value>> {
        Ok(self.0.lock().unwrap().events.pop_front())
    }
    fn shutdown(&mut self) -> Result<()> {
        self.0.lock().unwrap().shutdown = true;
        Ok(())
    }
}
fn session(pages: Vec<Value>) -> (CompanionSession<Fake>, Arc<Mutex<Peer>>) {
    let peer = Arc::new(Mutex::new(Peer {
        pages: pages.into(),
        ..Default::default()
    }));
    (
        CompanionSession::with_exchange(Fake(peer.clone()), PathBuf::from("/test-scratch")),
        peer,
    )
}
fn page(models: Vec<Value>) -> Value {
    json!({"data":models,"nextCursor":null})
}
fn request() -> ModelRequest {
    ModelRequest {
        model_preference: ModelPreference::Automatic,
        resolved_model: None,
        conversation_id: Some("durable-session".into()),
        action: CompanionAction::Ask,
        question: "Explain this decision".into(),
        language: "rust".into(),
        guidance: GuidanceSettings::default().effective("project", "rust", None),
        task: None,
        context: ContextSnapshot {
            id: "captured".into(),
            project_fingerprint: "source".into(),
            files: vec![],
            tree: vec![],
            omitted: vec![],
            captured_at_ms: 0,
            validation_environments: Default::default(),
        },
        evidence: vec![],
        prior_decisions: vec!["Keep the transcript".into()],
        delegation: None,
    }
}
fn run(
    s: &mut CompanionSession<Fake>,
    request: ModelRequest,
    binding: Option<ThreadBinding>,
) -> (ModelRun, Option<ThreadBinding>) {
    s.run(
        ModelRun::pending("project".into(), request),
        binding,
        &AtomicBool::new(false),
        |_| Ok(()),
    )
    .unwrap()
}
#[test]
fn paginated_catalog_uses_live_identity_and_advertised_effort() {
    let (mut s, p) = session(vec![
        json!({"data":[model("a",true,"high")],"nextCursor":"page-two"}),
        page(vec![model("b", false, "low")]),
    ]);
    let catalog = s.list_models().unwrap();
    let resolved = resolve_model(&catalog, &ModelPreference::Automatic).unwrap();
    assert_eq!(resolved.model, "a");
    assert_eq!(resolved.catalog_id, "entry-a");
    assert_eq!(resolved.effort.as_deref(), Some("high"));
    assert_eq!(
        resolve_model(&catalog, &ModelPreference::Explicit { model: "b".into() })
            .unwrap()
            .effort
            .as_deref(),
        Some("low")
    );
    let p = p.lock().unwrap();
    let lists = p
        .requests
        .iter()
        .filter(|(m, _)| m == "model/list")
        .collect::<Vec<_>>();
    assert_eq!(lists.len(), 2);
    assert_eq!(lists[0].1["includeHidden"], false);
    assert_eq!(lists[0].1["limit"], 100);
    assert_eq!(lists[1].1["cursor"], "page-two");
}
#[test]
fn catalogs_reject_empty_malformed_ambiguous_and_unsupported_defaults() {
    let mut unsupported = model("a", true, "high");
    unsupported["defaultReasoningEffort"] = json!("unsupported");
    let mut nontext = model("a", true, "high");
    nontext["inputModalities"] = json!(["image"]);
    let mut invalid_id = model("a", true, "high");
    invalid_id["model"] = json!("bad model\nprivate");
    for models in [
        vec![],
        vec![json!({"model":"a"})],
        vec![model("a", false, "high")],
        vec![model("a", true, "high"), model("b", true, "low")],
        vec![unsupported],
        vec![nontext],
        vec![invalid_id],
    ] {
        let (mut s, _) = session(vec![page(models)]);
        assert!(s.list_models().is_err());
    }
}
#[test]
fn pagination_rejects_duplicates_loops_and_excess_pages() {
    for pages in [
        vec![json!({"data":[model("a",true,"high")],"nextCursor":"again"}),page(vec![model("a",true,"high")])],
        vec![json!({"data":[model("a",true,"high")],"nextCursor":"again"}),json!({"data":[model("b",false,"low")],"nextCursor":"again"})],
        (0..10).map(|i|json!({"data":[model(&format!("a-{i}"),i==0,"high")],"nextCursor":format!("cursor-{i}")})).collect(),
    ] {let (mut s,p)=session(pages);assert!(s.list_models().is_err());assert!(p.lock().unwrap().requests.iter().filter(|(m,_)|m=="model/list").count()<=10);}
}
#[test]
fn missing_explicit_choice_blocks_without_a_turn_or_preference_change() {
    let (mut s, p) = session(vec![page(vec![model("a", true, "high")])]);
    let mut req = request();
    req.model_preference = ModelPreference::Explicit {
        model: "missing".into(),
    };
    let expected = req.model_preference.clone();
    let (r, b) = run(&mut s, req, None);
    assert_eq!(r.status, ModelStatus::Failed);
    assert!(r.detail.contains("unavailable"));
    assert_eq!(r.request.model_preference, expected);
    assert!(b.is_none());
    assert_eq!(p.lock().unwrap().turns, 0);
}
#[test]
fn account_must_be_ready_before_catalog_or_model_work() {
    let (mut s, p) = session(vec![]);
    p.lock().unwrap().account = Some("apiKey".into());
    assert!(s.list_models().is_err());
    assert_eq!(s.connection.kind, ConnectionKind::Unsupported);
    assert!(
        !p.lock()
            .unwrap()
            .requests
            .iter()
            .any(|(m, _)| m == "model/list")
    );
}
#[test]
fn exact_start_resume_and_turn_fields_and_snapshot_reuse() {
    let (mut s, p) = session(vec![page(vec![model("a", true, "high")])]);
    let (first, binding) = run(&mut s, request(), None);
    assert_eq!(first.status, ModelStatus::Completed);
    let first_binding = binding.clone().unwrap();
    let mut next = first.request.clone();
    next.prior_decisions.push("Second local decision".into());
    let (second, _) = run(&mut s, next.clone(), binding);
    assert_eq!(second.thread_id, first.thread_id);
    let peer = Arc::new(Mutex::new(Peer::default()));
    let mut restarted =
        CompanionSession::with_exchange(Fake(peer.clone()), PathBuf::from("/test-scratch"));
    let (resumed, _) = run(&mut restarted, next, Some(first_binding));
    assert_eq!(resumed.status, ModelStatus::Completed);
    let p = p.lock().unwrap();
    let start = &p
        .requests
        .iter()
        .find(|(m, _)| m == "thread/start")
        .unwrap()
        .1;
    assert_eq!(start["model"], "a");
    assert_eq!(start["allowProviderModelFallback"], false);
    assert_eq!(
        start.pointer("/config/model_reasoning_effort"),
        Some(&json!("high"))
    );
    assert_eq!(
        p.requests.iter().filter(|(m, _)| m == "model/list").count(),
        1
    );
    for (_, v) in p.requests.iter().filter(|(m, _)| m == "turn/start") {
        assert_eq!(v["model"], "a");
        assert_eq!(v["effort"], "high");
    }
    let peer = peer.lock().unwrap();
    let resume = &peer
        .requests
        .iter()
        .find(|(m, _)| m == "thread/resume")
        .unwrap()
        .1;
    assert_eq!(resume["model"], "a");
    assert_eq!(
        resume.pointer("/config/model_reasoning_effort"),
        Some(&json!("high"))
    );
}
#[test]
fn changed_default_unknown_or_mismatched_binding_starts_fresh_and_keeps_local_history() {
    let (mut s, p) = session(vec![
        page(vec![model("a", true, "high")]),
        page(vec![model("b", true, "low")]),
    ]);
    let (first, b) = run(&mut s, request(), None);
    let (second, new) = run(&mut s, request(), b);
    assert_ne!(first.thread_id, second.thread_id);
    assert_eq!(second.request.prior_decisions, vec!["Keep the transcript"]);
    let mut req = second.request.clone();
    let mut unknown = new.unwrap();
    unknown.resolved_model = None;
    let (third, known) = run(&mut s, req.clone(), Some(unknown));
    assert_ne!(third.thread_id, second.thread_id);
    req.conversation_id = Some("different-session".into());
    let (fourth, _) = run(&mut s, req, known);
    assert_ne!(fourth.thread_id, third.thread_id);
    assert_eq!(p.lock().unwrap().threads, 4);
}
#[test]
fn model_substitution_and_reroutes_quarantine_without_binding_or_visible_response() {
    for reroute in [false, true] {
        let (mut s, p) = session(vec![page(vec![model("a", true, "high")])]);
        {
            let mut p = p.lock().unwrap();
            if reroute {
                p.reroute = true;
            } else {
                p.substituted_model = Some("substitute".into());
            }
        }
        let (r, b) = run(&mut s, request(), None);
        assert_eq!(r.status, ModelStatus::Quarantined);
        assert!(r.response.is_none() && b.is_none());
        assert!(r.detail.contains("model"));
        assert!(p.lock().unwrap().shutdown);
    }
}
#[test]
fn explicitly_empty_supported_effort_omits_effort_instead_of_guessing_medium() {
    let mut m = model("a", true, "high");
    m["supportedReasoningEfforts"] = json!([]);
    let (mut s, p) = session(vec![page(vec![m])]);
    let (r, _) = run(&mut s, request(), None);
    assert_eq!(r.status, ModelStatus::Completed);
    assert!(r.request.resolved_model.unwrap().effort.is_none());
    let p = p.lock().unwrap();
    let turn = &p
        .requests
        .iter()
        .find(|(m, _)| m == "turn/start")
        .unwrap()
        .1;
    assert!(turn.get("effort").is_none());
}

#[test]
fn availability_transport_loss_before_thread_preserves_request_and_reports_unavailable() {
    let (mut s, p) = session(vec![]);
    assert_eq!(s.refresh().unwrap().kind, ConnectionKind::Ready);
    p.lock().unwrap().availability_lost = true;
    let (run, binding) = run(&mut s, request(), None);
    assert_eq!(run.status, ModelStatus::Failed);
    assert_eq!(s.connection.kind, ConnectionKind::Unavailable);
    assert!(run.detail.contains("Reconnect or retry explicitly"));
    assert_eq!(run.request.context.id, "captured");
    assert!(binding.is_none() && run.thread_id.is_none() && run.turn_id.is_none());
    assert!(s.connection.limits.is_empty());
    assert_eq!(p.lock().unwrap().threads, 0);
}

#[test]
fn binding_language_channel_instruction_and_public_mode_mismatches_start_fresh() {
    let (mut s, p) = session(vec![page(vec![model("a", true, "high")])]);
    let (first, binding) = run(&mut s, request(), None);
    let binding = binding.unwrap();
    for mismatch in 0..4 {
        let mut changed = binding.clone();
        match mismatch {
            0 => changed.identity.as_mut().unwrap().language = "python".into(),
            1 => changed.identity.as_mut().unwrap().channel = "solution".into(),
            2 => changed.identity.as_mut().unwrap().mode = ThreadMode::PublicResearch,
            _ => changed.instructions_hash = Some("old instructions".into()),
        }
        let (next, _) = run(&mut s, first.request.clone(), Some(changed));
        assert_eq!(next.status, ModelStatus::Completed);
        assert_ne!(next.thread_id, first.thread_id);
    }
    assert_eq!(p.lock().unwrap().threads, 5);
}
