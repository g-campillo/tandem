//! Takeover: Claude does the active step, or every remaining step, itself. Before it starts,
//! Tandem snapshots the project's files into the state folder; when the turn ends, a diff
//! against the snapshot lists what changed (shell commands included), and the user keeps
//! or undoes each file. These are methods of the companion controller; `companion.rs`
//! routes the commands, the `step_done` tool and every ending takeover turn here.

use crate::companion::{Controller, lock};
use crate::guided::FollowUp;
use crate::prompts;
use crate::protocol::{Policy, TurnStatus};
use crate::state::{Connection, HostAnswer, HostQuestion, TakeoverRequest, TurnKind};
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tandem_core::chat::{
    ChangeSet, ChangeSetStatus, ChatItem, Decision, FileChange, ItemKind, Mode, Phase,
    TakeoverScope,
};
use tandem_core::takeover::{Snapshot, SnapshotChange};
use tandem_core::{Project, Store, new_id, now_ms};

const NO_TAKEOVER: &str = "No takeover is running.";
const ALL_RECORDED: &str = "Every step is already recorded. Check each \"done when\" criterion and call report_check with scope \"task\".";
const STEP_RECORDED: &str = "Recorded. Stop here; the user continues with the next step.";
const LAST_RECORDED: &str =
    "Recorded. Now check every \"done when\" criterion and call report_check with scope \"task\".";
const EARLIER_KEPT: &str = "Earlier changes from Claude were kept.";
const CLOSED_DURING: &str = "Tandem closed during a takeover. Review what Claude changed.";

/// The `step_done` tool input; `step` is 1-based.
#[derive(Deserialize)]
struct StepDoneInput {
    step: usize,
    summary: String,
}

/// The folder of change set `id`'s snapshot: `<state folder>/takeover/<id>`. `None` for an
/// id that is not a plain name, so a damaged record can never point elsewhere.
fn snapshot_dir(state_dir: &Path, id: &str) -> Option<PathBuf> {
    let plain = !id.is_empty()
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');
    plain.then(|| state_dir.join("takeover").join(id))
}

/// Merges a fresh diff into the set's files: an earlier decision stays for a path that
/// did not change again, every change is undecided, and the files are sorted by path.
fn merge(set: &mut ChangeSet, changes: Vec<SnapshotChange>) {
    let changed: BTreeSet<&str> = changes.iter().map(|change| change.path.as_str()).collect();
    set.files.retain(|file| {
        file.decision != Decision::Undecided && !changed.contains(file.path.as_str())
    });
    set.files
        .extend(changes.into_iter().map(|change| FileChange {
            path: change.path,
            kind: change.kind,
            decision: Decision::Undecided,
            after_hash: change.after_hash,
            undoable: change.undoable,
        }));
    set.files.sort_by(|a, b| a.path.cmp(&b.path));
}

fn has_undecided(set: &ChangeSet) -> bool {
    set.files
        .iter()
        .any(|file| file.decision == Decision::Undecided)
}

/// `Review` while a file waits for Keep or Undo, `Resolved` otherwise.
fn review_or_resolved(set: &ChangeSet) -> ChangeSetStatus {
    if has_undecided(set) {
        ChangeSetStatus::Review
    } else {
        ChangeSetStatus::Resolved
    }
}

/// Whether Keep or Undo applies to `file`: an undecided file named by `path`, or any
/// undecided file when `path` is `None`.
fn targeted(file: &FileChange, path: Option<&str>) -> bool {
    file.decision == Decision::Undecided && path.is_none_or(|path| path == file.path)
}

fn diff(project: &Project, dir: &Path) -> Result<Vec<SnapshotChange>> {
    Snapshot::open(dir)?.diff(project)
}

/// Removes a snapshot folder; one that is already gone is fine.
fn discard_snapshot(dir: &Path) -> Result<()> {
    match Snapshot::open(dir) {
        Ok(snapshot) => snapshot.discard(),
        Err(_) => match fs::remove_dir_all(dir) {
            Err(error) if error.kind() != ErrorKind::NotFound => Err(error)
                .with_context(|| format!("Cannot remove the takeover snapshot {}", dir.display())),
            _ => Ok(()),
        },
    }
}

/// Whether the transcript already has the card of change set `id`.
fn shows_changes(items: &[ChatItem], id: &str) -> bool {
    items
        .iter()
        .any(|item| matches!(&item.kind, ItemKind::Changes { change_set } if change_set == id))
}

/// Settles the takeovers still open when Tandem closed: each one's changes go up for review
/// in its task, which returns to the guided build. Runs at start-up, before the helper is
/// needed. Returns what went wrong, for the banner.
pub(crate) fn recover(project: &Project, store: &Mutex<Store>) -> Option<String> {
    let (state_dir, open) = {
        let store = lock(store);
        (
            store.directory.clone(),
            store.open_change_sets(project.key()),
        )
    };
    let open = match open {
        Ok(open) => open,
        Err(error) => {
            return Some(format!(
                "Could not check for an unfinished takeover: {error:#}"
            ));
        }
    };
    let problems: Vec<String> = open
        .into_iter()
        .filter_map(|mut set| recover_set(project, store, &state_dir, &mut set).err())
        .map(|error| format!("Could not recover a takeover: {error:#}"))
        .collect();
    (!problems.is_empty()).then(|| problems.join(" "))
}

fn recover_set(
    project: &Project,
    store: &Mutex<Store>,
    state_dir: &Path,
    set: &mut ChangeSet,
) -> Result<()> {
    let Some(dir) = snapshot_dir(state_dir, &set.id).filter(|dir| dir.is_dir()) else {
        // Nothing can be diffed or restored without the snapshot.
        set.status = ChangeSetStatus::Resolved;
        return lock(store).save_change_set(set);
    };
    // The diff walks the project, so the store stays unlocked meanwhile.
    let diffed = diff(project, &dir);
    let problem = match diffed {
        Ok(changes) => {
            merge(set, changes);
            None
        }
        Err(error) => Some(error),
    };
    set.status = review_or_resolved(set);
    {
        let mut store = lock(store);
        store.save_change_set(set)?;
        if !set.files.is_empty() && !shows_changes(&store.chat_items(&set.task_id)?, &set.id) {
            store.append_chat_item(
                &set.task_id,
                &ItemKind::Changes {
                    change_set: set.id.clone(),
                },
            )?;
        }
        if let Some(mut task) = store.chat_task(&set.task_id)? {
            task.mode = Mode::Guided;
            store.save_chat_task(&task)?;
        }
        store.append_chat_item(
            &set.task_id,
            &ItemKind::Notice {
                text: CLOSED_DURING.into(),
            },
        )?;
    }
    if set.status == ChangeSetStatus::Resolved {
        discard_snapshot(&dir)?;
    }
    problem.map_or(Ok(()), Err)
}

impl Controller {
    /// The editor's files with unsaved changes; none when the UI does not answer.
    pub(crate) fn dirty_paths(&self) -> Vec<String> {
        match self.ask_host(HostQuestion::DirtyPaths) {
            Some(HostAnswer::DirtyPaths(paths)) => paths,
            _ => Vec::new(),
        }
    }

    fn snapshot_dir(&self, id: &str) -> Result<PathBuf> {
        let state_dir = self.with_store(|store| Ok(store.directory.clone()))?;
        snapshot_dir(&state_dir, id).context("This takeover has a damaged id.")
    }

    /// Publishes the active task's change sets from the store.
    pub(crate) fn refresh_change_sets(&self) {
        let task_id = self.snapshot().task.as_ref().map(|task| task.id.clone());
        let sets = match task_id {
            Some(id) => self.with_store(|store| store.change_sets(&id)),
            None => Ok(Vec::new()),
        };
        self.update(|state| match sets {
            Ok(sets) => state.change_sets = sets,
            Err(error) => {
                state.banner = Some(format!("Could not load Claude's changes: {error:#}"))
            }
        });
    }

    /// The active task's change set `id`, as stored.
    fn change_set(&self, id: &str) -> Option<ChangeSet> {
        let task_id = self.snapshot().task.as_ref()?.id.clone();
        match self.with_store(|store| store.change_sets(&task_id)) {
            Ok(sets) => sets.into_iter().find(|set| set.id == id),
            Err(error) => {
                self.set_banner(format!("Could not load Claude's changes: {error:#}"));
                None
            }
        }
    }

    fn save_change_set(&self, set: &ChangeSet) -> bool {
        match self.with_store(|store| store.save_change_set(set)) {
            Ok(()) => true,
            Err(error) => {
                self.set_banner(format!("Could not save Claude's changes: {error:#}"));
                false
            }
        }
    }

    fn discard_snapshot(&self, id: &str) {
        if let Err(error) = self.snapshot_dir(id).and_then(|dir| discard_snapshot(&dir)) {
            self.set_banner(format!("{error:#}"));
        }
    }

    /// Diffs the project against the set's snapshot and merges the result into its files.
    fn diff_into(&self, set: &mut ChangeSet) {
        match self
            .snapshot_dir(&set.id)
            .and_then(|dir| diff(&self.project, &dir))
        {
            Ok(changes) => merge(set, changes),
            Err(error) => self.set_banner(format!("Could not list Claude's changes: {error:#}")),
        }
    }

    /// Adds the set's card to the transcript once it has files.
    fn show_changes(&self, set: &ChangeSet) {
        if !set.files.is_empty() && !shows_changes(&self.snapshot().items, &set.id) {
            self.append_item(ItemKind::Changes {
                change_set: set.id.clone(),
            });
        }
    }

    // ----- Commands -----

    /// "Do it for me" (`Step`), "Build the rest for me" and the plan card's "You do it"
    /// (`Rest`). `saved` lists the files the UI saved just before.
    pub(crate) fn take_over(&mut self, request: TakeoverRequest, saved: &[String]) {
        let state = self.snapshot();
        if state.turn.is_some() || !matches!(state.connection, Connection::Ready { .. }) {
            return;
        }
        let Some(mut task) = state.task.clone() else {
            return;
        };
        let Some(plan) = task.plan.clone() else {
            return;
        };
        let allowed = match (task.phase, request) {
            (Phase::Build, TakeoverRequest::Step) => task.current_step < plan.steps.len(),
            (Phase::Build | Phase::Plan, TakeoverRequest::Rest) => true,
            _ => false,
        };
        if !allowed {
            return;
        }
        // The UI saves before it sends this command; this is the safety net, because
        // Claude edits the files on disk.
        let dirty = self.dirty_paths();
        if !dirty.is_empty() {
            return self.set_banner(format!(
                "Save or close these files first: {}",
                dirty.join(", ")
            ));
        }
        if task.phase == Phase::Plan {
            task.phase = Phase::Build;
            task.current_step = 0;
        }
        self.close_earlier_sets(&task.id);
        let from = task.current_step;
        let (scope, text) = match request {
            TakeoverRequest::Step => {
                task.mode = Mode::Guided;
                (
                    TakeoverScope::Step(from),
                    prompts::takeover_step(&plan, from),
                )
            }
            TakeoverRequest::Rest => {
                task.mode = Mode::Takeover;
                (
                    TakeoverScope::Rest(from),
                    prompts::takeover_rest(&plan, from),
                )
            }
        };
        self.save_task(task);
        if !saved.is_empty() {
            self.append_item(ItemKind::Notice {
                text: format!(
                    "Tandem saved {} before the takeover, because Claude edits the files on disk.",
                    saved.join(", ")
                ),
            });
        }
        self.start_takeover_turn(scope, None, text);
    }

    /// The user's answer while a takeover of the remaining steps waits for one: Claude
    /// continues on the same change set, or on a new one when none is open.
    pub(crate) fn continue_takeover(&mut self, text: &str) {
        let Some(task) = self.snapshot().task.clone() else {
            return;
        };
        let Some(plan) = task.plan.clone() else {
            return;
        };
        let dirty = self.dirty_paths();
        if !dirty.is_empty() {
            return self.set_banner(format!(
                "Save or close these files first: {}",
                dirty.join(", ")
            ));
        }
        let open = match self.with_store(|store| store.change_sets(&task.id)) {
            Ok(sets) => sets
                .into_iter()
                .rev()
                .find(|set| set.status == ChangeSetStatus::Open),
            Err(error) => {
                return self.set_banner(format!("Could not load Claude's changes: {error:#}"));
            }
        };
        self.append_item(ItemKind::User {
            text: text.to_owned(),
        });
        let from = task.current_step;
        self.start_takeover_turn(
            TakeoverScope::Rest(from),
            open,
            prompts::takeover_chat(&plan, from, text),
        );
    }

    /// Keeps every undecided file of the task's earlier takeovers, so the new snapshot
    /// starts from the project as it is now. A takeover still open (paused for an answer)
    /// is diffed first, so none of its changes go unlisted.
    fn close_earlier_sets(&mut self, task_id: &str) {
        let sets = match self.with_store(|store| store.change_sets(task_id)) {
            Ok(sets) => sets,
            Err(error) => {
                return self.set_banner(format!("Could not load Claude's changes: {error:#}"));
            }
        };
        let mut kept = false;
        let mut closed = Vec::new();
        for mut set in sets
            .into_iter()
            .filter(|set| set.status != ChangeSetStatus::Resolved)
        {
            if set.status == ChangeSetStatus::Open {
                self.diff_into(&mut set);
            }
            for file in &mut set.files {
                if file.decision == Decision::Undecided {
                    file.decision = Decision::Kept;
                    kept = true;
                }
            }
            set.status = ChangeSetStatus::Resolved;
            if self.save_change_set(&set) {
                self.discard_snapshot(&set.id);
            }
            closed.push(set);
        }
        self.refresh_change_sets();
        for set in &closed {
            self.show_changes(set);
        }
        if kept {
            self.append_item(ItemKind::Notice {
                text: EARLIER_KEPT.into(),
            });
        }
    }

    /// Starts a takeover turn on `set`, or on a new change set whose snapshot is taken
    /// first. `preparing_takeover` keeps the editor read-only until the turn runs, so
    /// nothing the user saves meanwhile is taken for Claude's work.
    fn start_takeover_turn(&mut self, scope: TakeoverScope, set: Option<ChangeSet>, text: String) {
        self.update(|state| state.preparing_takeover = true);
        let created = set.is_none();
        let set = match set {
            Some(set) => set,
            None => match self.capture(scope) {
                Some(set) => set,
                None => return self.update(|state| state.preparing_takeover = false),
            },
        };
        let kind = TurnKind::Takeover {
            scope,
            change_set: set.id,
        };
        let started = self.start_turn(kind.clone(), text, Policy::Takeover);
        self.update(|state| state.preparing_takeover = false);
        if !started && created {
            // Nothing ran; the new set resolves without files.
            self.settle_takeover(&kind, TurnStatus::Error);
        }
    }

    /// Creates the active task's change set for `scope` and snapshots the project into
    /// its folder. On a failure the banner says why and the task returns to the guided
    /// build.
    fn capture(&mut self, scope: TakeoverScope) -> Option<ChangeSet> {
        let task = self.snapshot().task.clone()?;
        let set = ChangeSet {
            id: new_id(),
            task_id: task.id.clone(),
            scope,
            status: ChangeSetStatus::Open,
            files: Vec::new(),
            created_at_ms: now_ms(),
        };
        let captured = self.snapshot_dir(&set.id).and_then(|dir| {
            Snapshot::capture(&self.project, &dir)?;
            self.with_store(|store| store.save_change_set(&set))
                .inspect_err(|_| {
                    let _ = discard_snapshot(&dir);
                })
                .context("Could not save the takeover")
        });
        if let Err(error) = captured {
            let mut task = task;
            task.mode = Mode::Guided;
            self.save_task(task);
            self.set_banner(format!("{error:#}"));
            return None;
        }
        self.refresh_change_sets();
        Some(set)
    }

    // ----- Tools -----

    /// The `step_done` tool: Claude finished the active step during a takeover.
    pub(crate) fn step_done(&mut self, input: Value) -> (bool, String) {
        let state = self.snapshot();
        let Some(TurnKind::Takeover { scope, .. }) = state.turn.clone() else {
            return (false, NO_TAKEOVER.into());
        };
        let Some(mut task) = state.task.clone() else {
            return (false, NO_TAKEOVER.into());
        };
        let Some(plan) = &task.plan else {
            return (false, NO_TAKEOVER.into());
        };
        let total = plan.steps.len();
        let input = match serde_json::from_value::<StepDoneInput>(input) {
            Ok(input) => input,
            Err(error) => {
                return (
                    false,
                    format!("The step report does not have the expected fields: {error}."),
                );
            }
        };
        let active = task.current_step;
        let step = match scope {
            // A step takeover records its one step once.
            TakeoverScope::Step(step) => {
                if self.progress.step_recorded || active != step {
                    return (false, NO_TAKEOVER.into());
                }
                step
            }
            TakeoverScope::Rest(_) if active >= total => return (false, ALL_RECORDED.into()),
            TakeoverScope::Rest(_) => active,
        };
        if input.step != step + 1 {
            return (false, format!("Step {} is the active step.", step + 1));
        }
        let summary = match input.summary.trim() {
            "" => plan.steps[step].summary.clone(),
            summary => summary.to_owned(),
        };
        task.current_step = step + 1;
        let open = task.current_step < total;
        self.update(|state| state.guidance = None);
        self.save_task(task);
        self.append_item(ItemKind::Notice {
            text: format!("Step {} done: {summary}", step + 1),
        });
        self.progress.step_recorded = true;
        let answer = match scope {
            TakeoverScope::Step(_) => {
                self.progress.follow_up = Some(if open {
                    FollowUp::NextGuidance
                } else {
                    FollowUp::FinalCheck
                });
                STEP_RECORDED.to_owned()
            }
            TakeoverScope::Rest(_) if open => format!("Recorded. Continue with step {}.", step + 2),
            TakeoverScope::Rest(_) => LAST_RECORDED.to_owned(),
        };
        (true, answer)
    }

    // ----- The end of a takeover turn -----

    /// Records what a takeover turn changed. It runs for every way such a turn ends,
    /// including a helper that stopped, and before the turn is shown as ended, so the
    /// editor stays read-only until the diff is done. A takeover of the remaining steps
    /// that completed without finishing the task (Claude asked something) stays open.
    pub(crate) fn settle_takeover(&mut self, kind: &TurnKind, status: TurnStatus) {
        let TurnKind::Takeover { scope, change_set } = kind else {
            return;
        };
        let Some(mut set) = self.change_set(change_set) else {
            return;
        };
        let Some(mut task) = self.snapshot().task.clone() else {
            return;
        };
        self.diff_into(&mut set);
        let paused = matches!(scope, TakeoverScope::Rest(_))
            && status == TurnStatus::Completed
            && task.phase != Phase::Done;
        if !paused {
            set.status = review_or_resolved(&set);
            if task.mode != Mode::Guided {
                task.mode = Mode::Guided;
                self.save_task(task);
            }
        }
        if self.save_change_set(&set) && set.status == ChangeSetStatus::Resolved {
            self.discard_snapshot(&set.id);
        }
        self.refresh_change_sets();
        self.show_changes(&set);
    }

    // ----- Keep and Undo -----

    /// Keeps Claude's change to `path`, or to every undecided file. Works without Claude.
    pub(crate) fn keep(&mut self, change_set: &str, path: Option<&str>) {
        if self.snapshot().turn.is_some() {
            return;
        }
        let Some(mut set) = self.change_set(change_set) else {
            return;
        };
        // Keeping leaves the file as it is. The snapshot learns the new baseline, which
        // matters while the takeover is still open; without one there is nothing to learn.
        let mut snapshot = self
            .snapshot_dir(&set.id)
            .and_then(|dir| Snapshot::open(&dir))
            .ok();
        let mut changed = false;
        for file in set.files.iter_mut().filter(|file| targeted(file, path)) {
            if let Some(snapshot) = &mut snapshot
                && let Err(error) = snapshot.keep(&self.project, &file.path)
            {
                self.set_banner(format!("{error:#}"));
                continue;
            }
            file.decision = Decision::Kept;
            changed = true;
        }
        if changed {
            self.finish_set(set);
        }
    }

    /// Restores the version from before Claude's change of `path`, or of every undecided
    /// file. A file with unsaved editor changes, or one changed again since, is left alone.
    /// Works without Claude.
    pub(crate) fn undo(&mut self, change_set: &str, path: Option<&str>) {
        if self.snapshot().turn.is_some() {
            return;
        }
        let Some(mut set) = self.change_set(change_set) else {
            return;
        };
        if !set.files.iter().any(|file| targeted(file, path)) {
            return;
        }
        let dirty = self.dirty_paths();
        let snapshot = match self
            .snapshot_dir(&set.id)
            .and_then(|dir| Snapshot::open(&dir))
        {
            Ok(snapshot) => snapshot,
            Err(error) => return self.set_banner(format!("{error:#}")),
        };
        let mut changed = false;
        for file in set.files.iter_mut().filter(|file| targeted(file, path)) {
            if dirty.contains(&file.path) {
                self.set_banner(format!(
                    "Save or close {} before undoing Claude's change.",
                    file.path
                ));
                continue;
            }
            if !file.undoable {
                continue;
            }
            match snapshot.undo(&self.project, &file.path, file.after_hash.as_deref()) {
                Ok(()) => {
                    file.decision = Decision::Undone;
                    changed = true;
                }
                Err(error) => self.set_banner(format!("{error:#}")),
            }
        }
        if changed {
            self.finish_set(set);
        }
    }

    /// Saves a set after Keep or Undo. A reviewed set with no undecided file left is
    /// resolved and its snapshot removed; when nothing was kept and something was undone,
    /// the whole takeover was undone, so the plan goes back to where it started. The set is
    /// published once all of that is done, so the UI never sees it half finished.
    fn finish_set(&mut self, mut set: ChangeSet) {
        let resolved = set.status == ChangeSetStatus::Review && !has_undecided(&set);
        if resolved {
            set.status = ChangeSetStatus::Resolved;
        }
        let mut rolled_back = false;
        if self.save_change_set(&set) && resolved {
            self.discard_snapshot(&set.id);
            let kept = set.files.iter().any(|file| file.decision == Decision::Kept);
            let undone = set
                .files
                .iter()
                .any(|file| file.decision == Decision::Undone);
            if undone && !kept {
                rolled_back = self.roll_back(set.scope);
            }
        }
        self.refresh_change_sets();
        if rolled_back {
            self.guide_after_roll_back();
        }
    }

    /// Puts the task back at the step the takeover started from.
    fn roll_back(&mut self, scope: TakeoverScope) -> bool {
        let Some(mut task) = self.snapshot().task.clone() else {
            return false;
        };
        let (TakeoverScope::Step(start) | TakeoverScope::Rest(start)) = scope;
        task.current_step = start;
        task.phase = Phase::Build;
        task.mode = Mode::Guided;
        self.save_task(task);
        self.append_item(ItemKind::Notice {
            text: format!(
                "Claude's changes were undone. The plan is back at step {}.",
                start + 1
            ),
        });
        true
    }

    /// Shows the guidance for the step the plan went back to, asking Claude when it is
    /// connected and none is stored.
    fn guide_after_roll_back(&mut self) {
        let state = self.snapshot();
        if matches!(state.connection, Connection::Ready { .. }) && state.turn.is_none() {
            self.start_guidance();
        } else {
            self.refresh_guidance();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tandem_core::chat::ChangeKind;

    fn file(path: &str, kind: ChangeKind, decision: Decision) -> FileChange {
        FileChange {
            path: path.into(),
            kind,
            decision,
            after_hash: Some(format!("{path}-old")),
            undoable: true,
        }
    }

    fn change(path: &str, kind: ChangeKind) -> SnapshotChange {
        SnapshotChange {
            path: path.into(),
            kind,
            after_hash: Some(format!("{path}-new")),
            undoable: false,
        }
    }

    #[test]
    fn merge_keeps_decisions_of_unchanged_paths_and_replaces_the_rest() {
        let mut set = ChangeSet {
            id: "s".into(),
            task_id: "t".into(),
            scope: TakeoverScope::Rest(0),
            status: ChangeSetStatus::Open,
            files: vec![
                file("kept.rs", ChangeKind::Created, Decision::Kept),
                file("kept-then-changed.rs", ChangeKind::Created, Decision::Kept),
                file("undone.rs", ChangeKind::Modified, Decision::Undone),
                file("reverted.rs", ChangeKind::Modified, Decision::Undecided),
                file("still.rs", ChangeKind::Modified, Decision::Undecided),
            ],
            created_at_ms: 0,
        };
        merge(
            &mut set,
            vec![
                change("a.rs", ChangeKind::Created),
                change("kept-then-changed.rs", ChangeKind::Modified),
                change("still.rs", ChangeKind::Modified),
            ],
        );
        let merged: Vec<(&str, ChangeKind, Decision, Option<&str>)> = set
            .files
            .iter()
            .map(|file| {
                (
                    file.path.as_str(),
                    file.kind,
                    file.decision,
                    file.after_hash.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            merged,
            [
                (
                    "a.rs",
                    ChangeKind::Created,
                    Decision::Undecided,
                    Some("a.rs-new")
                ),
                (
                    "kept-then-changed.rs",
                    ChangeKind::Modified,
                    Decision::Undecided,
                    Some("kept-then-changed.rs-new")
                ),
                (
                    "kept.rs",
                    ChangeKind::Created,
                    Decision::Kept,
                    Some("kept.rs-old")
                ),
                (
                    "still.rs",
                    ChangeKind::Modified,
                    Decision::Undecided,
                    Some("still.rs-new")
                ),
                (
                    "undone.rs",
                    ChangeKind::Modified,
                    Decision::Undone,
                    Some("undone.rs-old")
                ),
            ]
        );
        assert!(!set.files[0].undoable, "undoable comes from the diff");
    }

    #[test]
    fn snapshot_folders_are_named_only_by_plain_ids() {
        let state = Path::new("/state");
        assert_eq!(
            snapshot_dir(state, "0b5e-41aa"),
            Some(PathBuf::from("/state/takeover/0b5e-41aa"))
        );
        for id in ["", "..", "../x", "a/b", "a b"] {
            assert_eq!(snapshot_dir(state, id), None, "{id:?}");
        }
    }
}
