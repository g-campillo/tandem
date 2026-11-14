use crate::{
    AgentRun, AgentStatus, CheckRecord, CheckStatus, Document, GuidanceSettings, ImportStatus,
    ImportTransaction, ModelRun, ModelStatus, Project, ProjectTools, TaskPlan, ThreadBinding,
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
            version <= 4,
            "This workspace state belongs to a newer Tandem version; it was not modified"
        );
        if version > 0 && version < 4 {
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
            PRAGMA user_version=4;")?;
        migration.commit()?;
        let mut store = Self {
            connection,
            _lock: lock,
            directory: directory.to_path_buf(),
        };
        store.reconcile_interrupted_checks()?;
        store.reconcile_interrupted_models()?;
        store.reconcile_interrupted_agents()?;
        Ok(store)
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
