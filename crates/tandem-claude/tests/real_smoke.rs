//! One real Claude turn through the real helper, Node and Claude Code, using the signed-in
//! Claude subscription. It is skipped unless `TANDEM_REAL_CLAUDE_SMOKE=1`; every other test
//! runs against the scripted fake helper and never starts a real turn.
//!
//! Run it once, by hand:
//! `TANDEM_REAL_CLAUDE_SMOKE=1 cargo test -p tandem-claude --test real_smoke -- --nocapture`

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tandem_claude::companion::Companion;
use tandem_claude::{
    ChatState, Command, CompanionConfig, Connection, HostAnswer, HostQuestion, Signal, TurnKind,
};
use tandem_core::Store;
use tandem_core::chat::{ChatPrefs, ItemKind};

const READY_LIMIT: Duration = Duration::from_secs(60);
const TURN_LIMIT: Duration = Duration::from_secs(120);

#[test]
fn real_claude_turn_round_trips() {
    if std::env::var("TANDEM_REAL_CLAUDE_SMOKE").as_deref() != Ok("1") {
        println!("skipped: set TANDEM_REAL_CLAUDE_SMOKE=1 to run one real Claude turn");
        return;
    }

    let temp = tempfile::tempdir().expect("temp dir");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).expect("project folder");
    std::fs::write(project.join("hello.txt"), "hello\n").expect("hello.txt");
    let mut store = Store::open(&temp.path().join("state")).expect("open the store");
    store
        .save_chat_prefs(&ChatPrefs {
            model: "haiku".into(),
            effort: Some("low".into()),
            use_my_setup: false,
            ..ChatPrefs::default()
        })
        .expect("save the preferences");
    let store = Arc::new(Mutex::new(store));

    let (companion, signals) = Companion::start(CompanionConfig {
        project,
        store: Arc::clone(&store),
        launch: None,
        offline: false,
    });
    // The editor has no open buffers: the host thread ends with the controller.
    thread::spawn(move || {
        while let Ok(signal) = signals.recv_blocking() {
            if let Signal::Host(ask) = signal {
                let answer = match ask.question {
                    HostQuestion::ReadBuffer { .. } => HostAnswer::Buffer(None),
                    HostQuestion::DirtyPaths => HostAnswer::DirtyPaths(Vec::new()),
                };
                let _ = ask.reply.send(answer);
            }
        }
    });

    let ready = wait_for(
        &companion,
        "a ready connection",
        READY_LIMIT,
        |state| match &state.connection {
            Connection::Ready { .. } => Some(true),
            Connection::Starting => None,
            _ => Some(false),
        },
    );
    let Connection::Ready {
        subscription,
        claude_code,
    } = ready.connection.clone()
    else {
        panic!("Claude is not ready: {:#?}", ready.connection);
    };
    println!("connected: {subscription} · Claude Code {claude_code}");
    assert!(!subscription.trim().is_empty(), "no subscription name");
    // A stale model falls back to Default; the one real turn must use Haiku.
    assert_eq!(
        ready.prefs.model, "haiku",
        "haiku is not offered: {ready:#?}"
    );

    companion.send(Command::Send("Reply with exactly the word: ok".into()));
    let mut seen_turn = false;
    let finished = wait_for(&companion, "a finished turn", TURN_LIMIT, |state| {
        seen_turn |= state.turn == Some(TurnKind::Chat);
        let answered = state
            .items
            .iter()
            .any(|item| matches!(item.kind, ItemKind::Assistant { .. }));
        if state.turn.is_none() && answered {
            Some(true)
        } else if !matches!(state.connection, Connection::Ready { .. })
            || (state.turn.is_none() && (seen_turn || state.banner.is_some()))
        {
            Some(false)
        } else {
            None
        }
    });

    let reply = finished
        .items
        .iter()
        .rev()
        .find_map(|item| match &item.kind {
            ItemKind::Assistant { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("Claude did not answer: {finished:#?}"));
    println!("Claude replied: {reply:?}");
    assert!(
        reply.to_lowercase().contains("ok"),
        "unexpected reply {reply:?}"
    );
    assert_eq!(finished.banner, None, "a problem was shown: {finished:#?}");
    let task_id = finished.task.as_ref().expect("an active task").id.clone();
    let stored = store
        .lock()
        .unwrap()
        .chat_task(&task_id)
        .expect("read the task")
        .expect("the stored task");
    assert!(stored.session_started, "the session was not marked started");

    companion.shutdown();
}

/// Polls the state until `verdict` returns `Some`: `Some(true)` returns the state and
/// `Some(false)`, or running out of time, fails with the state printed.
fn wait_for(
    companion: &Companion,
    what: &str,
    limit: Duration,
    mut verdict: impl FnMut(&ChatState) -> Option<bool>,
) -> Arc<ChatState> {
    let deadline = Instant::now() + limit;
    loop {
        let state = companion.state();
        match verdict(&state) {
            Some(true) => return state,
            Some(false) => panic!("failed waiting for {what}; state: {state:#?}"),
            None if Instant::now() >= deadline => {
                panic!("timed out waiting for {what}; state: {state:#?}")
            }
            None => thread::sleep(Duration::from_millis(100)),
        }
    }
}
