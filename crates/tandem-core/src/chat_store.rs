//! Storage for the Claude chat: the schema 6 `chat_*` tables and the `chat_prefs` setting.
use crate::chat::{
    ChangeSet, ChangeSetStatus, ChatItem, ChatPrefs, ChatTask, Guidance, HelpLevel, ItemKind,
    TaskSummary,
};
use crate::{Store, now_ms};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::de::DeserializeOwned;

impl Store {
    /// The saved chat preferences, or the defaults when none were saved.
    pub fn chat_prefs(&self) -> Result<ChatPrefs> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM settings WHERE key='chat_prefs'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        value
            .map(|value| decode(&value, "Chat preferences are damaged"))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    pub fn save_chat_prefs(&mut self, prefs: &ChatPrefs) -> Result<()> {
        self.connection.execute(
            "INSERT INTO settings(key,value) VALUES('chat_prefs',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [serde_json::to_string(prefs)?],
        )?;
        Ok(())
    }

    /// Stores a new task for `project` and selects it there.
    pub fn create_chat_task(&mut self, project: &str) -> Result<ChatTask> {
        let task = ChatTask::new(project);
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO chat_task(id,root,updated,data) VALUES(?1,?2,?3,?4)",
            params![
                task.id,
                task.project,
                task.updated_at_ms,
                serde_json::to_string(&task)?
            ],
        )?;
        select_chat_task(&transaction, project, &task.id)?;
        transaction.commit()?;
        Ok(task)
    }

    /// Saves a task created earlier in this store, stamping it as updated now.
    pub fn save_chat_task(&mut self, task: &ChatTask) -> Result<()> {
        let root = chat_task_root(&self.connection, &task.id)?
            .context("This chat task was never created, so it was not saved")?;
        ensure!(
            root == task.project,
            "A chat task cannot move to another project"
        );
        let mut stored = task.clone();
        stored.updated_at_ms = now_ms();
        self.connection.execute(
            "UPDATE chat_task SET updated=?2,data=?3 WHERE id=?1",
            params![
                stored.id,
                stored.updated_at_ms,
                serde_json::to_string(&stored)?
            ],
        )?;
        Ok(())
    }

    pub fn chat_task(&self, id: &str) -> Result<Option<ChatTask>> {
        let data: Option<String> = self
            .connection
            .query_row("SELECT data FROM chat_task WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        data.map(|data| decode(&data, "Chat task is damaged"))
            .transpose()
    }

    /// The project's tasks, most recently updated first.
    pub fn chat_tasks(&self, project: &str) -> Result<Vec<TaskSummary>> {
        let rows = self
            .connection
            .prepare("SELECT data FROM chat_task WHERE root=?1 ORDER BY updated DESC,rowid DESC")?
            .query_map([project], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.iter()
            .map(|data| {
                let task: ChatTask = decode(data, "Chat task is damaged")?;
                Ok(TaskSummary {
                    id: task.id,
                    title: task.title,
                    phase: task.phase,
                    updated_at_ms: task.updated_at_ms,
                })
            })
            .collect()
    }

    pub fn selected_chat_task(&self, project: &str) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT task_id FROM chat_selection WHERE root=?1",
                [project],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn select_chat_task(&mut self, project: &str, id: &str) -> Result<()> {
        select_chat_task(&self.connection, project, id)
    }

    pub fn append_chat_item(&mut self, task_id: &str, kind: &ItemKind) -> Result<ChatItem> {
        let created_at_ms = now_ms();
        self.connection.execute(
            "INSERT INTO chat_item(task_id,created,data) VALUES(?1,?2,?3)",
            params![task_id, created_at_ms, serde_json::to_string(kind)?],
        )?;
        Ok(ChatItem {
            id: self.connection.last_insert_rowid(),
            created_at_ms,
            kind: kind.clone(),
        })
    }

    /// Replaces the content of one item, keeping its id and creation time.
    pub fn update_chat_item(&mut self, id: i64, kind: &ItemKind) -> Result<()> {
        let changed = self.connection.execute(
            "UPDATE chat_item SET data=?2 WHERE id=?1",
            params![id, serde_json::to_string(kind)?],
        )?;
        ensure!(changed == 1, "Unknown chat item");
        Ok(())
    }

    /// The task's items, oldest first.
    pub fn chat_items(&self, task_id: &str) -> Result<Vec<ChatItem>> {
        let rows = self
            .connection
            .prepare("SELECT id,created,data FROM chat_item WHERE task_id=?1 ORDER BY id")?
            .query_map([task_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, created_at_ms, data)| {
                Ok(ChatItem {
                    id,
                    created_at_ms,
                    kind: decode(&data, "Chat history is damaged")?,
                })
            })
            .collect()
    }

    /// Stores guidance under (task, guidance.step, guidance.level), replacing any earlier one.
    pub fn save_chat_guidance(&mut self, task_id: &str, guidance: &Guidance) -> Result<()> {
        self.connection.execute(
            "INSERT INTO chat_guidance(task_id,step,level,data) VALUES(?1,?2,?3,?4) ON CONFLICT(task_id,step,level) DO UPDATE SET data=excluded.data",
            params![
                task_id,
                i64::try_from(guidance.step)?,
                guidance.level.id(),
                serde_json::to_string(guidance)?
            ],
        )?;
        Ok(())
    }

    pub fn chat_guidance(
        &self,
        task_id: &str,
        step: usize,
        level: HelpLevel,
    ) -> Result<Option<Guidance>> {
        let data: Option<String> = self
            .connection
            .query_row(
                "SELECT data FROM chat_guidance WHERE task_id=?1 AND step=?2 AND level=?3",
                params![task_id, i64::try_from(step)?, level.id()],
                |r| r.get(0),
            )
            .optional()?;
        data.map(|data| decode(&data, "Saved step guidance is damaged"))
            .transpose()
    }

    pub fn delete_chat_guidance(
        &mut self,
        task_id: &str,
        step: usize,
        level: HelpLevel,
    ) -> Result<()> {
        self.connection.execute(
            "DELETE FROM chat_guidance WHERE task_id=?1 AND step=?2 AND level=?3",
            params![task_id, i64::try_from(step)?, level.id()],
        )?;
        Ok(())
    }

    /// Inserts the change set, or replaces the one with the same id.
    pub fn save_change_set(&mut self, set: &ChangeSet) -> Result<()> {
        self.connection.execute(
            "INSERT INTO chat_change_set(id,task_id,created,data) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET task_id=excluded.task_id,created=excluded.created,data=excluded.data",
            params![
                set.id,
                set.task_id,
                set.created_at_ms,
                serde_json::to_string(set)?
            ],
        )?;
        Ok(())
    }

    /// The task's change sets, oldest first.
    pub fn change_sets(&self, task_id: &str) -> Result<Vec<ChangeSet>> {
        let rows = self
            .connection
            .prepare("SELECT data FROM chat_change_set WHERE task_id=?1 ORDER BY created,rowid")?
            .query_map([task_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.iter()
            .map(|data| decode(data, "Takeover changes are damaged"))
            .collect()
    }

    /// Change sets still `Open` in any task of the project, oldest first.
    pub fn open_change_sets(&self, project: &str) -> Result<Vec<ChangeSet>> {
        let rows = self
            .connection
            .prepare(
                "SELECT s.data FROM chat_change_set s JOIN chat_task t ON t.id=s.task_id WHERE t.root=?1 ORDER BY s.created,s.rowid",
            )?
            .query_map([project], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut open = Vec::new();
        for data in &rows {
            let set: ChangeSet = decode(data, "Takeover changes are damaged")?;
            if set.status == ChangeSetStatus::Open {
                open.push(set);
            }
        }
        Ok(open)
    }
}

fn decode<T: DeserializeOwned>(data: &str, damaged: &'static str) -> Result<T> {
    serde_json::from_str(data).context(damaged)
}

fn chat_task_root(connection: &Connection, id: &str) -> Result<Option<String>> {
    Ok(connection
        .query_row("SELECT root FROM chat_task WHERE id=?1", [id], |r| r.get(0))
        .optional()?)
}

fn select_chat_task(connection: &Connection, project: &str, id: &str) -> Result<()> {
    let root = chat_task_root(connection, id)?.context("Unknown chat task")?;
    ensure!(root == project, "This chat task belongs to another project");
    connection.execute(
        "INSERT INTO chat_selection(root,task_id) VALUES(?1,?2) ON CONFLICT(root) DO UPDATE SET task_id=excluded.task_id",
        params![project, id],
    )?;
    Ok(())
}
