use super::*;

use serde_json::{Value, json};
use std::rc::Rc;
use tandem_lsp::{LanguageDocument, LanguageManager, LanguageStamp, ServerConfig};

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
    pub navigation: Vec<Navigation>,
    pub navigation_stamp: Option<LanguageStamp>,
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
            navigation: vec![],
            navigation_stamp: None,
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
}
impl Drop for Development {
    fn drop(&mut self) {
        if let Some(language) = &self.language {
            language.stop();
        }
        if let Some(language) = &self.python_language {
            language.stop();
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
            (rust, python)
        });
        cx.spawn(async move |view, cx| {
            let (rust, python) = operation.await;
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
            self.open_settings(cx);
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
                    view.dev.navigation_stamp=Some(reply.stamp);view.bottom=Some(BottomTab::Problems);
                    view.message(format!("{} language navigation result(s)",view.dev.navigation.len()),false,cx);
                }
            }Ok(())})();if let Err(e)=applied{view.message(e.to_string(),true,cx);}cx.notify();
        });}).detach();
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
