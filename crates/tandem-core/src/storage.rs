use crate::ModelPreference;
use crate::conversation::{bounded_structure, bounded_text, stable_id};
use crate::{
    AgentRun, AgentStatus, CheckRecord, CheckStatus, ClarificationAnswer, ClarificationQuestion,
    ConsumedActionTransition, ConversationEvent, ConversationPayload, Document, DocumentProposal,
    DocumentRef, GuidanceSettings, ImportStatus, ImportTransaction, LegacyTaskLink, ModelRun,
    ModelStatus, OperationOutcome, PlanDocument, Project, ProjectTools, ResearchActivity,
    ResearchFinding, ResearchReceipt, SessionOperation, SpecDocument, TaskPlan, TaskSession,
    ThreadBinding, Walkthrough, WorkflowAction, WorkflowBinding, WorkflowDocumentKind, new_id,
    now_ms,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceState {
    pub root: String,
    pub tabs: Vec<String>,
    pub active: Option<String>,
    pub font_size: f32,
    #[serde(default)]
    pub sequence: u64,
}

impl WorkspaceState {
    pub fn new(project: &Project) -> Self {
        Self {
            root: project.key().into(),
            tabs: Vec::new(),
            active: None,
            font_size: 14.0,
            sequence: 0,
        }
    }
}

pub struct Store {
    connection: Connection,
    _lock: File,
    pub directory: PathBuf,
}

impl Store {
    pub fn open(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("workspace.lock"))?;
        lock.try_lock()
            .context("Another Tandem instance is using this state directory")?;
        let mut connection = Connection::open(directory.join("workspace.sqlite"))?;
        connection.busy_timeout(std::time::Duration::from_secs(2))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        ensure!(
            version <= 5,
            "This workspace state belongs to a newer Tandem version; it was not modified"
        );
        if version > 0 && version < 5 {
            let backup = directory.join(format!(
                "workspace-schema{version}-backup-{}.sqlite",
                now_ms()
            ));
            connection
                .backup(rusqlite::MAIN_DB, &backup, rusqlite::backup::NO_PROGRESS)
                .context("Could not back up workspace before migration")?;
        }
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
        let migration = connection.transaction()?;
        migration.execute_batch("
            CREATE TABLE IF NOT EXISTS workspace(root TEXT PRIMARY KEY, data TEXT NOT NULL, opened INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS draft(root TEXT NOT NULL, path TEXT NOT NULL, data TEXT NOT NULL, PRIMARY KEY(root,path));
            CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS project_tools(root TEXT PRIMARY KEY, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS task(id TEXT PRIMARY KEY, root TEXT NOT NULL, revision INTEGER NOT NULL, updated INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS task_revision(id TEXT NOT NULL, revision INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(id,revision));
            CREATE TABLE IF NOT EXISTS check_run(id TEXT PRIMARY KEY, root TEXT NOT NULL, sequence INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS model_run(id TEXT PRIMARY KEY,root TEXT NOT NULL,sequence INTEGER NOT NULL,data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS companion_thread(root TEXT NOT NULL,name TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(root,name));
            CREATE TABLE IF NOT EXISTS agent_run(id TEXT PRIMARY KEY,root TEXT NOT NULL,sequence INTEGER NOT NULL,data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS import_group(id TEXT PRIMARY KEY,root TEXT NOT NULL,sequence INTEGER NOT NULL,data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS import_history(id TEXT NOT NULL,sequence INTEGER NOT NULL,data TEXT NOT NULL,PRIMARY KEY(id,sequence));
            CREATE TABLE IF NOT EXISTS task_session(id TEXT PRIMARY KEY, root TEXT NOT NULL, sequence INTEGER NOT NULL, task_id TEXT UNIQUE, updated INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS session_selection(root TEXT PRIMARY KEY, session_id TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS session_composer(session_id TEXT PRIMARY KEY, sequence INTEGER NOT NULL, text TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS conversation_event(id TEXT PRIMARY KEY, session_id TEXT NOT NULL, sequence INTEGER NOT NULL, data TEXT NOT NULL, UNIQUE(session_id,sequence));
            CREATE TABLE IF NOT EXISTS document_revision(session_id TEXT NOT NULL, kind TEXT NOT NULL, revision INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(session_id,kind,revision));
            CREATE TABLE IF NOT EXISTS research_receipt(id TEXT PRIMARY KEY, session_id TEXT NOT NULL, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS session_transition(id TEXT PRIMARY KEY, session_id TEXT NOT NULL, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS action_transition(id TEXT PRIMARY KEY, session_id TEXT NOT NULL, walkthrough_id TEXT NOT NULL, action_id TEXT NOT NULL, review_id TEXT NOT NULL UNIQUE, data TEXT NOT NULL, UNIQUE(session_id,walkthrough_id,action_id));
            CREATE TABLE IF NOT EXISTS session_operation(id TEXT PRIMARY KEY, session_id TEXT NOT NULL, data TEXT NOT NULL, outcome TEXT);
            CREATE UNIQUE INDEX IF NOT EXISTS one_active_session_operation ON session_operation(session_id) WHERE outcome IS NULL;
            CREATE TABLE IF NOT EXISTS legacy_task_link(session_id TEXT PRIMARY KEY, task_id TEXT NOT NULL UNIQUE, data TEXT NOT NULL);
            PRAGMA user_version=5;")?;
        migration.commit()?;
        let mut store = Self {
            connection,
            _lock: lock,
            directory: directory.to_path_buf(),
        };
        store.reconcile_interrupted_checks()?;
        store.reconcile_interrupted_models()?;
        store.reconcile_interrupted_agents()?;
        store.reconcile_interrupted_session_operations()?;
        Ok(store)
    }

    pub fn model_preference(&self) -> Result<ModelPreference> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM settings WHERE key='model_preference'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let preference: ModelPreference = value
            .map(|s| serde_json::from_str(&s).context("Model preference is damaged"))
            .transpose()?
            .unwrap_or_default();
        preference.validate()?;
        Ok(preference)
    }
    pub fn save_model_preference(&mut self, preference: &ModelPreference) -> Result<()> {
        preference.validate()?;
        self.connection.execute("INSERT INTO settings(key,value) VALUES('model_preference',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [serde_json::to_string(preference)?])?;
        Ok(())
    }

    pub fn guidance(&self) -> Result<GuidanceSettings> {
        let value: Option<String> = self
            .connection
            .query_row("SELECT value FROM settings WHERE key='guidance'", [], |r| {
                r.get(0)
            })
            .optional()?;
        value
            .map(|s| serde_json::from_str(&s).context("Guidance settings are damaged"))
            .transpose()
            .map(Option::unwrap_or_default)
    }
    pub fn agent_runs(&self, root: &str) -> Result<Vec<AgentRun>> {
        let rows = self
            .connection
            .prepare("SELECT data FROM agent_run WHERE root=?1 ORDER BY rowid DESC LIMIT 20")?
            .query_map([root], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|s| serde_json::from_str(&s).context("Delegation journal is damaged"))
            .collect()
    }
    pub fn save_agent_run(&mut self, run: &AgentRun) -> Result<()> {
        run.grant.validate()?;
        ensure!(
            run.project == run.grant.project && run.id == run.grant.id,
            "Delegation identity changed"
        );
        let previous: Option<String> = self
            .connection
            .query_row("SELECT data FROM agent_run WHERE id=?1", [&run.id], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(previous) = previous {
            let old: AgentRun = serde_json::from_str(&previous)?;
            ensure!(
                old.grant.scope_fingerprint() == run.grant.scope_fingerprint(),
                "Delegation authority is immutable; create a new grant"
            );
            ensure!(
                old.grant.confirmed_at_ms == run.grant.confirmed_at_ms
                    || (old.status == AgentStatus::Prepared
                        && old.grant.confirmed_at_ms == 0
                        && run.grant.confirmed_at_ms > 0),
                "Delegation confirmation cannot be replaced"
            );
            ensure!(
                !(!old.status.active()
                    && old.status != AgentStatus::Prepared
                    && (run.status.active() || run.status == AgentStatus::Prepared)),
                "A terminal delegation cannot resume without a new grant"
            );
            ensure!(
                run.sequence != old.sequence
                    || serde_json::to_string(&old)? == serde_json::to_string(run)?,
                "Delegation version already belongs to a different snapshot"
            );
            ensure!(
                run.sequence >= old.sequence,
                "A newer delegation state is already recorded"
            );
        }
        self.connection.execute("INSERT INTO agent_run(id,root,sequence,data) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET sequence=excluded.sequence,data=excluded.data WHERE excluded.sequence>=agent_run.sequence AND excluded.root=agent_run.root",params![run.id,run.project,i64::try_from(run.sequence)?,serde_json::to_string(run)?])?;
        Ok(())
    }
    fn reconcile_interrupted_agents(&mut self) -> Result<()> {
        let rows = self
            .connection
            .prepare("SELECT data FROM agent_run")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for data in rows {
            let mut run: AgentRun = serde_json::from_str(&data)?;
            if run.status.active()
                || (run.status == AgentStatus::Prepared && run.grant.confirmed_at_ms > 0)
            {
                run.stage_verified = false;
                run.finish(AgentStatus::Interrupted,"Delegation was interrupted before a terminal handoff. Inspect retained staged files; work was not resumed automatically.");
                self.save_agent_run(&run)?;
            }
        }
        Ok(())
    }
    pub fn imports(&self, root: &str) -> Result<Vec<ImportTransaction>> {
        let rows = self
            .connection
            .prepare("SELECT data FROM import_group WHERE root=?1 ORDER BY rowid DESC LIMIT 30")?
            .query_map([root], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|s| serde_json::from_str(&s).context("Import journal is damaged"))
            .collect()
    }
    /// Commit all affected draft buffers and the inverse journal together.
    /// No live source file is written by an import transaction.
    pub fn commit_import(
        &mut self,
        workspace: &WorkspaceState,
        working: &[Document],
        group: &ImportTransaction,
    ) -> Result<()> {
        ensure!(
            workspace.root == group.project && !group.files.is_empty(),
            "Import workspace mismatch"
        );
        if group.status == ImportStatus::Applied {
            let count:i64=self.connection.query_row("SELECT COUNT(*) FROM import_group WHERE json_extract(data,'$.agent_run_id')=?1 AND id!=?2",params![group.agent_run_id,group.id],|row|row.get(0))?;
            ensure!(
                count == 0,
                "This delegation already has an import journal and cannot be replayed"
            );
        }
        let transaction = self.connection.transaction()?;
        let previous_sequence: Option<String> = transaction
            .query_row(
                "SELECT value FROM settings WHERE key='checkpoint_sequence'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        ensure!(
            previous_sequence
                .as_deref()
                .map(str::parse::<u64>)
                .transpose()?
                .is_none_or(|n| workspace.sequence > n),
            "The workspace advanced; prepare the import again"
        );
        let old: Option<String> = transaction
            .query_row(
                "SELECT data FROM import_group WHERE id=?1",
                [&group.id],
                |r| r.get(0),
            )
            .optional()?;
        match (group.status, old) {
            (ImportStatus::Applied, None) => {}
            (ImportStatus::Undone, Some(old)) => {
                let old: ImportTransaction = serde_json::from_str(&old)?;
                ensure!(
                    old.status == ImportStatus::Applied
                        && old.project == group.project
                        && old.agent_run_id == group.agent_run_id
                        && group.sequence == old.sequence + 1,
                    "Import has changed or was already undone"
                );
            }
            _ => anyhow::bail!("Import transaction cannot be replayed"),
        }
        let now = now_ms();
        transaction.execute("INSERT INTO workspace(root,data,opened) VALUES(?1,?2,?3) ON CONFLICT(root) DO UPDATE SET data=excluded.data,opened=excluded.opened",params![workspace.root,serde_json::to_string(workspace)?,now])?;
        transaction.execute("INSERT INTO settings(key,value) VALUES('checkpoint_sequence',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[workspace.sequence.to_string()])?;
        for document in working {
            if document.dirty() {
                transaction.execute("INSERT INTO draft(root,path,data) VALUES(?1,?2,?3) ON CONFLICT(root,path) DO UPDATE SET data=excluded.data",params![workspace.root,document.path,serde_json::to_string(document)?])?;
            } else {
                transaction.execute(
                    "DELETE FROM draft WHERE root=?1 AND path=?2",
                    params![workspace.root, document.path],
                )?;
            }
        }
        for file in &group.files {
            if file.after.is_none() {
                transaction.execute(
                    "DELETE FROM draft WHERE root=?1 AND path=?2",
                    params![workspace.root, file.path],
                )?;
            }
        }
        let data = serde_json::to_string(group)?;
        transaction.execute("INSERT INTO import_group(id,root,sequence,data) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET sequence=excluded.sequence,data=excluded.data",params![group.id,group.project,i64::try_from(group.sequence)?,data])?;
        transaction.execute(
            "INSERT INTO import_history(id,sequence,data) VALUES(?1,?2,?3)",
            params![
                group.id,
                i64::try_from(group.sequence)?,
                serde_json::to_string(group)?
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }
    pub fn save_guidance(&mut self, settings: &GuidanceSettings) -> Result<()> {
        let previous = self.guidance()?;
        ensure!(
            settings.sequence > previous.sequence
                || serde_json::to_string(settings)? == serde_json::to_string(&previous)?,
            "Newer guidance settings already exist"
        );
        self.connection.execute("INSERT INTO settings(key,value) VALUES('guidance',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[serde_json::to_string(settings)?])?;
        Ok(())
    }
    pub fn model_runs(&self, root: &str) -> Result<Vec<ModelRun>> {
        let rows = self
            .connection
            .prepare("SELECT data FROM model_run WHERE root=?1 ORDER BY rowid DESC LIMIT 40")?
            .query_map([root], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|s| serde_json::from_str(&s).context("Companion history is damaged"))
            .collect()
    }
    pub fn save_model_run(&mut self, run: &ModelRun) -> Result<()> {
        self.connection.execute("INSERT INTO model_run(id,root,sequence,data) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET sequence=excluded.sequence,data=excluded.data WHERE excluded.sequence>=model_run.sequence AND excluded.root=model_run.root",params![run.id,run.project,i64::try_from(run.sequence)?,serde_json::to_string(run)?])?;
        Ok(())
    }
    pub fn companion_thread(&self, root: &str, name: &str) -> Result<Option<ThreadBinding>> {
        let row: Option<String> = self
            .connection
            .query_row(
                "SELECT data FROM companion_thread WHERE root=?1 AND name=?2",
                params![root, name],
                |r| r.get(0),
            )
            .optional()?;
        row.map(|s| serde_json::from_str(&s).context("Conversation binding is damaged"))
            .transpose()
    }
    pub fn save_companion_thread(
        &mut self,
        root: &str,
        name: &str,
        binding: &ThreadBinding,
    ) -> Result<()> {
        self.connection.execute("INSERT INTO companion_thread(root,name,data) VALUES(?1,?2,?3) ON CONFLICT(root,name) DO UPDATE SET data=excluded.data",params![root,name,serde_json::to_string(binding)?])?;
        Ok(())
    }
    pub fn forget_companion_thread(&mut self, root: &str, name: &str) -> Result<()> {
        self.connection.execute(
            "DELETE FROM companion_thread WHERE root=?1 AND name=?2",
            params![root, name],
        )?;
        Ok(())
    }
    fn reconcile_interrupted_models(&mut self) -> Result<()> {
        let rows = self
            .connection
            .prepare("SELECT data FROM model_run")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for row in rows {
            let mut run: ModelRun =
                serde_json::from_str(&row).context("Companion run journal is damaged")?;
            if matches!(run.status, ModelStatus::Preparing | ModelStatus::Running) {
                run.finish(ModelStatus::Interrupted,"No terminal result was recorded before restart. Continue explicitly; this request was not replayed.");
                self.save_model_run(&run)?;
                if let Some(id) = run.thread_id {
                    self.connection.execute("DELETE FROM companion_thread WHERE root=?1 AND json_extract(data,'$.thread_id')=?2",params![run.project,id])?;
                }
            }
        }
        Ok(())
    }

    pub fn recent(&self) -> Result<Vec<String>> {
        Ok(self
            .connection
            .prepare("SELECT root FROM workspace ORDER BY opened DESC LIMIT 12")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn last_project(&self) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT value FROM settings WHERE key='last_project'",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn sequence(&self) -> Result<u64> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM settings WHERE key='checkpoint_sequence'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        value
            .map(|v| v.parse().context("Invalid recovery sequence"))
            .transpose()
            .map(|v| v.unwrap_or(0))
    }

    pub fn workspace(&self, root: &str) -> Result<Option<WorkspaceState>> {
        let text: Option<String> = self
            .connection
            .query_row("SELECT data FROM workspace WHERE root=?1", [root], |r| {
                r.get(0)
            })
            .optional()?;
        text.map(|text| {
            serde_json::from_str(&text)
                .context("Workspace metadata is damaged; existing data was preserved")
        })
        .transpose()
    }

    pub fn drafts(&self, root: &str) -> Result<Vec<Document>> {
        let rows = self
            .connection
            .prepare("SELECT data FROM draft WHERE root=?1 ORDER BY path")?
            .query_map([root], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|text| {
                serde_json::from_str(&text)
                    .context("A recovery draft is damaged; existing data was preserved")
            })
            .collect()
    }
    /// Retained closed drafts remain part of the working version. Live buffers
    /// win over their last checkpoint, including a clean buffer that was saved.
    pub fn working_documents(&self, root: &str, live: &[Document]) -> Result<Vec<Document>> {
        let mut documents = self
            .drafts(root)?
            .into_iter()
            .map(|d| (d.path.clone(), d))
            .collect::<std::collections::BTreeMap<_, _>>();
        documents.extend(live.iter().cloned().map(|d| (d.path.clone(), d)));
        Ok(documents.into_values().collect())
    }

    /// One transaction publishes workspace metadata and all supplied snapshots.
    /// Closed drafts not supplied remain available until explicitly saved/discarded.
    pub fn checkpoint(&mut self, workspace: &WorkspaceState, documents: &[Document]) -> Result<()> {
        let transaction = self.connection.transaction()?;
        let previous: Option<String> = transaction
            .query_row(
                "SELECT value FROM settings WHERE key='checkpoint_sequence'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if previous
            .as_deref()
            .map(str::parse::<u64>)
            .transpose()?
            .is_some_and(|sequence| sequence > workspace.sequence)
        {
            return Ok(());
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
        transaction.execute("INSERT INTO workspace(root,data,opened) VALUES(?1,?2,?3) ON CONFLICT(root) DO UPDATE SET data=excluded.data,opened=excluded.opened", params![workspace.root, serde_json::to_string(workspace)?, now])?;
        transaction.execute("INSERT INTO settings(key,value) VALUES('last_project',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [&workspace.root])?;
        transaction.execute("INSERT INTO settings(key,value) VALUES('checkpoint_sequence',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [workspace.sequence.to_string()])?;
        for document in documents {
            if document.dirty() {
                transaction.execute("INSERT INTO draft(root,path,data) VALUES(?1,?2,?3) ON CONFLICT(root,path) DO UPDATE SET data=excluded.data", params![workspace.root, document.path, serde_json::to_string(document)?])?;
            } else {
                transaction.execute(
                    "DELETE FROM draft WHERE root=?1 AND path=?2",
                    params![workspace.root, document.path],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn project_tools(&self, root: &str) -> Result<ProjectTools> {
        let data: Option<String> = self
            .connection
            .query_row(
                "SELECT data FROM project_tools WHERE root=?1",
                [root],
                |row| row.get(0),
            )
            .optional()?;
        data.map(|data| serde_json::from_str(&data).context("Project tool settings are damaged"))
            .transpose()
            .map(Option::unwrap_or_default)
    }
    pub fn save_project_tools(&mut self, root: &str, settings: &ProjectTools) -> Result<()> {
        self.connection.execute("INSERT INTO project_tools(root,data) VALUES(?1,?2) ON CONFLICT(root) DO UPDATE SET data=excluded.data", params![root, serde_json::to_string(settings)?])?;
        Ok(())
    }
    pub fn tasks(&self, root: &str) -> Result<Vec<TaskPlan>> {
        let rows = self
            .connection
            .prepare("SELECT data FROM task WHERE root=?1 ORDER BY updated DESC")?
            .query_map([root], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|data| serde_json::from_str(&data).context("Task data is damaged"))
            .collect()
    }
    pub fn save_task(&mut self, task: &TaskPlan) -> Result<()> {
        task.validate()?;
        let transaction = self.connection.transaction()?;
        let managed: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_session WHERE task_id=?1)",
            [&task.id],
            |row| row.get(0),
        )?;
        ensure!(
            !managed,
            "Managed tasks must be changed through their session"
        );
        let previous: Option<String> = transaction
            .query_row("SELECT data FROM task WHERE id=?1", [&task.id], |row| {
                row.get(0)
            })
            .optional()?;
        let data = serde_json::to_string(task)?;
        let revision = i64::try_from(task.revision)?;
        if let Some(previous) = previous {
            let old: TaskPlan = serde_json::from_str(&previous)?;
            ensure!(
                old.project == task.project
                    && old.revision <= task.revision
                    && (old.state_sequence < task.state_sequence || previous == data),
                "A newer task state already exists"
            );
            ensure!(
                old.revision != task.revision || old.definition_hash() == task.definition_hash(),
                "Plan changes need a new revision"
            );
        }
        transaction.execute("INSERT INTO task(id,root,revision,updated,data) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,updated=excluded.updated,data=excluded.data", params![task.id,task.project,revision,task.updated_at_ms,data])?;
        transaction.execute(
            "INSERT OR IGNORE INTO task_revision(id,revision,data) VALUES(?1,?2,?3)",
            params![task.id, revision, serde_json::to_string(task)?],
        )?;
        transaction.commit()?;
        Ok(())
    }
    pub fn checks(&self, root: &str) -> Result<Vec<CheckRecord>> {
        let rows = self
            .connection
            .prepare("SELECT data FROM check_run WHERE root=?1 ORDER BY rowid DESC LIMIT 30")?
            .query_map([root], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|data| serde_json::from_str(&data).context("Check receipt is damaged"))
            .collect()
    }
    pub fn save_check(&mut self, record: &CheckRecord) -> Result<()> {
        self.connection.execute("INSERT INTO check_run(id,root,sequence,data) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET sequence=excluded.sequence,data=excluded.data WHERE excluded.sequence>=check_run.sequence AND excluded.root=check_run.root", params![record.id,record.project,i64::try_from(record.sequence)?,serde_json::to_string(record)?])?;
        Ok(())
    }
    fn reconcile_interrupted_checks(&mut self) -> Result<()> {
        let rows = self
            .connection
            .prepare("SELECT data FROM check_run")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for data in rows {
            let mut record: CheckRecord = serde_json::from_str(&data)
                .context("Check journal is damaged; it was not discarded")?;
            if matches!(record.status, CheckStatus::Preparing | CheckStatus::Running) {
                record.finish(CheckStatus::Interrupted, "No terminal result was recorded before restart. This check was not automatically rerun; retained artifacts may contain completed work.");
                self.save_check(&record)?;
            }
        }
        Ok(())
    }
}

/// Host commands, replayed against the latest durable state inside one transaction.
/// Model proposals and ordinary messages have no approval or execution authority.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", deny_unknown_fields)]
pub enum SessionCommand {
    HumanMessage {
        text: String,
    },
    AssistantMessage {
        text: String,
        run_id: String,
    },
    AskQuestion {
        question: ClarificationQuestion,
        run_id: Option<String>,
    },
    AnswerQuestion {
        answer: ClarificationAnswer,
    },
    ProposeSpec {
        spec: SpecDocument,
        run_id: Option<String>,
    },
    ProposePlan {
        task: TaskPlan,
        run_id: Option<String>,
    },
    SaveSpec {
        spec: SpecDocument,
    },
    SavePlan {
        plan: PlanDocument,
    },
    ApproveSpec {
        document_ref: DocumentRef,
        approved_at_ms: i64,
    },
    ApprovePlan {
        document_ref: DocumentRef,
        approved_at_ms: i64,
    },
    StartImplementation,
    ResearchActivity {
        activity: ResearchActivity,
    },
    ResearchFindings {
        findings: Vec<ResearchFinding>,
    },
    RecordResearchReceipt {
        receipt: ResearchReceipt,
    },
    SaveWalkthrough {
        walkthrough: Walkthrough,
    },
    /// Storage primitive only: this records an immutable receipt, not cursor/step
    /// progression. The review and Next state machine must validate consumption.
    RecordActionTransition {
        transition: ConsumedActionTransition,
    },
    BeginOperation {
        operation: SessionOperation,
    },
    FinishOperation {
        operation_id: String,
        outcome: OperationOutcome,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionAuthorization {
    pub action: WorkflowAction,
    pub binding: WorkflowBinding,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionTransition {
    pub id: String,
    pub commands: Vec<SessionCommand>,
    /// Async work carries the durable operation ID. Terminal operations cannot
    /// publish new artifacts, but identical already-committed retries still work.
    pub operation_id: Option<String>,
    pub authorization: Option<SessionAuthorization>,
}

impl SessionTransition {
    pub fn new(id: impl Into<String>, commands: Vec<SessionCommand>) -> Self {
        Self {
            id: id.into(),
            commands,
            operation_id: None,
            authorization: None,
        }
    }
    pub fn for_operation(mut self, operation_id: impl Into<String>) -> Self {
        self.operation_id = Some(operation_id.into());
        self
    }
    pub fn with_authorization(mut self, action: WorkflowAction, binding: WorkflowBinding) -> Self {
        self.authorization = Some(SessionAuthorization { action, binding });
        self
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ComposerDraft {
    pub sequence: u64,
    pub text: String,
}

impl Store {
    /// Persist identity before the first model request, and select it in the project.
    pub fn create_session(&mut self, project: &str) -> Result<TaskSession> {
        let session = TaskSession::new(project.into())?;
        let transaction = self.connection.transaction()?;
        insert_session(&transaction, &session, None)?;
        select_session(&transaction, project, &session.id)?;
        transaction.commit()?;
        Ok(session)
    }

    pub fn sessions(&self, project: &str) -> Result<Vec<TaskSession>> {
        let rows = self
            .connection
            .prepare(
                "SELECT data FROM task_session WHERE root=?1 ORDER BY updated DESC,rowid DESC",
            )?
            .query_map([project], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|data| serde_json::from_str(&data).context("Session history is damaged"))
            .collect()
    }

    pub fn load_session(&self, session_id: &str) -> Result<Option<TaskSession>> {
        load_session(&self.connection, session_id)
    }

    pub fn selected_session(&self, project: &str) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT session_id FROM session_selection WHERE root=?1",
                [project],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn select_session(&mut self, project: &str, session_id: &str) -> Result<()> {
        select_session(&self.connection, project, session_id)
    }

    pub fn composer_draft(&self, session_id: &str) -> Result<ComposerDraft> {
        ensure_session(&self.connection, session_id)?;
        let row: Option<(i64, String)> = self
            .connection
            .query_row(
                "SELECT sequence,text FROM session_composer WHERE session_id=?1",
                [session_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(sequence, text)| {
            Ok(ComposerDraft {
                sequence: u64::try_from(sequence)?,
                text,
            })
        })
        .unwrap_or_else(|| Ok(ComposerDraft::default()))
    }

    /// Composer sequencing is independent of transcript and document approvals.
    pub fn save_composer_draft(
        &mut self,
        session_id: &str,
        expected_sequence: u64,
        text: &str,
    ) -> Result<u64> {
        ensure!(
            text.len() <= crate::MAX_HUMAN_MESSAGE_BYTES && !text.contains('\0'),
            "Composer draft exceeds 16 KiB or contains NUL"
        );
        let transaction = self.connection.transaction()?;
        ensure_session(&transaction, session_id)?;
        let sequence = u64::try_from(
            transaction
                .query_row(
                    "SELECT sequence FROM session_composer WHERE session_id=?1",
                    [session_id],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0),
        )?;
        ensure!(
            sequence == expected_sequence,
            "A newer composer draft already exists"
        );
        let next = sequence
            .checked_add(1)
            .context("Composer sequence exhausted")?;
        transaction.execute("INSERT INTO session_composer(session_id,sequence,text) VALUES(?1,?2,?3) ON CONFLICT(session_id) DO UPDATE SET sequence=excluded.sequence,text=excluded.text",params![session_id,i64::try_from(next)?,text])?;
        transaction.commit()?;
        Ok(next)
    }

    /// Newest page before the exclusive cursor, returned chronologically. No rows
    /// are pruned; callers page backward using the first returned sequence.
    pub fn conversation_events(
        &self,
        session_id: &str,
        before_sequence: Option<u64>,
        limit: u32,
    ) -> Result<Vec<ConversationEvent>> {
        ensure!(
            (1..=200).contains(&limit),
            "Read 1–200 conversation events per page"
        );
        ensure_session(&self.connection, session_id)?;
        let before = before_sequence
            .map(i64::try_from)
            .transpose()?
            .unwrap_or(i64::MAX);
        let rows = self.connection.prepare("SELECT data FROM conversation_event WHERE session_id=?1 AND sequence<?2 ORDER BY sequence DESC LIMIT ?3")?
            .query_map(params![session_id,before,limit],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .rev()
            .map(|data| serde_json::from_str(&data).context("Conversation history is damaged"))
            .collect()
    }

    pub fn document_revision(
        &self,
        session_id: &str,
        kind: WorkflowDocumentKind,
        revision: u64,
    ) -> Result<Option<DocumentProposal>> {
        let row: Option<String> = self.connection.query_row("SELECT data FROM document_revision WHERE session_id=?1 AND kind=?2 AND revision=?3",params![session_id,document_kind(kind),i64::try_from(revision)?],|r|r.get(0)).optional()?;
        row.map(|data| serde_json::from_str(&data).context("Document history is damaged"))
            .transpose()
    }

    pub fn research_receipt(
        &self,
        session_id: &str,
        receipt_id: &str,
    ) -> Result<Option<ResearchReceipt>> {
        let row: Option<String> = self
            .connection
            .query_row(
                "SELECT data FROM research_receipt WHERE session_id=?1 AND id=?2",
                params![session_id, receipt_id],
                |r| r.get(0),
            )
            .optional()?;
        row.map(|data| serde_json::from_str(&data).context("Research receipt is damaged"))
            .transpose()
    }

    pub fn action_transition(
        &self,
        session_id: &str,
        transition_id: &str,
    ) -> Result<Option<ConsumedActionTransition>> {
        let row: Option<String> = self
            .connection
            .query_row(
                "SELECT data FROM action_transition WHERE session_id=?1 AND id=?2",
                params![session_id, transition_id],
                |r| r.get(0),
            )
            .optional()?;
        row.map(|data| serde_json::from_str(&data).context("Action transition is damaged"))
            .transpose()
    }

    pub fn active_session_operation(&self, session_id: &str) -> Result<Option<SessionOperation>> {
        let row: Option<String> = self
            .connection
            .query_row(
                "SELECT data FROM session_operation WHERE session_id=?1 AND outcome IS NULL",
                [session_id],
                |r| r.get(0),
            )
            .optional()?;
        row.map(|data| serde_json::from_str(&data).context("Session operation is damaged"))
            .transpose()
    }

    pub fn session_operation_outcome(
        &self,
        operation_id: &str,
    ) -> Result<Option<OperationOutcome>> {
        let row: Option<String> = self
            .connection
            .query_row(
                "SELECT outcome FROM session_operation WHERE id=?1",
                [operation_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        row.map(|data| serde_json::from_str(&data).context("Operation outcome is damaged"))
            .transpose()
    }

    pub fn append_user_message(
        &mut self,
        session_id: &str,
        expected_sequence: u64,
        event_id: &str,
        text: &str,
    ) -> Result<TaskSession> {
        self.apply_session_transition(
            session_id,
            expected_sequence,
            &SessionTransition::new(
                event_id,
                vec![SessionCommand::HumanMessage { text: text.into() }],
            ),
        )
    }

    pub fn record_question_answer(
        &mut self,
        session_id: &str,
        expected_sequence: u64,
        event_id: &str,
        answer: ClarificationAnswer,
    ) -> Result<TaskSession> {
        self.apply_session_transition(
            session_id,
            expected_sequence,
            &SessionTransition::new(event_id, vec![SessionCommand::AnswerQuestion { answer }]),
        )
    }

    /// Retry is idempotent only for the same session, immutable ID and exact
    /// command content. An identical retry returns the latest state, never an
    /// obsolete snapshot. Expected sequence is deliberately not part of the ID.
    pub fn apply_session_transition(
        &mut self,
        session_id: &str,
        expected_sequence: u64,
        transition: &SessionTransition,
    ) -> Result<TaskSession> {
        stable_id(&transition.id)?;
        ensure!(
            (1..=64).contains(&transition.commands.len()),
            "Use 1–64 session commands in a transition"
        );
        let transaction = self.connection.transaction()?;
        let mut session = load_session(&transaction, session_id)?.context("Unknown session")?;
        let data = serde_json::to_string(transition)?;
        let previous: Option<(String, String)> = transaction
            .query_row(
                "SELECT session_id,data FROM session_transition WHERE id=?1",
                [&transition.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((owner, previous)) = previous {
            ensure!(
                owner == session_id && previous == data,
                "Transition ID is immutable"
            );
            return Ok(session);
        }
        ensure!(
            session.sequence == expected_sequence,
            "A newer session state already exists"
        );
        if let Some(authorization) = &transition.authorization {
            ensure!(
                session.binding_is_current(authorization.action, &authorization.binding),
                "Workflow binding is stale or unauthorized"
            );
        }
        for (index, command) in transition.commands.iter().enumerate() {
            if let Some(operation_id) = &transition.operation_id {
                stable_id(operation_id)?;
                let active: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM session_operation WHERE id=?1 AND session_id=?2 AND outcome IS NULL)",params![operation_id,session.id],|r|r.get(0))?;
                ensure!(
                    active,
                    "Operation is unknown or already terminal; late results cannot be applied"
                );
            }
            // Authorization remains current after earlier commands in the batch.
            if let Some(authorization) = &transition.authorization {
                ensure!(
                    session.binding_is_current(authorization.action, &authorization.binding),
                    "Workflow binding changed during transition"
                );
            }
            if let Some(mut event) = apply_session_command(&transaction, &mut session, command)? {
                event.id = if index == 0 {
                    transition.id.clone()
                } else {
                    crate::content_hash(
                        format!("{}:{}:{index}", session.id, transition.id).as_bytes(),
                    )
                };
                transaction.execute("INSERT INTO conversation_event(id,session_id,sequence,data) VALUES(?1,?2,?3,?4)",params![event.id,session.id,i64::try_from(event.sequence)?,serde_json::to_string(&event)?])?;
            }
        }
        let task_id = session
            .plan
            .as_ref()
            .map(|p| p.task.id.as_str())
            .or_else(|| session.legacy_task.as_ref().map(|l| l.task_id.as_str()));
        if let Some(plan) = &session.plan {
            persist_task_projection(&transaction, &plan.task)?;
        }
        transaction.execute(
            "UPDATE task_session SET sequence=?2,task_id=?3,updated=?4,data=?5 WHERE id=?1",
            params![
                session.id,
                i64::try_from(session.sequence)?,
                task_id,
                now_ms(),
                serde_json::to_string(&session)?
            ],
        )?;
        transaction.execute(
            "INSERT INTO session_transition(id,session_id,data) VALUES(?1,?2,?3)",
            params![transition.id, session.id, data],
        )?;
        transaction.commit()?;
        Ok(session)
    }

    /// Explicit adoption links original IDs and progress as historical evidence.
    /// It neither drafts missing documents nor grants approval/Start authority.
    pub fn adopt_legacy_task(&mut self, task_id: &str) -> Result<TaskSession> {
        let transaction = self.connection.transaction()?;
        if let Some(session_id) = transaction
            .query_row(
                "SELECT id FROM task_session WHERE task_id=?1",
                [task_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            return load_session(&transaction, &session_id)?.context("Linked session is missing");
        }
        let data: String = transaction
            .query_row("SELECT data FROM task WHERE id=?1", [task_id], |r| r.get(0))
            .optional()?
            .context("Unknown legacy task")?;
        let task: TaskPlan = serde_json::from_str(&data)?;
        let review_ids = transaction.prepare("SELECT id FROM model_run WHERE json_extract(data,'$.request.task.id')=?1 AND json_extract(data,'$.request.action')='Review' ORDER BY rowid")?
            .query_map([task_id],|r|r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
        let mut session = TaskSession::new(task.project.clone())?;
        session.legacy_task = Some(LegacyTaskLink {
            task_id: task.id.clone(),
            revision: task.revision,
            state_sequence: task.state_sequence,
            review_ids,
        });
        insert_session(&transaction, &session, Some(&task.id))?;
        transaction.execute(
            "INSERT INTO legacy_task_link(session_id,task_id,data) VALUES(?1,?2,?3)",
            params![session.id, task.id, data],
        )?;
        select_session(&transaction, &session.project, &session.id)?;
        transaction.commit()?;
        Ok(session)
    }

    pub fn legacy_task(&self, session_id: &str) -> Result<Option<TaskPlan>> {
        let data: Option<String> = self
            .connection
            .query_row(
                "SELECT data FROM legacy_task_link WHERE session_id=?1",
                [session_id],
                |r| r.get(0),
            )
            .optional()?;
        data.map(|data| serde_json::from_str(&data).context("Legacy task history is damaged"))
            .transpose()
    }

    fn reconcile_interrupted_session_operations(&mut self) -> Result<()> {
        let operations = self
            .connection
            .prepare("SELECT data FROM session_operation WHERE outcome IS NULL")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for data in operations {
            let operation: SessionOperation = serde_json::from_str(&data)?;
            let session = self
                .load_session(&operation.session_id)?
                .context("Operation session is missing")?;
            self.apply_session_transition(&session.id,session.sequence,&SessionTransition::new(new_id(),vec![SessionCommand::FinishOperation {
                operation_id: operation.id,
                outcome: OperationOutcome::Interrupted {detail:"No terminal outcome was recorded before restart. Continue explicitly; no model turn or inspection was replayed.".into()},
            }]))?;
        }
        Ok(())
    }
}

fn load_session(connection: &Connection, id: &str) -> Result<Option<TaskSession>> {
    let data: Option<String> = connection
        .query_row("SELECT data FROM task_session WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    data.map(|data| {
        serde_json::from_str(&data).context("Session state is damaged; existing data was preserved")
    })
    .transpose()
}
fn ensure_session(connection: &Connection, id: &str) -> Result<()> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_session WHERE id=?1)",
        [id],
        |r| r.get(0),
    )?;
    ensure!(exists, "Unknown session");
    Ok(())
}
fn insert_session(
    connection: &Connection,
    session: &TaskSession,
    task_id: Option<&str>,
) -> Result<()> {
    connection.execute(
        "INSERT INTO task_session(id,root,sequence,task_id,updated,data) VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            session.id,
            session.project,
            i64::try_from(session.sequence)?,
            task_id,
            now_ms(),
            serde_json::to_string(session)?
        ],
    )?;
    Ok(())
}
fn select_session(connection: &Connection, project: &str, session_id: &str) -> Result<()> {
    let session = load_session(connection, session_id)?.context("Unknown session")?;
    ensure!(
        session.project == project,
        "Session belongs to another project"
    );
    connection.execute("INSERT INTO session_selection(root,session_id) VALUES(?1,?2) ON CONFLICT(root) DO UPDATE SET session_id=excluded.session_id",params![project,session_id])?;
    Ok(())
}
fn document_kind(kind: WorkflowDocumentKind) -> &'static str {
    match kind {
        WorkflowDocumentKind::Spec => "spec",
        WorkflowDocumentKind::Plan => "plan",
    }
}
fn persist_document(
    connection: &Connection,
    session_id: &str,
    document: DocumentProposal,
) -> Result<()> {
    let (kind, revision) = match &document {
        DocumentProposal::Spec(spec) => ("spec", spec.revision),
        DocumentProposal::Plan(plan) => ("plan", plan.revision),
    };
    let data = serde_json::to_string(&document)?;
    let previous: Option<String> = connection
        .query_row(
            "SELECT data FROM document_revision WHERE session_id=?1 AND kind=?2 AND revision=?3",
            params![session_id, kind, i64::try_from(revision)?],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(previous) = previous {
        ensure!(previous == data, "Document revision is immutable");
    } else {
        connection.execute(
            "INSERT INTO document_revision(session_id,kind,revision,data) VALUES(?1,?2,?3,?4)",
            params![session_id, kind, i64::try_from(revision)?, data],
        )?;
    }
    Ok(())
}
fn persist_task_projection(connection: &Connection, task: &TaskPlan) -> Result<()> {
    task.validate()?;
    let previous: Option<String> = connection
        .query_row("SELECT data FROM task WHERE id=?1", [&task.id], |r| {
            r.get(0)
        })
        .optional()?;
    let data = serde_json::to_string(task)?;
    if let Some(previous) = previous {
        let old: TaskPlan = serde_json::from_str(&previous)?;
        ensure!(
            old.project == task.project
                && old.revision <= task.revision
                && (old.state_sequence < task.state_sequence || previous == data),
            "A newer task projection already exists"
        );
        ensure!(
            old.revision != task.revision || old.definition_hash() == task.definition_hash(),
            "Plan changes need a new revision"
        );
    }
    connection.execute("INSERT INTO task(id,root,revision,updated,data) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,updated=excluded.updated,data=excluded.data",params![task.id,task.project,i64::try_from(task.revision)?,task.updated_at_ms,data])?;
    // Progress updates never rewrite a saved definition/history row.
    connection.execute(
        "INSERT OR IGNORE INTO task_revision(id,revision,data) VALUES(?1,?2,?3)",
        params![
            task.id,
            i64::try_from(task.revision)?,
            serde_json::to_string(task)?
        ],
    )?;
    Ok(())
}

fn apply_session_command(
    connection: &Connection,
    session: &mut TaskSession,
    command: &SessionCommand,
) -> Result<Option<ConversationEvent>> {
    let event = match command {
        SessionCommand::HumanMessage { text } => session.append_human_message(text.clone())?,
        SessionCommand::AssistantMessage { text, run_id } => {
            stable_id(run_id)?;
            session.append_assistant_message(text.clone(), run_id.clone())?
        }
        SessionCommand::AskQuestion { question, run_id } => {
            session.ask_question(question.clone(), run_id.clone())?
        }
        SessionCommand::AnswerQuestion { answer } => session.answer_question(answer.clone())?,
        SessionCommand::ProposeSpec { spec, run_id } => {
            session.propose_spec(spec.clone(), run_id.clone())?
        }
        SessionCommand::ProposePlan { task, run_id } => {
            session.propose_plan(task.clone(), run_id.clone())?
        }
        SessionCommand::SaveSpec { spec } => {
            let Some(event) = session.save_spec(spec.clone())? else {
                return Ok(None);
            };
            persist_document(
                connection,
                &session.id,
                DocumentProposal::Spec(session.spec.clone().context("Saved spec is missing")?),
            )?;
            event
        }
        SessionCommand::SavePlan { plan } => {
            let owner: Option<String> = connection
                .query_row(
                    "SELECT id FROM task_session WHERE task_id=?1",
                    [&plan.task.id],
                    |r| r.get(0),
                )
                .optional()?;
            let exists: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM task WHERE id=?1)",
                [&plan.task.id],
                |r| r.get(0),
            )?;
            ensure!(
                owner.as_ref().is_none_or(|id| id == &session.id)
                    && (!exists || owner.as_ref() == Some(&session.id)),
                "Task belongs to another session or needs explicit legacy adoption"
            );
            let Some(event) = session.save_plan(plan.clone())? else {
                return Ok(None);
            };
            persist_document(
                connection,
                &session.id,
                DocumentProposal::Plan(session.plan.clone().context("Saved plan is missing")?),
            )?;
            event
        }
        SessionCommand::ApproveSpec {
            document_ref,
            approved_at_ms,
        } => session.approve_spec(document_ref, *approved_at_ms)?,
        SessionCommand::ApprovePlan {
            document_ref,
            approved_at_ms,
        } => session.approve_plan(document_ref, *approved_at_ms)?,
        SessionCommand::StartImplementation => session.start_implementation()?,
        SessionCommand::ResearchActivity { activity } => {
            bounded_structure(activity)?;
            session.event(
                ConversationPayload::ResearchActivity {
                    activity: activity.clone(),
                },
                None,
            )?
        }
        SessionCommand::ResearchFindings { findings } => {
            bounded_structure(findings)?;
            let receipts = connection
                .prepare("SELECT data FROM research_receipt WHERE session_id=?1")?
                .query_map([&session.id], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .map(|data| serde_json::from_str::<ResearchReceipt>(&data))
                .collect::<serde_json::Result<Vec<_>>>()?;
            for finding in findings {
                finding.validate(&receipts)?;
            }
            session.event(
                ConversationPayload::ResearchFindings {
                    findings: findings.clone(),
                },
                None,
            )?
        }
        SessionCommand::RecordResearchReceipt { receipt } => {
            receipt.validate()?;
            let data = serde_json::to_string(receipt)?;
            let previous: Option<(String, String)> = connection
                .query_row(
                    "SELECT session_id,data FROM research_receipt WHERE id=?1",
                    [&receipt.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((owner, previous)) = previous {
                ensure!(
                    owner == session.id && previous == data,
                    "Research receipt ID is immutable"
                );
                return Ok(None);
            }
            connection.execute(
                "INSERT INTO research_receipt(id,session_id,data) VALUES(?1,?2,?3)",
                params![receipt.id, session.id, data],
            )?;
            session.event(
                ConversationPayload::ResearchReceiptRecorded {
                    receipt_id: receipt.id.clone(),
                },
                None,
            )?
        }
        SessionCommand::SaveWalkthrough { walkthrough } => {
            session.authorize(WorkflowAction::PrepareWalkthrough)?;
            let spec = session.spec.as_ref().context("Missing spec")?;
            let plan = session.plan.as_ref().context("Missing plan")?;
            walkthrough.validate(spec, plan)?;
            ensure!(
                walkthrough.generation == session.execution_generation,
                "Walkthrough generation is stale"
            );
            if let Some(old) = session.walkthroughs.iter().find(|w| w.id == walkthrough.id) {
                ensure!(
                    serde_json::to_string(old)? == serde_json::to_string(walkthrough)?,
                    "Walkthrough ID is immutable"
                );
                return Ok(None);
            }
            ensure!(
                walkthrough.cursor == 0,
                "A new walkthrough cannot contain consumed actions"
            );
            session.walkthroughs.push(walkthrough.clone());
            session.event(
                ConversationPayload::WalkthroughPrepared {
                    walkthrough_id: walkthrough.id.clone(),
                },
                None,
            )?
        }
        SessionCommand::RecordActionTransition { transition } => {
            stable_id(&transition.id)?;
            stable_id(&transition.review_id)?;
            stable_id(&transition.binding.walkthrough_id)?;
            stable_id(&transition.binding.step_id)?;
            stable_id(&transition.binding.action_id)?;
            bounded_structure(transition)?;
            let data = serde_json::to_string(transition)?;
            let previous: Option<(String, String)> = connection
                .query_row(
                    "SELECT session_id,data FROM action_transition WHERE id=?1",
                    [&transition.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((owner, previous)) = previous {
                ensure!(
                    owner == session.id && previous == data,
                    "Action transition ID is immutable"
                );
                return Ok(None);
            }
            ensure!(
                session.binding_is_current(
                    WorkflowAction::AdvanceAction,
                    &transition.binding.workflow
                ),
                "Action workflow binding is stale"
            );
            ensure!(
                transition.to_cursor
                    == transition
                        .from_cursor
                        .checked_add(1)
                        .context("Action cursor exhausted")?,
                "Action transition must name one advancement"
            );
            connection.execute("INSERT INTO action_transition(id,session_id,walkthrough_id,action_id,review_id,data) VALUES(?1,?2,?3,?4,?5,?6)",params![transition.id,session.id,transition.binding.walkthrough_id,transition.binding.action_id,transition.review_id,data])?;
            session.event(
                ConversationPayload::ActionTransitionRecorded {
                    transition_id: transition.id.clone(),
                },
                None,
            )?
        }
        SessionCommand::BeginOperation { operation } => {
            stable_id(&operation.id)?;
            ensure!(
                operation.session_id == session.id,
                "Operation belongs to another session"
            );
            connection.execute(
                "INSERT INTO session_operation(id,session_id,data,outcome) VALUES(?1,?2,?3,NULL)",
                params![operation.id, session.id, serde_json::to_string(operation)?],
            )?;
            session.event(
                ConversationPayload::OperationStarted {
                    operation: operation.clone(),
                },
                None,
            )?
        }
        SessionCommand::FinishOperation {
            operation_id,
            outcome,
        } => {
            stable_id(operation_id)?;
            let detail = match outcome {
                OperationOutcome::Completed { summary } => summary,
                OperationOutcome::Cancelled { detail }
                | OperationOutcome::Interrupted { detail }
                | OperationOutcome::Failed { detail } => detail,
            };
            bounded_text(
                detail,
                crate::MAX_STRUCTURED_RESPONSE_BYTES,
                "Operation outcome",
            )?;
            let updated = connection.execute("UPDATE session_operation SET outcome=?3 WHERE id=?1 AND session_id=?2 AND outcome IS NULL",params![operation_id,session.id,serde_json::to_string(outcome)?])?;
            ensure!(updated == 1, "Operation is unknown or already terminal");
            session.event(
                ConversationPayload::OperationOutcome {
                    operation_id: operation_id.clone(),
                    outcome: outcome.clone(),
                },
                None,
            )?
        }
    };
    Ok(Some(event))
}
