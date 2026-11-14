use super::*;
use gpui_kit::component::input::TextareaState;
use serde_json::{Value, json};
use std::rc::Rc;
use tandem_checks::{CheckEnvironment, Recipe};
use tandem_lsp::{LanguageDocument, LanguageManager, LanguageStamp, ServerConfig};

#[derive(Clone, Copy, PartialEq)]
pub enum Panel {
    Tools,
    Problems,
    Checks,
    Task,
    Companion,
    Agent,
    Health,
}
#[derive(Clone)]
pub struct Navigation {
    pub path: String,
    pub line: usize,
    pub label: String,
}
pub struct Development {
    pub visible: bool,
    pub panel: Panel,
    pub scroll: ScrollHandle,
    pub settings: ProjectTools,
    pub rust: Option<RustTools>,
    pub python: Option<PythonTools>,
    pub python_error: String,
    pub python_path: Entity<InputState>,
    pub python_language: Option<LanguageManager>,
    pub python_health: String,
    pub environment: Option<CheckEnvironment>,
    pub recipes: Vec<Recipe>,
    pub discovering: bool,
    pub tool_error: String,
    pub check_error: String,
    pub language: Option<LanguageManager>,
    pub health: String,
    pub last_edit: Instant,
    pub diagnostic_busy: bool,
    pub diagnostic_key: String,
    pub diagnostic_stamp: Option<LanguageStamp>,
    pub problems: Vec<Navigation>,
    pub navigation: Vec<Navigation>,
    pub navigation_stamp: Option<LanguageStamp>,
    pub check_cancel: Option<Arc<AtomicBool>>,
    pub check_updates: Option<Arc<Mutex<Option<CheckRecord>>>>,
    pub checks: Vec<CheckRecord>,
    pub selected_check: Option<String>,
    pub pending_recipe: Option<String>,
    pub tasks: Vec<TaskPlan>,
    pub selected_task: Option<String>,
    pub editing_task: Option<String>,
    pub task_editor: bool,
    pub goal: Entity<InputState>,
    pub criteria: Entity<TextareaState>,
    pub steps: Entity<TextareaState>,
}
impl Development {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        Self {
            visible: true,
            panel: Panel::Companion,
            scroll: ScrollHandle::default(),
            settings: ProjectTools::default(),
            rust: None,
            python: None,
            python_error: String::new(),
            python_path: cx
                .new(|cx| InputState::new(window, cx).placeholder("Optional absolute Python path")),
            python_language: None,
            python_health: "Python tools off".into(),
            environment: None,
            recipes: vec![],
            discovering: false,
            tool_error: String::new(),
            check_error: String::new(),
            language: None,
            health: "Language tools off".into(),
            last_edit: Instant::now(),
            diagnostic_busy: false,
            diagnostic_key: String::new(),
            diagnostic_stamp: None,
            problems: vec![],
            navigation: vec![],
            navigation_stamp: None,
            check_cancel: None,
            check_updates: None,
            checks: vec![],
            selected_check: None,
            pending_recipe: None,
            tasks: vec![],
            selected_task: None,
            editing_task: None,
            task_editor: false,
            goal: cx.new(|cx| InputState::new(window, cx).placeholder("What should change?")),
            criteria: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(3)
                    .placeholder("One acceptance criterion per line")
            }),
            steps: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(5)
                    .placeholder("One meaningful step per line")
            }),
        }
    }
    pub fn reset(
        &mut self,
        settings: ProjectTools,
        tasks: Vec<TaskPlan>,
        checks: Vec<CheckRecord>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let visible = self.visible;
        *self = Self::new(window, cx);
        self.visible = visible;
        self.settings = settings;
        self.selected_task = tasks.first().map(|t| t.id.clone());
        self.tasks = tasks;
        self.selected_check = checks.first().map(|c| c.id.clone());
        self.checks = checks;
    }
}
impl Drop for Development {
    fn drop(&mut self) {
        if let Some(language) = &self.language {
            language.stop();
        }
        if let Some(language) = &self.python_language {
            language.stop();
        }
        if let Some(cancel) = &self.check_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Workspace {
    pub(super) fn discover_tools(&mut self, cx: &mut Context<Self>) {
        if self.dev.discovering {
            return;
        }
        let Some(project) = self.project.clone() else {
            return;
        };
        let runs = PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join("Library/Caches/Tandem/checks");
        self.dev.discovering = true;
        self.dev.tool_error.clear();
        self.dev.check_error.clear();
        let epoch = self.epoch;
        let selected_python = self.dev.settings.python_interpreter.clone();
        let operation = cx.background_spawn(async move {
            let rust = RustTools::discover(&project.root);
            let python = PythonTools::discover(&project.root, selected_python.as_deref());
            let checks = CheckEnvironment::discover_with_python(
                &project.root,
                runs,
                selected_python.as_deref(),
            )
            .map(|env| {
                let ids = if project.root.join("Cargo.toml").is_file() {
                    vec![
                        "cargo-check",
                        "cargo-test",
                        "python-unittest",
                        "python-pytest",
                        "python-benchmark",
                    ]
                } else {
                    vec!["python-unittest", "python-pytest", "python-benchmark"]
                };
                (
                    ids.into_iter()
                        .filter_map(|id| env.recipe(id).ok())
                        .collect::<Vec<_>>(),
                    env,
                )
            });
            (rust, python, checks)
        });
        cx.spawn(async move |view, cx| {
            let (rust, python, checks) = operation.await;
            let _ = view.update(cx, |view, cx| {
                if view.epoch != epoch {
                    return;
                }
                view.dev.discovering = false;
                match rust {
                    Ok(tools) => view.dev.rust = Some(tools),
                    Err(error) => {
                        view.dev.rust = None;
                        view.dev.tool_error = error.to_string();
                    }
                }
                match checks {
                    Ok((recipes, env)) => {
                        view.dev.recipes = recipes;
                        view.dev.environment = Some(env);
                    }
                    Err(error) => {
                        view.dev.environment = None;
                        view.dev.check_error = error.to_string();
                    }
                }
                match python {
                    Ok(tools) => {
                        view.dev.python = Some(tools);
                        view.dev.python_error.clear();
                    }
                    Err(e) => {
                        view.dev.python = None;
                        view.dev.python_error = e.to_string();
                    }
                }
                view.start_language(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn authorize_language(&mut self, cx: &mut Context<Self>) {
        let Some(tools) = self.dev.rust.as_ref() else {
            return;
        };
        let Some(project) = &self.project else {
            return;
        };
        let mut settings = self.dev.settings.clone();
        settings.trusted_fingerprint = Some(tools.fingerprint.clone());
        let result = self
            .store
            .lock()
            .unwrap()
            .save_project_tools(project.key(), &settings);
        match result {
            Ok(()) => {
                self.dev.settings = settings;
                self.start_language(cx);
            }
            Err(e) => self.message(format!("Could not save tool trust: {e}"), true, cx),
        }
    }
    fn start_language(&mut self, cx: &mut Context<Self>) {
        self.stop_language(cx);
        let Some(project) = &self.project else {
            return;
        };
        let cache = self
            .store
            .lock()
            .unwrap()
            .directory
            .join("language-cache")
            .join(content_hash(project.key().as_bytes()));
        if let Some(tools) = &self.dev.rust
            && self.dev.settings.trusted_fingerprint.as_ref() == Some(&tools.fingerprint)
        {
            self.dev.language = Some(LanguageManager::start(ServerConfig {
                python: None,
                binary: tools.rust_analyzer.clone(),
                toolchain: tools.toolchain.clone(),
                project: project.root.clone(),
                cache: cache.join("rust"),
            }));
        }
        if let Some(tools) = &self.dev.python
            && self.dev.settings.python_trusted_fingerprint.as_ref() == Some(&tools.fingerprint)
            && let (Some(node), Some(script)) = (&tools.node, &tools.server)
        {
            self.dev.python_language = Some(LanguageManager::start(ServerConfig {
                binary: node.clone(),
                toolchain: tools.prefix.clone(),
                project: project.root.clone(),
                cache: cache.join("python"),
                python: Some(tandem_lsp::PythonServer {
                    script: script.clone(),
                    interpreter: tools.interpreter.clone(),
                }),
            }));
        }
        self.dev.diagnostic_key.clear();
        if let Some(watcher) = self._watcher.as_mut()
            && let Some(python) = &self.dev.python
        {
            let _ = watcher.watch(
                &python.environment.join("lib"),
                notify::RecursiveMode::Recursive,
            );
        }
        self.sync_language(cx);
        cx.notify();
    }
    pub(super) fn language_manager(&self) -> Option<&LanguageManager> {
        match self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.document.language())
        {
            Some("rust") => self.dev.language.as_ref(),
            Some("python") => self.dev.python_language.as_ref(),
            _ => None,
        }
    }
    pub(super) fn authorize_python(&mut self, cx: &mut Context<Self>) {
        let (Some(project), Some(tools)) = (&self.project, &self.dev.python) else {
            return;
        };
        let mut settings = self.dev.settings.clone();
        settings.python_trusted_fingerprint = if self.dev.python_language.is_some() {
            None
        } else {
            Some(tools.fingerprint.clone())
        };
        let result = self
            .store
            .lock()
            .unwrap()
            .save_project_tools(project.key(), &settings);
        match result {
            Ok(()) => {
                self.dev.settings = settings;
                self.start_language(cx);
            }
            Err(e) => self.message(e.to_string(), true, cx),
        }
    }
    pub(super) fn select_python(&mut self, cx: &mut Context<Self>) {
        let Some(project) = &self.project else {
            return;
        };
        let path = self.dev.python_path.read(cx).value().trim().to_string();
        let mut settings = self.dev.settings.clone();
        settings.python_interpreter = if path.is_empty() {
            None
        } else {
            Some(PathBuf::from(path))
        };
        settings.python_trusted_fingerprint = None;
        let result = self
            .store
            .lock()
            .unwrap()
            .save_project_tools(project.key(), &settings);
        match result {
            Ok(()) => {
                self.dev.settings = settings;
                self.discover_tools(cx);
            }
            Err(e) => self.message(e.to_string(), true, cx),
        }
    }
    pub(super) fn stop_language(&mut self, cx: &mut Context<Self>) {
        if let Some(manager) = self.dev.language.take() {
            manager.stop();
        }
        if let Some(manager) = self.dev.python_language.take() {
            manager.stop();
        }
        self.dev.python_health = "Python tools off".into();
        self.dev.health = "Language tools off".into();
        self.dev.problems.clear();
        self.dev.navigation.clear();
        self.dev.diagnostic_stamp = None;
        self.dev.navigation_stamp = None;
        self.dev.diagnostic_busy = false;
        for tab in &mut self.tabs {
            tab.language_bound = false;
            tab.editor.update(cx, |editor, cx| {
                editor.lsp_mut().completion_provider = None;
                editor.lsp_mut().hover_provider = None;
                editor.lsp_mut().definition_provider = None;
                editor.lsp_mut().show_document = None;
                if let Some(set) = editor.diagnostics_mut() {
                    set.clear();
                }
                cx.notify();
            });
        }
    }
    pub(super) fn disable_language(&mut self, cx: &mut Context<Self>) {
        let Some(project) = &self.project else {
            return;
        };
        let mut settings = self.dev.settings.clone();
        settings.trusted_fingerprint = None;
        let result = self
            .store
            .lock()
            .unwrap()
            .save_project_tools(project.key(), &settings);
        match result {
            Ok(()) => {
                self.dev.settings = settings;
                self.start_language(cx);
                cx.notify();
            }
            Err(e) => self.message(e.to_string(), true, cx),
        }
    }
    pub(super) fn sync_language(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else {
            return;
        };
        for (language, manager) in [
            ("rust", self.dev.language.clone()),
            ("python", self.dev.python_language.clone()),
        ] {
            if let Some(manager) = manager {
                manager.set_documents(
                    self.tabs
                        .iter()
                        .filter(|t| t.document.language() == language)
                        .map(|t| LanguageDocument {
                            path: project.root.join(&t.document.path),
                            instance: t.instance.clone(),
                            text: Arc::from(t.editor.read(cx).text().to_string()),
                        })
                        .collect(),
                );
            }
        }
        let weak = cx.entity().downgrade();
        for tab in &mut self.tabs {
            if tab.language_bound {
                continue;
            }
            let manager = match tab.document.language() {
                "rust" => self.dev.language.clone(),
                "python" => self.dev.python_language.clone(),
                _ => None,
            };
            let Some(manager) = manager else {
                continue;
            };
            let provider = Rc::new(super::provider::Provider {
                manager: manager.clone(),
                file: project.root.join(&tab.document.path),
                instance: tab.instance.clone(),
            });
            let weak = weak.clone();
            let root = project.root.clone();
            tab.editor.update(cx, |editor, _| {
                editor.lsp_mut().completion_provider = Some(provider.clone());
                editor.lsp_mut().definition_provider = Some(provider.clone());
                editor.lsp_mut().hover_provider = Some(provider);
                editor.lsp_mut().show_document = Some(Rc::new(move |params, window, cx| {
                    let Ok(path) = tandem_lsp::uri_path(params.uri.as_str()) else {
                        return true;
                    };
                    let Ok(relative) = path
                        .strip_prefix(&root)
                        .ok()
                        .and_then(|p| normalized_relative(p).ok())
                        .ok_or(())
                    else {
                        return true;
                    };
                    let line = params.selection.as_ref().map(|r| r.start.line as usize + 1);
                    // Returning true prevents the toolkit from opening external URIs.
                    let weak = weak.clone();
                    window.defer(cx, move |window, cx| {
                        let _ = weak.update(cx, |view, cx| {
                            view.open_file(relative, false, line, window, cx)
                        });
                    });
                    true
                }));
            });
            tab.language_bound = true;
        }
    }
    pub(super) fn invalidate_language(&mut self, cx: &mut Context<Self>) {
        self.dev.last_edit = Instant::now();
        self.dev.diagnostic_key.clear();
        self.dev.diagnostic_stamp = None;
        self.dev.navigation_stamp = None;
        self.dev.problems.clear();
        self.dev.navigation.clear();
        if let Some(manager) = &self.dev.language {
            manager.invalidate();
        }
        if let Some(manager) = &self.dev.python_language {
            manager.invalidate();
        }
        for tab in &self.tabs {
            tab.editor.update(cx, |editor, cx| {
                if let Some(set) = editor.diagnostics_mut() {
                    set.clear();
                }
                cx.notify();
            });
        }
        self.sync_language(cx);
    }
    pub(super) fn development_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        if self.disk_events.load(Ordering::Relaxed) {
            self.invalidate_language(cx);
            self.invalidate_companion(cx);
        }
        self.poll_agent(cx);
        let update = self
            .dev
            .check_updates
            .as_ref()
            .and_then(|u| u.lock().unwrap().take());
        if let Some(record) = update {
            self.receive_check(record, cx);
        }
        self.sync_language(cx);
        for (language, manager) in [
            ("rust", self.dev.language.clone()),
            ("python", self.dev.python_language.clone()),
        ] {
            if let Some(manager) = manager {
                let health = manager.health();
                let detail = format!("{} · {}", health.state, health.detail);
                let target = if language == "rust" {
                    &mut self.dev.health
                } else {
                    &mut self.dev.python_health
                };
                if *target != detail {
                    *target = detail;
                    cx.notify();
                }
            }
        }
        let Some(manager) = self.language_manager().cloned() else {
            return;
        };
        let health = manager.health();
        if self
            .dev
            .diagnostic_stamp
            .as_ref()
            .is_some_and(|s| !manager.is_current(s))
        {
            self.invalidate_language(cx);
        }
        if health.state == "ready"
            && !self.dev.diagnostic_busy
            && self.dev.last_edit.elapsed() >= Duration::from_millis(700)
        {
            let key = self
                .active
                .and_then(|i| self.tabs.get(i))
                .filter(|t| matches!(t.document.language(), "rust" | "python"))
                .map(|t| {
                    format!(
                        "{}:{}",
                        t.instance,
                        content_hash(t.editor.read(cx).text().to_string().as_bytes())
                    )
                });
            if let Some(key) = key
                && (key != self.dev.diagnostic_key
                    || self.dev.last_edit.elapsed() >= Duration::from_secs(8))
            {
                self.dev.diagnostic_key = key;
                self.dev.last_edit = Instant::now();
                self.language_action("textDocument/diagnostic", window, cx);
            }
        }
    }
    pub(super) fn language_action(
        &mut self,
        method: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.loading {
            return;
        }
        self.sync_language(cx);
        let Some(manager) = self.language_manager().cloned() else {
            self.dev.visible = true;
            self.dev.panel = Panel::Tools;
            self.message("Enable trusted tools for this file’s language.", false, cx);
            return;
        };
        let (Some(project), Some(tab)) =
            (&self.project, self.active.and_then(|i| self.tabs.get(i)))
        else {
            return;
        };
        if !matches!(tab.document.language(), "rust" | "python") {
            self.message(
                "Language features currently support Rust and Python.",
                false,
                cx,
            );
            return;
        }
        if tab.document.language() == "python" && method == "textDocument/formatting" {
            self.message("Pyright provides Python analysis and navigation; a Python formatter is not configured.",false,cx);
            return;
        }
        let file = project.root.join(&tab.document.path);
        let relative = tab.document.path.clone();
        let instance = tab.instance.clone();
        let editor = tab.editor.read(cx);
        let text = editor.text().to_string();
        let cursor = editor.cursor();
        let epoch = self.epoch;
        let method = method.to_string();
        let params = match method.as_str() {
            "textDocument/diagnostic" | "textDocument/documentSymbol" => json!({}),
            "textDocument/formatting" => json!({"options":{"tabSize":4,"insertSpaces":true}}),
            _ => {
                json!({"position":tandem_lsp::utf16_position(&text,cursor).unwrap_or_default(),"context":{"includeDeclaration":true}})
            }
        };
        if method == "textDocument/diagnostic" {
            self.dev.diagnostic_busy = true;
        } else {
            self.message("Requesting language result…", false, cx);
        }
        let operation = cx.background_spawn({
            let method = method.clone();
            let manager = manager.clone();
            async move {
                manager
                    .request(&method, &file, &instance, &text, params)
                    .await
            }
        });
        let weak = cx.entity().downgrade();
        window.spawn(cx,async move|cx|{let result=operation.await;let _=weak.update_in(cx,|view,window,cx|{
            if view.epoch!=epoch{return;}
            if method=="textDocument/diagnostic"{view.dev.diagnostic_busy=false;}
            view.sync_language(cx);
            let reply=match result {Ok(reply) if manager.is_current(&reply.stamp)=>reply,Ok(_)=>{if method!="textDocument/diagnostic"{view.message("Code changed; language result discarded. Request it again.",false,cx);}return;},Err(error)=>{if method!="textDocument/diagnostic"{view.message(error.to_string(),true,cx);}return;}};
            let applied=(||->Result<()>{match method.as_str(){
                "textDocument/diagnostic"=>{
                    let items=tandem_lsp::diagnostic_items(reply.value)?;
                    view.dev.problems=items.iter().take(200).map(|d|Navigation{path:relative.clone(),line:d.range.start.line as usize+1,label:format!("{}:{}  {}",relative,d.range.start.line+1,d.message)}).collect();
                    view.dev.diagnostic_stamp=Some(reply.stamp);
                    if let Some(tab)=view.tabs.iter().find(|t|t.document.path==relative){tab.editor.update(cx,|editor,cx|{if let Some(set)=editor.diagnostics_mut(){set.clear();set.extend(items.into_iter().map(gpui_kit::component::highlighter::Diagnostic::from));}cx.notify();});}
                },
                "textDocument/formatting"=>{
                    let edits:Vec<lsp_types::TextEdit>=serde_json::from_value(reply.value)?;
                    let tab=view.tabs.iter().find(|t|t.document.path==relative).context("File closed")?;
                    let formatted=tandem_lsp::apply_text_edits(&tab.editor.read(cx).text().to_string(),&edits)?;
                    tab.editor.update(cx,|editor,cx|editor.replace_all(formatted,window,cx));
                    view.message("Formatting applied to the buffer. Undo with ⌘Z; save when ready.",false,cx);
                },
                _=>{
                    view.dev.navigation=navigation_results(&reply.value,&relative,&view.project.as_ref().unwrap().root);
                    view.dev.navigation_stamp=Some(reply.stamp);view.dev.visible=true;view.dev.panel=Panel::Problems;
                    view.message(format!("{} language navigation result(s)",view.dev.navigation.len()),false,cx);
                }
            }Ok(())})();if let Err(e)=applied{view.message(e.to_string(),true,cx);}cx.notify();
        });}).detach();
    }
    pub(super) fn prepare_recipe(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.dev.check_cancel.is_some() || self.agent.busy {
            return;
        }
        let Some(recipe) = self.dev.recipes.iter().find(|r| r.id == id) else {
            return;
        };
        if self.dev.settings.recipe_grants.get(id) == Some(&recipe.grant_fingerprint) {
            self.start_check(id, cx);
        } else {
            self.dev.pending_recipe = Some(id.into());
            cx.notify();
        }
    }
    pub(super) fn authorize_recipe(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.dev.pending_recipe.clone() else {
            return;
        };
        let Some(recipe) = self.dev.recipes.iter().find(|r| r.id == id) else {
            return;
        };
        let Some(project) = &self.project else {
            return;
        };
        let mut settings = self.dev.settings.clone();
        settings
            .recipe_grants
            .insert(id.clone(), recipe.grant_fingerprint.clone());
        let result = self
            .store
            .lock()
            .unwrap()
            .save_project_tools(project.key(), &settings);
        match result {
            Ok(()) => {
                self.dev.settings = settings;
                self.dev.pending_recipe = None;
                self.start_check(&id, cx);
            }
            Err(e) => self.message(e.to_string(), true, cx),
        }
    }
    fn start_check(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.loading || self.dev.check_cancel.is_some() || self.agent.busy {
            return;
        }
        if let Err(e) = self.flush(cx) {
            self.message(
                format!("Check not started: drafts could not be preserved: {e}"),
                true,
                cx,
            );
            return;
        }
        self.snapshot_live(cx);
        let overlays = match self.working_snapshots(cx) {
            Ok(documents) => documents,
            Err(e) => {
                self.message(format!("Could not capture retained drafts: {e}"), true, cx);
                return;
            }
        };
        let (Some(project), Some(env), Some(recipe)) = (
            self.project.clone(),
            self.dev.environment.clone(),
            self.dev.recipes.iter().find(|r| r.id == id).cloned(),
        ) else {
            return;
        };
        if self.dev.settings.recipe_grants.get(id) != Some(&recipe.grant_fingerprint) {
            return;
        }
        let mut record = CheckRecord::pending(
            project.key().into(),
            recipe.id.clone(),
            recipe.fingerprint.clone(),
        );
        if let Some(task) = self
            .dev
            .tasks
            .iter()
            .find(|t| Some(&t.id) == self.dev.selected_task.as_ref())
        {
            record.task_id = Some(task.id.clone());
            record.step_id = task.active_step.clone();
            record.plan_revision = Some(task.revision);
        }
        let stored = self.store.lock().unwrap().save_check(&record);
        if let Err(e) = stored {
            self.message(format!("Check not started: {e}"), true, cx);
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let updates = Arc::new(Mutex::new(None));
        self.dev.check_cancel = Some(cancel.clone());
        self.dev.check_updates = Some(updates.clone());
        self.dev.selected_check = Some(record.id.clone());
        self.receive_check(record.clone(), cx);
        let store = self.store.clone();
        let epoch = self.epoch;
        let operation = cx.background_spawn(async move {
            tandem_checks::run_check(
                &env,
                &project,
                &overlays,
                &recipe,
                record,
                &cancel,
                |record| {
                    store
                        .lock()
                        .map_err(|_| anyhow!("Check journal unavailable"))?
                        .save_check(record)?;
                    *updates.lock().unwrap() = Some(record.clone());
                    Ok(())
                },
            )
        });
        cx.spawn(async move |view, cx| {
            let result = operation.await;
            let _ = view.update(cx, |view, cx| {
                if view.epoch != epoch {
                    return;
                }
                view.dev.check_cancel = None;
                view.dev.check_updates = None;
                match result {
                    Ok(record) => view.receive_check(record, cx),
                    Err(e) => view.message(
                        format!(
                            "Could not persist final check result: {e}. Check outcome is uncertain."
                        ),
                        true,
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn receive_check(&mut self, record: CheckRecord, cx: &mut Context<Self>) {
        if record.completed_at_ms.is_some() {
            self.status = format!(
                "{} · {:?}. {}",
                record.recipe_id, record.status, record.detail
            );
            self.error = matches!(
                record.status,
                CheckStatus::Failed | CheckStatus::Unvalidated | CheckStatus::Interrupted
            );
        }
        if let Some(existing) = self.dev.checks.iter_mut().find(|c| c.id == record.id) {
            if existing.sequence <= record.sequence {
                *existing = record;
            }
        } else {
            self.dev.checks.insert(0, record);
            self.dev.checks.truncate(30);
        }
        cx.notify();
    }
    pub(super) fn edit_task(
        &mut self,
        existing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task = existing
            .then(|| {
                self.dev
                    .tasks
                    .iter()
                    .find(|t| Some(&t.id) == self.dev.selected_task.as_ref())
                    .cloned()
            })
            .flatten();
        self.dev.editing_task = task.as_ref().map(|t| t.id.clone());
        self.dev.task_editor = true;
        self.dev.goal.update(cx, |field, cx| {
            field.set_value(
                task.as_ref().map(|t| t.goal.clone()).unwrap_or_default(),
                window,
                cx,
            )
        });
        self.dev.criteria.update(cx, |field, cx| {
            field.set_value(
                task.as_ref()
                    .map(|t| t.acceptance.join("\n"))
                    .unwrap_or_default(),
                window,
                cx,
            )
        });
        self.dev.steps.update(cx, |field, cx| {
            field.set_value(
                task.as_ref()
                    .map(|t| {
                        t.steps
                            .iter()
                            .map(|s| s.title.clone())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default(),
                window,
                cx,
            )
        });
        cx.notify();
    }
    pub(super) fn save_task_plan(&mut self, cx: &mut Context<Self>) {
        let Some(project) = &self.project else {
            return;
        };
        let goal = self.dev.goal.read(cx).value().to_string();
        let lines = |s: String| {
            s.lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        let criteria = lines(self.dev.criteria.read(cx).value().to_string());
        let titles = lines(self.dev.steps.read(cx).value().to_string());
        let result = (|| -> Result<TaskPlan> {
            let task = if let Some(old) = self
                .dev
                .tasks
                .iter()
                .find(|t| Some(&t.id) == self.dev.editing_task.as_ref())
            {
                let mut task = old.clone();
                task.revise(goal, criteria, titles)?;
                task
            } else {
                TaskPlan::new(project.key().into(), goal, criteria, titles)?
            };
            self.store.lock().unwrap().save_task(&task)?;
            Ok(task)
        })();
        match result {
            Ok(task) => {
                self.dev.tasks.retain(|t| t.id != task.id);
                self.dev.selected_task = Some(task.id.clone());
                self.dev.tasks.insert(0, task);
                self.dev.task_editor = false;
                self.message(
                    "Plan saved. Step completion remains your decision.",
                    false,
                    cx,
                );
            }
            Err(e) => self.message(e.to_string(), true, cx),
        }
    }
    pub(super) fn task_progress(&mut self, start: Option<String>, cx: &mut Context<Self>) {
        let Some(old) = self
            .dev
            .tasks
            .iter()
            .find(|t| Some(&t.id) == self.dev.selected_task.as_ref())
            .cloned()
        else {
            return;
        };
        let mut task = old;
        let result = (|| -> Result<()> {
            if let Some(id) = start {
                task.start_step(&id)?;
            } else {
                let evidence = self
                    .dev
                    .checks
                    .iter()
                    .filter(|c| {
                        c.task_id.as_ref() == Some(&task.id)
                            && c.step_id == task.active_step
                            && c.plan_revision == Some(task.revision)
                            && c.completed_at_ms.is_some()
                    })
                    .map(|c| c.id.clone())
                    .collect();
                task.complete_active_manually(evidence)?;
            }
            self.store.lock().unwrap().save_task(&task)
        })();
        match result {
            Ok(()) => {
                if let Some(existing) = self.dev.tasks.iter_mut().find(|t| t.id == task.id) {
                    *existing = task.clone();
                }
                for step in &task.steps {
                    if step.status == StepStatus::Complete {
                        self.companion.settings.clear_step(&step.id);
                    }
                }
                if let Some(step) = task
                    .steps
                    .iter()
                    .find(|s| Some(&s.id) == task.active_step.as_ref())
                    && !step.languages.is_empty()
                    && !step.languages.contains(&self.companion.language)
                {
                    self.companion.language = step.languages[0].clone();
                }
                let settings_saved = self
                    .store
                    .lock()
                    .unwrap()
                    .save_guidance(&self.companion.settings);
                if let Err(e) = settings_saved {
                    self.companion.status =
                        format!("Progress saved, but guidance cleanup failed: {e}");
                }
                self.companion.selected_run = None;
                self.companion.solution = None;
                self.companion.proposal_fields = None;
                self.dev.scroll.set_offset(point(px(0.0), px(0.0)));
                cx.notify();
            }
            Err(e) => self.message(e.to_string(), true, cx),
        }
    }
}

fn navigation_results(value: &Value, relative: &str, root: &std::path::Path) -> Vec<Navigation> {
    fn visit(value: &Value, relative: &str, root: &std::path::Path, result: &mut Vec<Navigation>) {
        if result.len() >= 200 {
            return;
        }
        if let Some(array) = value.as_array() {
            for item in array {
                visit(item, relative, root, result);
            }
            return;
        }
        let location = value.get("location").unwrap_or(value);
        let path = if let Some(uri) = location.get("uri").and_then(Value::as_str) {
            tandem_lsp::uri_path(uri).ok().and_then(|p| {
                p.strip_prefix(root)
                    .ok()
                    .and_then(|r| normalized_relative(r).ok())
            })
        } else {
            Some(relative.into())
        };
        let range = value
            .get("selectionRange")
            .or_else(|| location.get("range"));
        if let (Some(path), Some(range)) = (path, range) {
            let line = range["start"]["line"].as_u64().unwrap_or(0) as usize + 1;
            let name = value["name"].as_str().unwrap_or("Reference");
            result.push(Navigation {
                label: format!("{path}:{line}  {name}"),
                path,
                line,
            });
        }
        if let Some(children) = value.get("children") {
            visit(children, relative, root, result);
        }
    }
    let mut result = vec![];
    visit(value, relative, root, &mut result);
    result
}
