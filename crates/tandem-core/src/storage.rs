use crate::{Document, Project, ProjectTools, now_ms};
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
    pub(crate) connection: Connection,
    // Drop the connection before releasing the state-directory lock.
    _lock: StoreLock,
    pub directory: PathBuf,
}

struct StoreLock(File);

impl Drop for StoreLock {
    fn drop(&mut self) {
        // A concurrent fork can inherit this descriptor before CLOEXEC runs.
        // Closing only our descriptor would let that child extend the lock
        // beyond the owning Store's lifetime.
        let _ = self.0.unlock();
    }
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
        let lock = StoreLock(lock);
        let mut connection = Connection::open(directory.join("workspace.sqlite"))?;
        connection.busy_timeout(std::time::Duration::from_secs(2))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        ensure!(
            version <= 6,
            "This workspace state belongs to a newer Tandem version; it was not modified"
        );
        if version > 0 && version < 6 {
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
            CREATE TABLE IF NOT EXISTS chat_task(id TEXT PRIMARY KEY, root TEXT NOT NULL, updated INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS chat_selection(root TEXT PRIMARY KEY, task_id TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS chat_item(id INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT NOT NULL, created INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS chat_item_task ON chat_item(task_id, id);
            CREATE TABLE IF NOT EXISTS chat_guidance(task_id TEXT NOT NULL, step INTEGER NOT NULL, level TEXT NOT NULL, data TEXT NOT NULL, PRIMARY KEY(task_id, step, level));
            CREATE TABLE IF NOT EXISTS chat_change_set(id TEXT PRIMARY KEY, task_id TEXT NOT NULL, created INTEGER NOT NULL, data TEXT NOT NULL);
            PRAGMA user_version=6;")?;
        migration.commit()?;
        Ok(Self {
            connection,
            _lock: lock,
            directory: directory.to_path_buf(),
        })
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
}
