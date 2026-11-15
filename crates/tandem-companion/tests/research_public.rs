use anyhow::Result;
use std::{
    collections::VecDeque,
    net::IpAddr,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use tandem_companion::*;
use tandem_core::*;

fn brief() -> PublicResearchBrief {
    PublicResearchBrief {
        id: new_id(),
        topics: vec!["Rust iterators".into()],
        queries: vec!["Rust Iterator collect documentation".into()],
        urls: vec!["https://doc.rust-lang.org/std/iter/trait.Iterator.html".into()],
    }
}
fn page(url: &str) -> ResearchRequest {
    ResearchRequest::new(ResearchRequestKind::PublicPage { url: url.into() }).unwrap()
}
#[test]
fn public_brief_filters_private_data_without_forwarding_a_private_request() {
    let mut input = brief();
    input.queries.extend([
        "PRIVATE_PROJECT_SENTINEL deployment".into(),
        "/Users/alice/private/source.rs".into(),
        "fn secret() { private(); }".into(),
        "alice@internal.example".into(),
        "password=superprivate".into(),
    ]);
    let filtered = screen_public_brief(input, &["PRIVATE_PROJECT_SENTINEL".into()]).unwrap();
    assert_eq!(filtered.omissions.len(), 5);
    let safe = filtered.brief.unwrap();
    let serialized = serde_json::to_string(safe.brief()).unwrap();
    for secret in [
        "PRIVATE_PROJECT_SENTINEL",
        "source.rs",
        "fn secret",
        "alice@",
        "superprivate",
    ] {
        assert!(!serialized.contains(secret));
    }
    assert_eq!(
        safe.brief().queries,
        vec!["Rust Iterator collect documentation"]
    );
}
#[test]
fn encoded_credentials_and_malformed_encodings_are_withheld_from_queries_and_url_paths() {
    for unsafe_text in [
        "sk-abcdefghijklmnopqrstuv",
        "%73k-abcdefghijklmnopqrstuv",
        "s%6B-abcdefghijklmnopqrstuv",
        "sk%2dabcdefghijklmnopqrstuv",
        "sk-%61bcdefghijklmnopqrstuv",
        "%2573k-abcdefghijklmnopqrstuv",
        "%50RIVATE_PROJECT_SENTINEL",
        "%",
        "%7",
        "%GG",
        "%FF",
    ] {
        let mut input = brief();
        input.queries = vec![format!("Documentation {unsafe_text}")];
        input.urls = vec![format!("https://example.org/docs/{unsafe_text}")];
        let screened = screen_public_brief(input, &["PRIVATE_PROJECT_SENTINEL".into()]).unwrap();
        assert_eq!(screened.omissions.len(), 2, "{unsafe_text}");
        let safe = screened.brief.unwrap();
        assert!(safe.brief().queries.is_empty(), "{unsafe_text}");
        assert!(safe.brief().urls.is_empty(), "{unsafe_text}");
    }
    let mut input = brief();
    input.queries = vec!["Rust %49terator documentation".into()];
    input.urls = vec!["https://example.org/%49terator".into()];
    let screened = screen_public_brief(input, &[]).unwrap();
    assert!(screened.omissions.is_empty());
}
#[derive(Default)]
struct HttpFake {
    dns: VecDeque<Vec<IpAddr>>,
    responses: VecDeque<(PublicHttpResponse, Vec<u8>)>,
    requests: Vec<PublicHttpRequest>,
    cancel_during_body: bool,
}
impl PublicHttpTransport for HttpFake {
    fn resolve(&mut self, _: &str, _: &AtomicBool, _: Instant) -> Result<Vec<IpAddr>> {
        Ok(self
            .dns
            .pop_front()
            .unwrap_or_else(|| vec!["93.184.216.34".parse().unwrap()]))
    }
    fn get(
        &mut self,
        request: &PublicHttpRequest,
        cancel: &AtomicBool,
        _: Instant,
        receive: &mut dyn FnMut(&PublicHttpResponse, &[u8]) -> Result<()>,
    ) -> Result<PublicHttpResponse> {
        self.requests.push(request.clone());
        let (response, bytes) = self.responses.pop_front().unwrap();
        if self.cancel_during_body {
            cancel.store(true, Ordering::Relaxed);
        }
        receive(&response, &[])?;
        for part in bytes.chunks(8192) {
            receive(&response, part)?;
        }
        Ok(response)
    }
}
fn response(status: u16, mime: &str, location: Option<&str>) -> PublicHttpResponse {
    PublicHttpResponse {
        status,
        content_type: Some(mime.into()),
        content_encoding: None,
        location: location.map(str::to_string),
    }
}
fn fetch(fake: HttpFake, url: &str) -> (ResearchReceipt, PublicSourceFetcher<HttpFake>) {
    let mut fetcher = PublicSourceFetcher::with_transport(fake);
    let receipt = fetcher.fetch(page(url), &AtomicBool::new(false)).unwrap();
    (receipt, fetcher)
}
#[test]
fn fetches_visible_text_with_anonymous_pinned_request_and_citable_provenance() {
    let fake = HttpFake { responses: vec![(response(200,"text/html; charset=utf-8",None), b"<html><head><title>Iterator docs</title><script>secret()</script></head><body><h1>Iterator</h1><form>hidden form</form><p hidden>hidden data</p><p>Public text &amp; evidence</p><script>expand authority</script></body></html>".to_vec())].into(), ..Default::default() };
    let (receipt, fetcher) = fetch(fake, "https://example.org/docs");
    assert_eq!(receipt.status, ResearchReceiptStatus::Complete);
    assert!(receipt.content.contains("Public text & evidence"));
    for hidden in ["secret()", "hidden form", "hidden data", "expand authority"] {
        assert!(!receipt.content.contains(hidden));
    }
    let metadata = receipt.public_web.as_ref().unwrap();
    assert_eq!(metadata.title.as_deref(), Some("Iterator docs"));
    assert_eq!(metadata.final_url, "https://example.org/docs");
    let request = &fetcher.transport().requests[0];
    assert_eq!(
        request.addresses(),
        vec!["93.184.216.34:443".parse::<std::net::SocketAddr>().unwrap()]
    );
    assert_eq!(request.connect_timeout(), Duration::from_secs(5));
    assert!(
        request.total_timeout() <= Duration::from_secs(15) && !request.total_timeout().is_zero()
    );
    let finding = ResearchFinding {
        summary: "Iterator documentation was retrieved".into(),
        citations: vec![ResearchCitation {
            receipt_id: receipt.id.clone(),
            source_id: receipt.sources[0].id.clone(),
            locator: receipt.sources[0].locator.clone(),
        }],
    };
    finding.validate(&[receipt]).unwrap();
}
#[test]
fn rejects_nonpublic_and_credentialed_urls_and_dns_without_http() {
    for url in [
        "http://example.org/",
        "https://user:pass@example.org/",
        "https://example.org:444/",
        "https://127.0.0.1/",
        "https://[::ffff:127.0.0.1]/",
        "https://[::1]/",
        "https://10.0.0.1/",
        "https://example.local/",
        "https://example.org/?access_token=secret",
    ] {
        // Structural request validation can reject an input before the fetcher does.
        if let Ok(request) =
            ResearchRequest::new(ResearchRequestKind::PublicPage { url: url.into() })
        {
            let mut f = PublicSourceFetcher::with_transport(HttpFake::default());
            let r = f.fetch(request, &AtomicBool::new(false)).unwrap();
            assert_eq!(r.status, ResearchReceiptStatus::Unavailable, "{url}");
            assert!(f.transport().requests.is_empty());
        }
    }
    for ip in [
        "127.0.0.1",
        "169.254.1.1",
        "192.168.1.1",
        "100.64.0.1",
        "198.18.0.1",
        "192.0.2.1",
        "224.0.0.1",
        "::1",
        "fc00::1",
        "fe80::1",
        "::ffff:10.0.0.1",
        "2001:db8::1",
        "2002:7f00:1::",
    ] {
        let f = HttpFake {
            dns: vec![vec![ip.parse().unwrap()]].into(),
            ..Default::default()
        };
        let (r, f) = fetch(f, "https://example.org/");
        assert_eq!(r.status, ResearchReceiptStatus::Unavailable, "{ip}");
        assert!(f.transport().requests.is_empty());
    }
}
#[test]
fn redirect_revalidates_dns_and_blocks_rebinding_and_fourth_hop() {
    let fake = HttpFake {
        dns: vec![
            vec!["93.184.216.34".parse().unwrap()],
            vec!["127.0.0.1".parse().unwrap()],
        ]
        .into(),
        responses: vec![(response(302, "text/plain", Some("/next")), vec![])].into(),
        ..Default::default()
    };
    let (r, f) = fetch(fake, "https://example.org/start");
    assert_eq!(r.status, ResearchReceiptStatus::Unavailable);
    assert_eq!(f.transport().requests.len(), 1);
    let fake = HttpFake {
        responses: (0..4)
            .map(|_| (response(302, "text/plain", Some("/next")), vec![]))
            .collect(),
        ..Default::default()
    };
    let (r, f) = fetch(fake, "https://example.org/");
    assert_eq!(r.status, ResearchReceiptStatus::Unavailable);
    assert_eq!(f.transport().requests.len(), 4);
}
#[test]
fn unsupported_oversize_encoded_and_cancelled_sources_never_become_citations() {
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut gzip, &vec![b'x'; 1024 * 1024 + 1]).unwrap();
    let mut compressed = response(200, "text/plain", None);
    compressed.content_encoding = Some("gzip".into());
    let mut unsupported = response(200, "text/plain", None);
    unsupported.content_encoding = Some("br".into());
    for (response, bytes) in [
        (response(200, "application/pdf", None), vec![]),
        (response(403, "text/plain", None), b"denied".to_vec()),
        (
            response(200, "text/plain", None),
            vec![b'x'; 1024 * 1024 + 1],
        ),
        (compressed, gzip.finish().unwrap()),
        (unsupported, vec![]),
    ] {
        let (r, _) = fetch(
            HttpFake {
                responses: vec![(response, bytes)].into(),
                ..Default::default()
            },
            "https://example.org/",
        );
        assert_eq!(r.status, ResearchReceiptStatus::Unavailable);
        assert!(r.sources.is_empty());
    }
    let (r, _) = fetch(
        HttpFake {
            responses: vec![(response(200, "text/plain", None), b"cancelled".to_vec())].into(),
            cancel_during_body: true,
            ..Default::default()
        },
        "https://example.org/",
    );
    assert_eq!(r.status, ResearchReceiptStatus::Cancelled);
    assert!(r.sources.is_empty());
}

use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tandem_codex::ProtocolExchange;
#[derive(Default)]
struct SearchPeer {
    requests: Vec<(String, Value)>,
    events: VecDeque<Value>,
    narrative_only: bool,
    wrong_correlation: bool,
    unknown_tool: bool,
    reroute: bool,
    unsupported: bool,
    threads: usize,
    shutdown: bool,
    before_search: Vec<Value>,
    between_search: Vec<Value>,
    answer_item: Option<Value>,
}
#[derive(Clone)]
struct SearchFake(Arc<Mutex<SearchPeer>>);
impl ProtocolExchange for SearchFake {
    fn request(&mut self, method: &str, params: Value, _: Duration) -> Result<Value> {
        let mut p = self.0.lock().unwrap();
        p.requests.push((method.into(), params.clone()));
        Ok(match method {
            "account/read" => json!({"account":{"type":"chatgpt"}}),
            "account/rateLimits/read" => json!({"ordinaryUsageAllowed":true}),
            "thread/start" => {
                p.threads += 1;
                json!({"thread":{"id":format!("public-{}",p.threads)},"model":params["model"],"modelProvider":"openai","reasoningEffort":params["config"]["model_reasoning_effort"],"instructionSources":[]})
            }
            "turn/start" => {
                if p.unsupported {
                    anyhow::bail!("search not supported private diagnostic");
                }
                let thread = params["threadId"].clone();
                let turn = "turn-public";
                if p.reroute {
                    p.events.push_back(json!({"method":"model/rerouted","params":{"threadId":thread,"turnId":turn,"toModel":"other"}}));
                }
                if p.unknown_tool {
                    p.events.push_back(json!({"method":"item/started","params":{"threadId":thread,"turnId":turn,"item":{"type":"commandExecution","id":"tool"}}}));
                }
                for item in p.before_search.clone() {
                    p.events.push_back(json!({"method":"item/completed","params":{"threadId":thread,"turnId":turn,"item":item}}));
                }
                if !p.narrative_only {
                    let correlated = if p.wrong_correlation {
                        "private-thread"
                    } else {
                        thread.as_str().unwrap()
                    };
                    for method in ["item/started", "item/completed"] {
                        p.events.push_back(json!({"method":method,"params":{"threadId":correlated,"turnId":turn,"item":{"type":"webSearch","id":"search-1","query":"Rust Iterator docs","action":{"type":"search","queries":["Rust Iterator docs"]}}}}));
                        if method == "item/started" {
                            for item in p.between_search.clone() {
                                p.events.push_back(json!({"method":"item/completed","params":{"threadId":thread,"turnId":turn,"item":item}}));
                            }
                        }
                    }
                }
                let item = p.answer_item.clone().unwrap_or_else(|| {
                    candidate_answer(
                        &["https://doc.rust-lang.org/std/iter/trait.Iterator.html"],
                        None,
                    )
                });
                p.events.push_back(json!({"method":"item/completed","params":{"threadId":thread,"turnId":turn,"item":item}}));
                p.events.push_back(json!({"method":"turn/completed","params":{"threadId":thread,"turn":{"id":turn,"status":"completed"}}}));
                json!({"turn":{"id":turn}})
            }
            "turn/interrupt" => json!({}),
            _ => anyhow::bail!("Unexpected public method {method}"),
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
fn agent_message(text: &str, phase: Option<&str>) -> Value {
    let mut item = json!({"type":"agentMessage","text":text});
    if let Some(phase) = phase {
        item["phase"] = json!(phase);
    }
    item
}
fn candidate_answer(candidates: &[&str], phase: Option<&str>) -> Value {
    agent_message(&json!({"candidates":candidates}).to_string(), phase)
}
fn search_with_state(state: SearchPeer) -> (PublicSearchOutcome, Vec<PublicResearchProgress>) {
    let mut runtime = PublicResearchSession::with_exchange(
        SearchFake(Arc::new(Mutex::new(state))),
        PathBuf::from("/empty-public-scratch"),
    );
    let safe = screen_public_brief(brief(), &[]).unwrap().brief.unwrap();
    let mut progress = vec![];
    let result = runtime
        .search(&safe, &selected(), &AtomicBool::new(false), |p| {
            progress.push(p.clone());
            Ok(())
        })
        .unwrap();
    (result, progress)
}
#[test]
fn rejected_candidates_produce_sanitized_partial_or_unavailable_receipts() {
    let rejected = [
        "https://example.org/docs?q=Iterator",
        "https://example.org/sk-abcdefghijklmnopqrstuv",
        "https://user:privatevalue@example.org/docs",
        "https://example.internal/PRIVATE_PROJECT_SENTINEL",
    ];
    for mixed in [false, true] {
        let mut candidates = rejected.to_vec();
        if mixed {
            candidates.push("https://example.org/docs");
        }
        let (result, _) = search_with_state(SearchPeer {
            answer_item: Some(candidate_answer(&candidates, None)),
            ..Default::default()
        });
        assert_eq!(
            result.receipt.status,
            if mixed {
                ResearchReceiptStatus::Partial
            } else {
                ResearchReceiptStatus::Unavailable
            }
        );
        assert_eq!(result.candidates.len(), usize::from(mixed));
        assert_eq!(result.receipt.omissions.len(), rejected.len());
        for omission in &result.receipt.omissions {
            assert!(omission.locator.is_none());
            assert!(!omission.reason.is_empty() && omission.reason.len() <= 256);
        }
        let receipt = serde_json::to_string(&result.receipt).unwrap();
        for secret in rejected {
            assert!(!receipt.contains(secret));
        }
        for secret in [
            "abcdefghijklmnopqrstuv",
            "privatevalue",
            "PRIVATE_PROJECT_SENTINEL",
        ] {
            assert!(!receipt.contains(secret));
        }
    }
    let (empty, _) = search_with_state(SearchPeer {
        answer_item: Some(candidate_answer(&[], None)),
        ..Default::default()
    });
    assert_eq!(empty.receipt.status, ResearchReceiptStatus::Complete);
    assert!(empty.candidates.is_empty() && empty.receipt.omissions.is_empty());
}
#[test]
fn phased_commentary_and_partial_answers_do_not_replace_the_final_candidates() {
    let hidden = "UNVALIDATED_PROGRESS_SENTINEL";
    let mut null_phase = candidate_answer(&["https://example.org/docs"], None);
    null_phase["phase"] = Value::Null;
    for answer in [
        candidate_answer(&["https://example.org/docs"], Some("final_answer")),
        candidate_answer(&["https://example.org/docs"], None),
        null_phase,
    ] {
        let (result, progress) = search_with_state(SearchPeer {
            before_search: vec![agent_message(hidden, Some("commentary"))],
            between_search: vec![
                agent_message(hidden, Some("commentary")),
                agent_message(hidden, Some("partial_answer")),
            ],
            answer_item: Some(answer),
            ..Default::default()
        });
        assert_eq!(result.receipt.status, ResearchReceiptStatus::Complete);
        assert_eq!(result.candidates, vec!["https://example.org/docs"]);
        assert_eq!(result.observations.len(), 2);
        assert!(!format!("{progress:?}").contains(hidden));
    }
}
#[test]
fn invalid_or_missing_final_answers_and_excess_interim_text_are_unavailable() {
    for answer in [
        agent_message("invalid final JSON", Some("final_answer")),
        agent_message("invalid legacy JSON", None),
        candidate_answer(&["https://example.org/docs"], Some("commentary")),
        candidate_answer(&["https://example.org/docs"], Some("partial_answer")),
        candidate_answer(&["https://example.org/docs"], Some("unknown_phase")),
    ] {
        let (result, _) = search_with_state(SearchPeer {
            before_search: vec![agent_message("Searching", Some("commentary"))],
            answer_item: Some(answer),
            ..Default::default()
        });
        assert_eq!(result.receipt.status, ResearchReceiptStatus::Unavailable);
        assert!(result.candidates.is_empty());
    }
    let (result, _) = search_with_state(SearchPeer {
        before_search: vec![agent_message(&"x".repeat(64 * 1024), Some("commentary"))],
        between_search: vec![agent_message(
            &"x".repeat(64 * 1024),
            Some("partial_answer"),
        )],
        answer_item: Some(candidate_answer(
            &["https://example.org/docs"],
            Some("final_answer"),
        )),
        ..Default::default()
    });
    assert_eq!(result.receipt.status, ResearchReceiptStatus::Unavailable);
    let (result, _) = search_with_state(SearchPeer {
        narrative_only: true,
        before_search: vec![agent_message("I searched", Some("commentary"))],
        answer_item: Some(candidate_answer(
            &["https://example.org/docs"],
            Some("final_answer"),
        )),
        ..Default::default()
    });
    assert_eq!(result.receipt.status, ResearchReceiptStatus::Unavailable);
}
fn selected() -> ResolvedModel {
    ResolvedModel {
        model: "selected".into(),
        catalog_id: "catalog-selected".into(),
        display_name: "Selected model".into(),
        effort: Some("high".into()),
    }
}
#[test]
fn public_worker_sends_only_screened_brief_and_observes_correlated_real_search_items() {
    let peer = Arc::new(Mutex::new(SearchPeer::default()));
    let mut runtime = PublicResearchSession::with_exchange(
        SearchFake(peer.clone()),
        PathBuf::from("/empty-public-scratch"),
    );
    let safe = screen_public_brief(brief(), &["PRIVATE_SENTINEL".into()])
        .unwrap()
        .brief
        .unwrap();
    let mut progress = vec![];
    let result = runtime
        .search(&safe, &selected(), &AtomicBool::new(false), |p| {
            progress.push(p.clone());
            Ok(())
        })
        .unwrap();
    assert_eq!(result.receipt.status, ResearchReceiptStatus::Complete);
    assert!(result.receipt.sources.is_empty());
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.observations.len(), 2);
    assert!(matches!(
        progress[0],
        PublicResearchProgress::BriefPrepared { .. }
    ));
    // Every call starts a fresh thread, including the same brief. Private
    // ThreadBinding and ModelRequest cannot be supplied to this API.
    runtime
        .search(&safe, &selected(), &AtomicBool::new(false), |_| Ok(()))
        .unwrap();
    let p = peer.lock().unwrap();
    assert_eq!(p.threads, 2);
    let start = &p
        .requests
        .iter()
        .find(|(m, _)| m == "thread/start")
        .unwrap()
        .1;
    assert_eq!(start["config"]["web_search"], "live");
    assert_eq!(start["config"]["features.standalone_web_search"], false);
    assert_eq!(start["allowProviderModelFallback"], false);
    assert_eq!(start["model"], "selected");
    let turn = &p
        .requests
        .iter()
        .find(|(m, _)| m == "turn/start")
        .unwrap()
        .1;
    assert_eq!(
        turn["input"][0]["text"],
        serde_json::to_string(safe.brief()).unwrap()
    );
    assert!(
        !serde_json::to_string(&p.requests)
            .unwrap()
            .contains("PRIVATE_SENTINEL")
    );
    assert!(
        !p.requests
            .iter()
            .any(|(m, _)| m == "thread/resume" || m == "model/list")
    );
}
#[test]
fn narrative_fake_correlation_unknown_tool_unsupported_and_reroute_are_unavailable() {
    for mode in 0..5 {
        let mut state = SearchPeer::default();
        match mode {
            0 => state.narrative_only = true,
            1 => state.wrong_correlation = true,
            2 => state.unknown_tool = true,
            3 => state.unsupported = true,
            _ => state.reroute = true,
        }
        let peer = Arc::new(Mutex::new(state));
        let mut runtime = PublicResearchSession::with_exchange(
            SearchFake(peer.clone()),
            PathBuf::from("/scratch"),
        );
        let safe = screen_public_brief(brief(), &[]).unwrap().brief.unwrap();
        let result = runtime
            .search(&safe, &selected(), &AtomicBool::new(false), |_| Ok(()))
            .unwrap();
        assert_eq!(
            result.receipt.status,
            ResearchReceiptStatus::Unavailable,
            "mode {mode}"
        );
        assert!(result.candidates.is_empty() && result.receipt.sources.is_empty());
        assert!(!result.receipt.content.contains("private diagnostic"));
        assert_eq!(peer.lock().unwrap().threads, 1);
    }
}
#[test]
fn persistence_failure_or_cancellation_prevents_public_dispatch() {
    let peer = Arc::new(Mutex::new(SearchPeer::default()));
    let mut runtime =
        PublicResearchSession::with_exchange(SearchFake(peer.clone()), PathBuf::from("/scratch"));
    let safe = screen_public_brief(brief(), &[]).unwrap().brief.unwrap();
    assert!(
        runtime
            .search(
                &safe,
                &selected(),
                &AtomicBool::new(false),
                |_| anyhow::bail!("storage unavailable")
            )
            .is_err()
    );
    assert!(peer.lock().unwrap().requests.is_empty());
    let result = runtime
        .search(&safe, &selected(), &AtomicBool::new(true), |_| Ok(()))
        .unwrap();
    assert_eq!(result.receipt.status, ResearchReceiptStatus::Cancelled);
    assert!(peer.lock().unwrap().requests.is_empty());
}

#[test]
fn brief_identity_and_url_parameters_cannot_carry_private_data() {
    let mut input = brief();
    input.id = "PRIVATE_PROJECT_SENTINEL".into();
    input.urls = vec![
        "https://example.org/docs?session=privatevalue".into(),
        "https://example.org/docs?path=%2Ftmp%2Fprivate".into(),
    ];
    let result = screen_public_brief(input, &["PRIVATE_PROJECT_SENTINEL".into()]).unwrap();
    assert_eq!(result.omissions.len(), 2);
    let serialized = serde_json::to_string(result.brief.unwrap().brief()).unwrap();
    assert!(!serialized.contains("PRIVATE_PROJECT_SENTINEL"));
    assert!(!serialized.contains("privatevalue"));
}
#[test]
fn public_locator_cannot_claim_project_provenance_or_bypass_failed_retrieval() {
    let source = ResearchSource {
        id: new_id(),
        locator: SourceLocator::PublicUrl {
            url: "https://example.org/".into(),
        },
        source_hash: content_hash(b"invented"),
        captured_at_ms: now_ms(),
        provenance: ResearchProvenance::ProjectFile,
    };
    assert!(
        ResearchReceipt::new(
            page("https://example.org/"),
            vec![source],
            ResearchReceiptStatus::Complete,
            "invented".into(),
            vec![]
        )
        .is_err()
    );
    let fake = HttpFake {
        responses: vec![(
            response(200, "text/plain", None),
            b"verified public text".to_vec(),
        )]
        .into(),
        ..Default::default()
    };
    let (original, _) = fetch(fake, "https://example.org/");
    let citation = ResearchFinding {
        summary: "A public fact".into(),
        citations: vec![ResearchCitation {
            receipt_id: original.id.clone(),
            source_id: original.sources[0].id.clone(),
            locator: original.sources[0].locator.clone(),
        }],
    };
    for mode in 0..4 {
        let mut changed = original.clone();
        match mode {
            0 => changed.public_web = None,
            1 => changed.public_web.as_mut().unwrap().http_status = Some(403),
            2 => changed.public_web.as_mut().unwrap().body_hash = Some(content_hash(b"different")),
            _ => changed.status = ResearchReceiptStatus::Unavailable,
        }
        assert!(citation.validate(&[changed]).is_err());
    }
}
#[test]
fn source_limitations_name_unsupported_content_and_size() {
    for (mime, bytes, expected) in [
        ("application/pdf", b"pdf".to_vec(), "content type"),
        ("text/plain", vec![b'x'; 1024 * 1024 + 1], "download limit"),
    ] {
        let (r, _) = fetch(
            HttpFake {
                responses: vec![(response(200, mime, None), bytes)].into(),
                ..Default::default()
            },
            "https://example.org/",
        );
        assert!(
            r.omissions.iter().any(|o| o.reason.contains(expected)),
            "{:?}",
            r.omissions
        );
    }
}
