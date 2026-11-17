use super::breadcrumbs::OutlineSymbol;
use super::chrome_logic::{ProblemCounts, problem_counts};
use super::*;

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::rc::Rc;
use tandem_core::languages::{language_label, tool_kind};
use tandem_lsp::{LanguageDocument, LanguageManager, LanguageStamp};

#[derive(Clone)]
pub struct Navigation {
    pub path: String,
    pub line: usize,
    pub label: String,
}
pub struct Development {
    pub settings: ProjectTools,
    pub rust: Option<RustTools>,
    pub python: Option<PythonTools>,
    pub python_error: String,
    pub python_path: Entity<InputState>,
    pub python_language: Option<LanguageManager>,
    pub python_health: String,
    pub discovering: bool,
    pub tool_error: String,
    pub language: Option<LanguageManager>,
    pub health: String,
    pub last_edit: Instant,
    pub diagnostic_busy: bool,
    pub diagnostic_key: String,
    pub diagnostic_stamp: Option<LanguageStamp>,
    pub problems: Vec<Navigation>,
    /// The errors and warnings in the open file, for the status bar.
    pub problem_counts: ProblemCounts,
    pub navigation: Vec<Navigation>,
    pub navigation_stamp: Option<LanguageStamp>,
    /// The symbols of the open file, for the breadcrumbs.
    pub outline: Vec<OutlineSymbol>,
    /// The tab instance and text hash `outline` was last asked for.
    pub outline_key: String,
    /// The tab instance `outline` belongs to.
    pub outline_instance: String,
    pub outline_busy: bool,
    /// The tool groups served through `tandem_core::SERVER_KINDS`, filled by discovery.
    pub servers: BTreeMap<ToolKind, ServerSlot>,
}

/// What discovery found for one registry kind, and the servers running for it.
#[derive(Default)]
pub struct ServerSlot {
    pub tools: Option<KindTools>,
    pub missing: Option<MissingTools>,
    pub running: Vec<RunningServer>,
    /// One line per running server: "{name}: {state} · {detail}".
    pub health: Vec<String>,
}

pub struct RunningServer {
    pub name: String,
    pub languages: Vec<String>,
    pub manager: LanguageManager,
}

impl Development {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        Self {
            settings: ProjectTools::default(),
            rust: None,
            python: None,
            python_error: String::new(),
            python_path: cx
                .new(|cx| InputState::new(window, cx).placeholder("Optional absolute Python path")),
            python_language: None,
            python_health: "Python tools off".into(),
            discovering: false,
            tool_error: String::new(),
            language: None,
            health: "Language tools off".into(),
            last_edit: Instant::now(),
            diagnostic_busy: false,
            diagnostic_key: String::new(),
            diagnostic_stamp: None,
            problems: vec![],
            problem_counts: ProblemCounts::default(),
            navigation: vec![],
            navigation_stamp: None,
            outline: vec![],
            outline_key: String::new(),
            outline_instance: String::new(),
            outline_busy: false,
            servers: BTreeMap::new(),
        }
    }
    pub fn reset(
        &mut self,
        settings: ProjectTools,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        *self = Self::new(window, cx);
        self.settings = settings;
    }
    /// rust → `language`, python → `python_language`, others → the running server whose
    /// languages contain `language`.
    pub fn manager_for(&self, language: &str) -> Option<&LanguageManager> {
        match language {
            "rust" => self.language.as_ref(),
            "python" => self.python_language.as_ref(),
            _ => self
                .servers
                .values()
                .flat_map(|slot| &slot.running)
                .find(|server| server.languages.iter().any(|l| l == language))
                .map(|server| &server.manager),
        }
    }
    /// Every running manager with the language ids it serves: (["rust"], …), (["python"], …),
    /// then each `RunningServer`.
    pub fn managers(&self) -> Vec<(Vec<String>, LanguageManager)> {
        let fixed = [("rust", &self.language), ("python", &self.python_language)]
            .into_iter()
            .filter_map(|(language, manager)| Some((vec![language.to_string()], manager.clone()?)));
        let running = self
            .servers
            .values()
            .flat_map(|slot| &slot.running)
            .map(|server| (server.languages.clone(), server.manager.clone()));
        fixed.chain(running).collect()
    }
}
impl Drop for Development {
    fn drop(&mut self) {
        for (_, manager) in self.managers() {
            manager.stop();
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
        self.dev.discovering = true;
        self.dev.tool_error.clear();
        let epoch = self.epoch;
        let selected_python = self.dev.settings.python_interpreter.clone();
        let operation = cx.background_spawn(async move {
            let rust = RustTools::discover(&project.root);
            let python = PythonTools::discover(&project.root, selected_python.as_deref());
            let roots = ToolRoots::system().map_err(MissingTools::from);
            let servers: Vec<_> = SERVER_KINDS
                .iter()
                .map(|(kind, discover)| {
                    let found = match &roots {
                        Ok(roots) => discover(&project.root, roots),
                        Err(missing) => Err(missing.clone()),
                    };
                    (*kind, found)
                })
                .collect();
            (rust, python, servers)
        });
        cx.spawn(async move |view, cx| {
            let (rust, python, servers) = operation.await;
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
                for (kind, found) in servers {
                    let slot = view.dev.servers.entry(kind).or_default();
                    match found {
                        Ok(tools) => {
                            slot.tools = Some(tools);
                            slot.missing = None;
                        }
                        Err(missing) => {
                            slot.tools = None;
                            slot.missing = Some(missing);
                        }
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
    pub(super) fn start_language(&mut self, cx: &mut Context<Self>) {
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
            self.dev.language = Some(LanguageManager::start(tandem_lsp::rust_server(
                tools,
                &project.root,
                &cache.join("rust"),
            )));
        }
        if let Some(tools) = &self.dev.python
            && self.dev.settings.python_trusted_fingerprint.as_ref() == Some(&tools.fingerprint)
            && let Some(config) =
                tandem_lsp::python_server(tools, &project.root, &cache.join("python"))
        {
            self.dev.python_language = Some(LanguageManager::start(config));
        }
        for (kind, slot) in &mut self.dev.servers {
            let Some(tools) = &slot.tools else {
                continue;
            };
            if self.dev.settings.trust(*kind) != Some(&tools.fingerprint) {
                continue;
            }
            for server in &tools.servers {
                let manager = LanguageManager::start(tandem_lsp::server_config(
                    server,
                    &project.root,
                    &cache.join(kind.key()).join(&server.name),
                ));
                slot.running.push(RunningServer {
                    name: server.name.clone(),
                    languages: server.languages.clone(),
                    manager,
                });
            }
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
        let language = self
            .active
            .and_then(|i| self.tabs.get(i))?
            .document
            .language();
        self.dev.manager_for(language)
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
        for slot in self.dev.servers.values_mut() {
            for server in slot.running.drain(..) {
                server.manager.stop();
            }
            slot.health.clear();
        }
        self.dev.python_health = "Python tools off".into();
        self.dev.health = "Language tools off".into();
        self.dev.problems.clear();
        self.dev.problem_counts = ProblemCounts::default();
        self.dev.navigation.clear();
        self.dev.diagnostic_stamp = None;
        self.dev.navigation_stamp = None;
        self.dev.diagnostic_busy = false;
        self.dev.outline.clear();
        self.dev.outline_key.clear();
        self.dev.outline_instance.clear();
        self.dev.outline_busy = false;
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
    /// Trusts `kind` with the fingerprint discovery found, then starts its servers.
    pub(super) fn trust_kind(&mut self, kind: ToolKind, cx: &mut Context<Self>) {
        let Some(fingerprint) = self
            .dev
            .servers
            .get(&kind)
            .and_then(|slot| slot.tools.as_ref())
            .map(|tools| tools.fingerprint.clone())
        else {
            return;
        };
        self.save_kind_trust(kind, Some(fingerprint), cx);
    }
    /// Withdraws the trust in `kind`, which stops its servers.
    pub(super) fn distrust_kind(&mut self, kind: ToolKind, cx: &mut Context<Self>) {
        self.save_kind_trust(kind, None, cx);
    }
    fn save_kind_trust(
        &mut self,
        kind: ToolKind,
        fingerprint: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = &self.project else {
            return;
        };
        let mut settings = self.dev.settings.clone();
        settings.set_trust(kind, fingerprint);
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
    /// Restarts every running server of `kind`.
    pub(super) fn restart_kind(&mut self, kind: ToolKind, cx: &mut Context<Self>) {
        if let Some(slot) = self.dev.servers.get(&kind) {
            for server in &slot.running {
                server.manager.restart();
            }
        }
        self.invalidate_language(cx);
        cx.notify();
    }
    pub(super) fn sync_language(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else {
            return;
        };
        for (languages, manager) in self.dev.managers() {
            manager.set_documents(
                self.tabs
                    .iter()
                    .filter(|t| languages.iter().any(|l| l == t.document.language()))
                    .map(|t| LanguageDocument {
                        path: project.root.join(&t.document.path),
                        instance: t.instance.clone(),
                        text: Arc::from(t.editor.read(cx).text().to_string()),
                    })
                    .collect(),
            );
        }
        let weak = cx.entity().downgrade();
        for tab in &mut self.tabs {
            if tab.language_bound {
                continue;
            }
            let Some(manager) = self.dev.manager_for(tab.document.language()).cloned() else {
                continue;
            };
            let provider = Rc::new(super::provider::Provider {
                manager: manager.clone(),
                file: project.root.join(&tab.document.path),
                instance: tab.instance.clone(),
            });
            let hover = Rc::new(super::hover::HoverSource {
                manager,
                file: provider.file.clone(),
                instance: tab.instance.clone(),
                language: tab.document.language(),
                editor: tab.editor.downgrade(),
                tip: self.hover.downgrade(),
            });
            let weak = weak.clone();
            let root = project.root.clone();
            tab.editor.update(cx, |editor, _| {
                editor.lsp_mut().completion_provider = Some(provider.clone());
                editor.lsp_mut().definition_provider = Some(provider);
                editor.lsp_mut().hover_provider = Some(hover);
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
        self.dev.problem_counts = ProblemCounts::default();
        self.dev.navigation.clear();
        for (_, manager) in self.dev.managers() {
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
        for slot in self.dev.servers.values_mut() {
            let health: Vec<String> = slot
                .running
                .iter()
                .map(|server| {
                    let health = server.manager.health();
                    format!("{}: {} · {}", server.name, health.state, health.detail)
                })
                .collect();
            if slot.health != health {
                slot.health = health;
                cx.notify();
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
            && !self.dev.outline_busy
            && self.dev.last_edit.elapsed() >= Duration::from_millis(700)
        {
            self.request_outline(cx);
        }
        if health.state == "ready"
            && !self.dev.diagnostic_busy
            && self.dev.last_edit.elapsed() >= Duration::from_millis(700)
        {
            let key = self
                .active
                .and_then(|i| self.tabs.get(i))
                .filter(|t| self.dev.manager_for(t.document.language()).is_some())
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
    /// Asks the language server for the open file's symbols when its text has changed
    /// since the last answer. The breadcrumbs look the cursor's line up in the answer, so
    /// a cursor move asks for nothing.
    fn request_outline(&mut self, cx: &mut Context<Self>) {
        let Some(manager) = self.language_manager().cloned() else {
            return;
        };
        let (Some(project), Some(tab)) =
            (&self.project, self.active.and_then(|i| self.tabs.get(i)))
        else {
            return;
        };
        let text = tab.editor.read(cx).text().to_string();
        let key = format!("{}:{}", tab.instance, content_hash(text.as_bytes()));
        if key == self.dev.outline_key {
            return;
        }
        let file = project.root.join(&tab.document.path);
        let instance = tab.instance.clone();
        let epoch = self.epoch;
        self.dev.outline_key = key;
        self.dev.outline_busy = true;
        let operation = cx.background_spawn({
            let (manager, instance) = (manager.clone(), instance.clone());
            async move {
                manager
                    .request(
                        "textDocument/documentSymbol",
                        &file,
                        &instance,
                        &text,
                        json!({}),
                    )
                    .await
            }
        });
        cx.spawn(async move |view, cx| {
            let result = operation.await;
            let _ = view.update(cx, |view, cx| {
                if view.epoch != epoch {
                    return;
                }
                view.dev.outline_busy = false;
                match result {
                    Ok(reply) if manager.is_current(&reply.stamp) => {
                        view.dev.outline = super::breadcrumbs::outline(&reply.value);
                        view.dev.outline_instance = instance;
                        cx.notify();
                    }
                    // The text changed while the server worked: ask again on a later tick.
                    Ok(_) => view.dev.outline_key.clear(),
                    // The server cannot answer: wait for the next change of the text.
                    Err(_) => {}
                }
            });
        })
        .detach();
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
        if let Some(language) = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.document.language())
            && tool_kind(language).is_none()
        {
            self.message(
                format!("{} files have no language tools.", language_label(language)),
                false,
                cx,
            );
            return;
        }
        let Some(manager) = self.language_manager().cloned() else {
            self.open_settings(cx);
            self.message("Enable trusted tools for this file’s language.", false, cx);
            return;
        };
        let (Some(project), Some(tab)) =
            (&self.project, self.active.and_then(|i| self.tabs.get(i)))
        else {
            return;
        };
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
            "textDocument/formatting" => {
                json!({"options":formatting_options(tab.document.language(),&text)})
            }
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
                    view.dev.problem_counts=problem_counts(items.iter().map(|d|d.severity));
                    view.dev.diagnostic_stamp=Some(reply.stamp);
                    if let Some(tab)=view.tabs.iter().find(|t|t.document.path==relative){tab.editor.update(cx,|editor,cx|{if let Some(set)=editor.diagnostics_mut(){set.clear();set.extend(items.into_iter().map(gpui_kit::component::highlighter::Diagnostic::from));}cx.notify();});}
                },
                "textDocument/formatting" if reply.value.is_null()=>view.message("No formatting changes.",false,cx),
                "textDocument/formatting"=>{
                    let edits:Vec<lsp_types::TextEdit>=serde_json::from_value(reply.value)?;
                    let tab=view.tabs.iter().find(|t|t.document.path==relative).context("File closed")?;
                    let formatted=tandem_lsp::apply_text_edits(&tab.editor.read(cx).text().to_string(),&edits)?;
                    tab.editor.update(cx,|editor,cx|editor.replace_all(formatted,window,cx));
                    view.message("Formatting applied to the buffer. Undo with ⌘Z; save when ready.",false,cx);
                },
                _=>{
                    view.dev.navigation=navigation_results(&reply.value,&relative,&view.project.as_ref().unwrap().root);
                    view.dev.navigation_stamp=Some(reply.stamp);view.bottom=Some(BottomTab::Problems);
                    view.message(format!("{} language navigation result(s)",view.dev.navigation.len()),false,cx);
                }
            }Ok(())})();if let Err(e)=applied{view.message(e.to_string(),true,cx);}cx.notify();
        });}).detach();
    }
}

/// Rust and Python keep tabSize 4 with spaces (their formatters read their own config). Other
/// languages follow the buffer: tabs when more indented lines start with a tab than with a space,
/// otherwise the smallest leading-space count from 2 to 8 among indented lines, else 4.
pub(super) fn formatting_options(language: &str, text: &str) -> Value {
    if matches!(language, "rust" | "python") {
        return json!({"tabSize": 4, "insertSpaces": true});
    }
    let indents = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let content = line.trim_start_matches([' ', '\t']);
            let indent = &line[..line.len() - content.len()];
            (!indent.is_empty()).then_some(indent)
        });
    let (mut tabs, mut spaces, mut smallest) = (0usize, 0usize, None::<usize>);
    for indent in indents {
        if indent.starts_with('\t') {
            tabs += 1;
        } else {
            spaces += 1;
            let width = indent.len() - indent.trim_start_matches(' ').len();
            if (2..=8).contains(&width) {
                smallest = Some(smallest.map_or(width, |s| s.min(width)));
            }
        }
    }
    if tabs > spaces {
        json!({"tabSize": 4, "insertSpaces": false})
    } else {
        json!({"tabSize": smallest.unwrap_or(4), "insertSpaces": true})
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

#[cfg(test)]
mod tests {
    use super::formatting_options;
    use serde_json::json;

    #[test]
    fn formatting_options_follow_the_buffer() {
        let tabbed = "fn a() {\n\tlet x = 1;\n\tlet y = 2;\n}\n";
        assert_eq!(
            formatting_options("rust", tabbed),
            json!({"tabSize": 4, "insertSpaces": true})
        );
        assert_eq!(
            formatting_options("python", "def a():\n  return 1\n"),
            json!({"tabSize": 4, "insertSpaces": true})
        );
        assert_eq!(
            formatting_options(
                "typescript",
                "function a() {\n  if (x) {\n    return 1;\n  }\n}\n"
            ),
            json!({"tabSize": 2, "insertSpaces": true})
        );
        assert_eq!(
            formatting_options(
                "typescript",
                "/**\n * Doc\n */\nfunction a() {\n    return 1;\n}\n"
            ),
            json!({"tabSize": 4, "insertSpaces": true})
        );
        assert_eq!(
            formatting_options(
                "css",
                "a {\n\tcolor: red;\n\tmargin: 0;\n  padding: 0;\n}\n"
            ),
            json!({"tabSize": 4, "insertSpaces": false})
        );
        assert_eq!(
            formatting_options("json", "{\"a\": 1}\n"),
            json!({"tabSize": 4, "insertSpaces": true})
        );
        assert_eq!(
            formatting_options("html", "<p>\n   \n</p>\n"),
            json!({"tabSize": 4, "insertSpaces": true})
        );
    }
}
