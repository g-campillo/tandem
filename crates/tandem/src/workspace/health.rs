use super::panels::note;
use super::*;
use gpui_kit::component::button::Button;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
#[derive(Default)]
pub(super) struct HealthUi {
    pub busy: bool,
    pub lines: Vec<String>,
    pub resources: Option<PathBuf>,
}
impl Workspace {
    pub(super) fn refresh_health(&mut self, cx: &mut Context<Self>) {
        if self.health.busy {
            return;
        }
        self.health.busy = true;
        let operation=cx.background_spawn(async move{
            let mut lines=vec![format!("Tandem {} · macOS Apple Silicon",env!("CARGO_PKG_VERSION"))];let mut resources=None;
            match tandem_codex::discover_runtime(){Ok(runtime)=>{
                lines.push(format!("Codex 0.169.0 · verified {} runtime",if runtime.bundled{"bundled"}else{"development"}));lines.push(format!("Runtime: {}",runtime.binary.display()));resources=runtime.resources;
                if let Some(root)=&resources && let Ok(bytes)=std::fs::read(root.join("release-manifest.json")) && bytes.len()<1024*1024 && let Ok(value)=serde_json::from_slice::<serde_json::Value>(&bytes){lines.push(format!("Release: {}",value["release_id"].as_str().unwrap_or("development")));}
            },Err(e)=>{lines.push(format!("AI and validation unavailable: {e:#}"));lines.push("Ordinary editing and draft recovery remain available. Reinstall a verified local release to repair bundled components.".into());}}
            (lines,resources)
        });
        cx.spawn(async move |view, cx| {
            let (lines, resources) = operation.await;
            let _ = view.update(cx, |v, cx| {
                v.health.busy = false;
                v.health.lines = lines;
                v.health.resources = resources;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn show_local_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match std::process::Command::new("/usr/bin/open")
            .arg(path)
            .spawn()
        {
            Ok(_) => {}
            Err(e) => self.message(format!("Could not open location: {e}"), true, cx),
        }
    }
    pub(super) fn health_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = self.store.lock().unwrap().directory.clone();
        let recovery = state.clone();
        let notices = self
            .health
            .resources
            .as_ref()
            .map(|p| p.join("ThirdPartyNotices"))
            .filter(|p| p.is_dir());
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let cache = home.join("Library/Caches/Tandem");
        let cache_open = cache.clone();
        div().v_flex().gap_3().child(note("Application health & recovery"))
            .child(Button::new("refresh-health").label(if self.health.busy{"Verifying runtime…"}else{"Refresh Application Health"}).disabled(self.health.busy).on_click(cx.listener(|v,_,_,cx|v.refresh_health(cx))))
            .children(self.health.lines.iter().enumerate().map(|(i,line)|div().id(("health-line",i)).child(note(line.clone()))))
            .child(note(self.companion.connection.as_ref().map_or_else(||"AI disconnected. Editing is available.".into(),|c|c.detail.clone())))
            .child(note("ChatGPT subscription only. Included usage limits apply; API-key billing is never selected automatically."))
            .child(note(format!("Recovery profile: {}",state.display())))
            .child(note("Drafts and task/import journals are stored locally. Quitting flushes open drafts; restart restores them without saving project files or replaying agent work. An older build refuses a newer database schema."))
            .child(Button::new("open-recovery").label("Open Recovery Folder").on_click(cx.listener(move|v,_,_,cx|v.show_local_folder(recovery.clone(),cx))))
            .child(note(format!("Validation and staged work: {}",cache.display())))
            .child(Button::new("open-caches").label("Open Local Work Artifacts").on_click(cx.listener(move|v,_,_,cx|v.show_local_folder(cache_open.clone(),cx))))
            .child(note("Staged work and receipts are retained for inspection. Cache cleanup is manual; staged files may still be needed for import. Managed sign-in credentials are separate and are never shown here."))
            .when_some(notices,|d,path|d.child(Button::new("open-notices").label("Open Third-Party Notices").on_click(cx.listener(move|v,_,_,cx|v.show_local_folder(path.clone(),cx)))))
            .child(note(format!("Rust: {}",self.dev.rust.as_ref().map_or_else(||if self.dev.tool_error.is_empty(){"Open a project to inspect installed tools".into()}else{self.dev.tool_error.clone()},|t|format!("{} · {}",t.label,t.cargo.display())))))
            .child(note(format!("Python: {}",self.dev.python.as_ref().map_or_else(||if self.dev.python_error.is_empty(){"Open a project to inspect installed tools".into()}else{self.dev.python_error.clone()},|t|format!("{} · {}",t.label,t.interpreter.display())))))
            .child(note("Language servers and toolchains are external. Configure them in Tools; Tandem does not install dependencies or change your shell settings."))
            .child(note("Local ad hoc macOS build. Not notarized; other Macs and Windows are untested. UTF-8 files ≤2 MiB; 32 open tabs. Buffer undo is per session. Import undo journals survive restart."))
            .into_any_element()
    }
}
