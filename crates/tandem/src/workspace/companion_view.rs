use super::companion::PreferenceScope;
use super::panels::note;
use super::*;
use crate::appearance::{
    compact_button, navigation_button, palette, panel_body, panel_heading, panel_metadata,
    panel_warning, path_button, solid_section, wrapping_button,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Editor, Input, Textarea};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;

impl Workspace {
    pub(super) fn companion_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let busy = self.companion.busy || self.agent.busy;
        let guidance = self.effective_guidance();
        let task = self.active_task();
        let selected = self
            .companion
            .runs
            .iter()
            .find(|r| Some(&r.id) == self.companion.selected_run.as_ref());
        panel_body(cx)
            .child(div().h_flex().flex_wrap().min_w_0().gap_2().child(panel_heading(note(if self.agent.busy{"Agent takeover · isolated copy"}else{"Companion · AI edits off"}))).child(wrapping_button(Button::new("connection-details"), match self.companion.connection.as_ref().map(|c|&c.kind){Some(tandem_companion::ConnectionKind::Ready)=>"ChatGPT connected",Some(tandem_companion::ConnectionKind::UsageLimited)=>"Usage limited",Some(tandem_companion::ConnectionKind::SignedOut)=>"Sign in",_=>"Connection"}).on_click(cx.listener(|this,_,_,cx|{this.companion.show_connection= !this.companion.show_connection;cx.notify();}))))
            .when(self.companion.show_connection||self.companion.connection.is_none(),|d|d.child(solid_section(div(),cx).min_w_0().p_3().border_1().when(self.companion.connection.as_ref().is_some_and(|c|c.kind==tandem_companion::ConnectionKind::UsageLimited),|d|d.bg(p.warning_surface).text_color(p.warning)).child(note(self.companion.connection.as_ref().map(|c|c.detail.clone()).unwrap_or_else(||"Use your ChatGPT subscription. Editing works offline.".into()))))
                .when_some(self.companion.connection.as_ref(),|d,c|d.children(c.limits.iter().enumerate().map(|(i,line)|panel_metadata(note(line.clone()),cx).id(("usage",i)))))
                .child(div().h_flex().flex_wrap().min_w_0().gap_2().child(wrapping_button(Button::new("connect-companion"), "Connect / Refresh").disabled(busy).on_click(cx.listener(|this,_,_,cx|this.connect_companion(false,cx))))
                    .child(wrapping_button(Button::new("login-companion"), "ChatGPT Sign In").disabled(busy).on_click(cx.listener(|this,_,_,cx|this.connect_companion(true,cx)))))
                .when(self.companion.connection.is_some(),|d|d.child(path_button(Button::new("disconnect-companion"), "Disconnect").disabled(busy).on_click(cx.listener(|this,_,_,cx|this.disconnect_companion(cx))))))
            .child(panel_metadata(note(self.companion.status.clone()),cx))
            .when(busy,|d|d.child(path_button(Button::new("cancel-companion"), if self.agent.busy{"Stop Takeover"}else{"Cancel Companion Work"}).danger().on_click(cx.listener(|this,_,_,cx|{for cancel in this.agent.cancel.iter().chain(this.companion.cancel.iter()){cancel.store(true,Ordering::Relaxed);}this.companion.status="Cancellation requested; completed work is retained.".into();cx.notify();}))))
            .when_some(task,|d,task|{
                let total=task.steps.len();let complete=task.steps.iter().filter(|s|s.status==StepStatus::Complete).count();
                d.child(panel_heading(note(task.goal.clone())).mt_3().pt_3().border_t_1().border_color(cx.theme().border)).child(panel_metadata(note(format!("Plan {} · {complete}/{total} steps complete",task.revision)),cx))
                    .child(path_button(Button::new("plan-details"), if self.companion.show_plan{"Hide Plan Details"}else{"Plan Details"}).on_click(cx.listener(|this,_,_,cx|{this.companion.show_plan= !this.companion.show_plan;cx.notify();})))
                    .when(self.companion.show_plan,|d|d.child(note(task.analysis.clone())).children(task.acceptance.iter().enumerate().map(|(i,criterion)|div().id(("task-criterion",i)).child(note(format!("Acceptance {}: {criterion}",i+1)))))
                    .children(task.unknowns.iter().enumerate().map(|(i,unknown)|div().id(("task-unknown",i)).child(note(format!("Unknown: {unknown}"))))))
                    .children(task.steps.iter().enumerate().map(|(i,step)|{
                        let id=step.id.clone();let active=task.active_step.as_ref()==Some(&id);
                        solid_section(panel_body(cx).id(("companion-step",i)),cx).gap_2().p_3().border_1().when(active,|d|d.border_color(palette(cx.theme().mode).accent))
                            .child(div().font_semibold().child(note(format!("{}. {} · {:?}",i+1,step.title,step.status))))
                            .when(active,|d|d.child(note(step.why.clone())).child(note(format!("Files: {}",step.files.join(", ")))).children(step.files.iter().enumerate().map(|(j,path)|{let path=path.clone();path_button(Button::new(("step-file",j)), format!("Open {path}")).on_click(cx.listener(move|this,_,window,cx|this.open_file(path.clone(),true,None,window,cx)))}))
                                .children(step.done_when.iter().enumerate().map(|(j,c)|div().id(("done",j)).child(note(format!("Done when: {c}")))))
                                .child(path_button(Button::new("complete-companion-step"), "Mark Complete (my decision)").disabled(busy).on_click(cx.listener(|this,_,_,cx|this.task_progress(None,cx)))))
                            .when(!active&&step.status!=StepStatus::Complete,|d|d.child(path_button(Button::new(SharedString::from(format!("work-step:{id}"))), "Work on This Step").disabled(busy).on_click(cx.listener(move|this,_,_,cx|this.task_progress(Some(id.clone()),cx)))))
                    }))
            })
            .child(self.guidance_controls(cx))
            .child(Textarea::new(&self.companion.question).aria_label("Companion task or question"))
            .child(path_button(Button::new("analyze-task"), if task.is_some(){"Analyze / Revise Plan"}else{"Analyze & Propose Plan"}).primary().disabled(busy||self.project.is_none()).on_click(cx.listener(|this,_,window,cx|this.run_companion(CompanionAction::Analyze,window,cx))))
            .when(task.is_none(),|d|d.child(div().h_flex().flex_wrap().min_w_0().gap_2().child(wrapping_button(Button::new("ask-before-plan"), "Ask a Question").disabled(busy).on_click(cx.listener(|this,_,window,cx|this.run_companion(CompanionAction::Ask,window,cx))))
                .child(wrapping_button(Button::new("explain-before-plan"), "Explain Selected Code / Plan").disabled(busy).on_click(cx.listener(|this,_,window,cx|this.run_companion(CompanionAction::Explain,window,cx))))))
            .when(task.is_some(),|d|d.child(div().h_flex().flex_wrap().min_w_0().gap_2().children([
                (CompanionAction::Guide,"Guide This Step"),(CompanionAction::Hint,"Hint"),(CompanionAction::Explain,"Explain"),(CompanionAction::Ask,"Ask"),(CompanionAction::Reveal,"Reveal Suggested Solution"),(CompanionAction::Review,"Review This Step")
            ].into_iter().map(|(action,label)|wrapping_button(Button::new(label), label).disabled(busy).on_click(cx.listener(move|this,_,window,cx|this.run_companion(action,window,cx))))))
                .child(panel_metadata(note("Reveal displays an optional example. It never inserts code or grants editing permission. Review is explicit; local diagnostics run separately."),cx)))
            .child(panel_heading(note("Improve selected code · proposals only")).mt_3().pt_3().border_t_1().border_color(cx.theme().border))
            .child(div().h_flex().flex_wrap().min_w_0().gap_2().children([(CompanionAction::ImproveClarity,"Clarity"),(CompanionAction::ImproveSpeed,"Speed"),(CompanionAction::ImproveBalanced,"Balanced")].into_iter().map(|(action,label)|wrapping_button(Button::new(label), label).disabled(busy||self.active.is_none()).on_click(cx.listener(move|this,_,window,cx|this.run_companion(action,window,cx))))))
            .when_some(selected,|d,run|d.child(self.companion_response(run,&guidance,cx)))
            .child(path_button(Button::new("context-details"), if self.companion.show_context{"Hide Context Details"}else{"Context & Applicability"}).on_click(cx.listener(|this,_,_,cx|{this.companion.show_context= !this.companion.show_context;if this.companion.show_context{this.refresh_companion_context(cx);}cx.notify();})))
            .when(self.companion.show_context,|d|d

            .child(note("Uses eligible open files, step files and project configuration. Without open files, starts with a small source selection. Preview or add relative paths below. Secrets and ignored/generated inputs are excluded."))
            .child(Textarea::new(&self.companion.context_paths).aria_label("Additional companion context paths"))
            .child(path_button(Button::new("refresh-context"), if self.companion.context_busy{"Capturing context…"}else{"Refresh Context / Applicability"}).disabled(self.companion.context_busy||self.loading).on_click(cx.listener(|this,_,_,cx|this.refresh_companion_context(cx))))
            .when_some(self.companion.context.as_ref(),|d,context|d.child(note(format!("{} included files · {} omitted · snapshot {}",context.files.len(),context.omitted.len(),context.id.chars().take(10).collect::<String>())))
                .children(context.files.iter().enumerate().map(|(i,file)|super::panels::code_note(("context-file",i),format!("{}{}",file.path,if file.unsaved{" · unsaved"}else{""}),cx)))
                .children(context.omitted.iter().take(6).enumerate().map(|(i,file)|div().id(("context-omitted",i)).child(note(format!("Omitted {}: {}",file.path,file.reason))))))
)
            .child(path_button(Button::new("new-companion-task"), "New Task").disabled(busy).on_click(cx.listener(|this,_,window,cx|this.new_companion_task(window,cx))))
            .when(!self.companion.runs.is_empty(),|d|d.child(path_button(Button::new("companion-history"), "Conversation History").on_click(cx.listener(|this,_,_,cx|{this.companion.show_history= !this.companion.show_history;cx.notify();}))))
            .when(self.companion.show_history,|d|d.children(self.companion.runs.iter().take(12).map(|run|{
                let id=run.id.clone();navigation_button(path_button(Button::new(SharedString::from(format!("model-history:{id}"))), format!("{:?} · {} · {:?}",run.request.action,run.request.language,run.status)),self.companion.selected_run.as_ref()==Some(&run.id),cx).on_click(cx.listener(move|this,_,window,cx|this.select_model_run(&id,window,cx)))
            })))
            .into_any_element()
    }
    fn guidance_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let effective = self.effective_guidance();
        let patch = self.current_patch();
        solid_section(panel_body(cx),cx).gap_2().p_3().border_1()
            .child(div().h_flex().flex_wrap().min_w_0().gap_2().child(div().font_semibold().child(note("Guided language")))
                .children(["rust","python"].into_iter().map(|language|navigation_button(compact_button(Button::new(language), language),self.companion.language==language,cx).on_click(cx.listener(move|this,_,_,cx|{this.companion.language=language.into();cx.notify();})))))
            .child(note(format!("{} instructions · {} explanations · {} solutions",effective.instructions,effective.explanation,effective.solution)))
            .when(self.companion.show_settings,|d|d.child(panel_metadata(note(format!("Sources: instructions from {}; explanations from {}; solution policy from {}",effective.instruction_source,effective.explanation_source,effective.solution_source)),cx)))
            .child(path_button(Button::new("guidance-preferences"), if self.companion.show_settings{"Hide Preferences"}else{"Configure Assistance"}).on_click(cx.listener(|this,_,_,cx|{this.companion.show_settings= !this.companion.show_settings;cx.notify();})))
            .when(self.companion.show_settings,|d|d.child(note("Choose a scope. Each field cycles independently; Inherit removes that override. Step preferences expire on completion or revision."))
                .child(div().h_flex().flex_wrap().min_w_0().gap_2().children([(PreferenceScope::Global,"Global"),(PreferenceScope::Language,"Language"),(PreferenceScope::Project,"Project"),(PreferenceScope::ProjectLanguage,"Project language"),(PreferenceScope::Step,"Step"),(PreferenceScope::StepLanguage,"Step language")].into_iter().map(|(scope,label)|{
                    let needs_step=matches!(scope,PreferenceScope::Step|PreferenceScope::StepLanguage);navigation_button(wrapping_button(Button::new(label), label),self.companion.scope==scope,cx).disabled(needs_step&&self.active_task().and_then(|t|t.active_step.as_ref()).is_none()).on_click(cx.listener(move|this,_,_,cx|{this.companion.scope=scope;cx.notify();}))
                })))
                .child(path_button(Button::new("instruction-preference"), format!("Instruction override: {}",patch.instructions.map(|p|format!("{p}")).unwrap_or("Inherit".into()))).on_click(cx.listener(|this,_,_,cx|this.change_guidance("instructions",cx))))
                .child(path_button(Button::new("explanation-preference"), format!("Explanation override: {}",patch.explanation.map(|p|format!("{p}")).unwrap_or("Inherit".into()))).on_click(cx.listener(|this,_,_,cx|this.change_guidance("explanation",cx))))
                .child(path_button(Button::new("solution-preference"), format!("Solution override: {}",patch.solution.map(|p|format!("{p}")).unwrap_or("Inherit".into()))).on_click(cx.listener(|this,_,_,cx|this.change_guidance("solution",cx))))
                .child(path_button(Button::new("reset-guidance"), "Reset This Scope to Inherited").on_click(cx.listener(|this,_,_,cx|this.change_guidance("reset",cx))))
                .child(note("AutomaticPreview is a standing request for a solution when this step/language becomes active. It still grants no source-edit authority. Hidden and OnRequest both keep implementation content out until you reveal it.")))
            .into_any_element()
    }
    fn companion_response(
        &self,
        run: &ModelRun,
        guidance: &EffectiveGuidance,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = self.run_is_current(run, cx);
        let display = run.displayable(guidance, &self.companion.language);
        let p = palette(cx.theme().mode);
        let mut body=solid_section(panel_body(cx).id("selected-model-result"),cx).p_3().border_1()
            .child(panel_heading(note(format!("{:?} · {:?}",run.request.action,run.status))))
            .child(panel_metadata(note(format!("Request: {}",run.request.question)),cx))
            .child(panel_warning(note(if run.status!=ModelStatus::Completed{"No current assessment is available from this request."}else if current{"Applies to the verified current context and plan"}else{"Historical or unverified context. Refresh applicability; changes require a new review."}),cx).when(run.status==ModelStatus::Completed&&current,|d|d.bg(p.success_surface).text_color(p.success)))
            .child(div().min_w_0().when(matches!(run.status,ModelStatus::Failed|ModelStatus::Quarantined),|d|d.p_3().rounded(px(10.)).bg(p.error_surface).text_color(p.error)).child(note(run.detail.clone())));
        if !display {
            return body.when(run.response.is_some(),|d|d.child(panel_warning(note("This response is hidden because its guidance policy or language differs. Choose the matching language/settings or request a new response."),cx))).into_any_element();
        }
        let mut requested = vec![];
        match &run.response {
            Some(CompanionResponse::Plan(plan)) => {
                requested = plan.requested_context.clone();
                body = body
                    .child(panel_heading(note(format!(
                        "Proposed outcome: {}",
                        plan.goal
                    ))))
                    .child(note(plan.architecture.clone()));
                body = body
                    .children(plan.constraints.iter().enumerate().map(|(i, text)| {
                        div()
                            .id(("constraint", i))
                            .child(note(format!("Constraint: {text}")))
                    }))
                    .children(plan.unknowns.iter().enumerate().map(|(i, text)| {
                        div()
                            .id(("unknown", i))
                            .child(note(format!("Unknown: {text}")))
                    }))
                    .children(plan.acceptance.iter().enumerate().map(|(i, text)| {
                        div()
                            .id(("proposed-criterion", i))
                            .child(note(format!("Acceptance {}: {text}", i + 1)))
                    }));
                for (i, step) in plan.steps.iter().enumerate() {
                    body = body.child(
                        div()
                            .id(("proposal-step", i))
                            .min_w_0()
                            .pt_3()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .v_flex()
                            .gap_1()
                            .child(div().font_semibold().child(note(format!(
                                "{}. {}",
                                i + 1,
                                step.title
                            ))))
                            .child(note(format!("Why: {}", step.why)))
                            .child(note(format!(
                                "Files/symbols: {} · {}",
                                step.files.join(", "),
                                step.symbols.join(", ")
                            )))
                            .child(note(format!(
                                "Depends on: {}",
                                step.depends_on
                                    .iter()
                                    .map(|n| (n + 1).to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )))
                            .children(step.done_when.iter().enumerate().map(|(j, text)| {
                                div()
                                    .id(("proposed-done", j))
                                    .child(note(format!("Done when: {text}")))
                            })),
                    );
                }
                body = body.child(
                    path_button(
                        Button::new("edit-proposed-plan"),
                        if self.companion.edit_proposal {
                            "Close Plan Editor"
                        } else {
                            "Edit Proposed Plan"
                        },
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.companion.edit_proposal = !this.companion.edit_proposal;
                        cx.notify();
                    })),
                );
                if self.companion.edit_proposal
                    && let Some(fields) = &self.companion.proposal_fields
                {
                    body = body
                        .child(note("Goal"))
                        .child(Input::new(&fields.goal).aria_label("Proposed task goal"))
                        .child(note("Acceptance criteria, one per line"))
                        .child(
                            Textarea::new(&fields.criteria)
                                .aria_label("Proposed acceptance criteria"),
                        );
                    for (i, step) in fields.steps.iter().enumerate() {
                        body =
                            body.child(
                                div()
                                    .id(("edit-proposed-step", i))
                                    .min_w_0()
                                    .pt_3()
                                    .border_t_1()
                                    .border_color(cx.theme().border)
                                    .v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .font_semibold()
                                            .child(note(format!("Edit step {}", i + 1))),
                                    )
                                    .child(
                                        Input::new(&step.title)
                                            .aria_label(format!("Step {} title", i + 1)),
                                    )
                                    .child(
                                        Textarea::new(&step.why)
                                            .aria_label(format!("Step {} reason", i + 1)),
                                    )
                                    .child(Input::new(&step.files).aria_label(format!(
                                        "Step {} files, comma separated",
                                        i + 1
                                    )))
                                    .child(Textarea::new(&step.done).aria_label(format!(
                                        "Step {} completion conditions",
                                        i + 1
                                    )))
                                    .child(Input::new(&step.dependencies).aria_label(format!(
                                        "Step {} dependencies, numbered from one",
                                        i + 1
                                    ))),
                            );
                    }
                }
                body = body.child(
                    path_button(Button::new("accept-companion-plan"), "Accept This Plan")
                        .primary()
                        .disabled(!current || self.companion.busy || !requested.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| this.accept_companion_plan(cx))),
                );
            }
            Some(CompanionResponse::Guidance(response)) => {
                requested = response.requested_context.clone();
                body = body
                    .child(note(response.summary.clone()))
                    .children(response.sections.iter().enumerate().map(|(i, section)| {
                        div()
                            .id(("guidance-section", i))
                            .min_w_0()
                            .pt_3()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .v_flex()
                            .gap_1()
                            .child(div().font_semibold().child(note(section.title.clone())))
                            .child(note(section.explanation.clone()))
                    }))
                    .when(!response.next_action.trim().is_empty(), |d| {
                        d.child(note(format!("Next: {}", response.next_action)))
                    });
            }
            Some(CompanionResponse::Solution(solution)) => {
                let id = run.id.clone();
                let collapsed = self.companion.collapsed.contains(&id);
                body=body.child(panel_warning(note("One possible implementation. Your implementation may differ. No editor change or permission grant has occurred."),cx))
                    .child(path_button(Button::new("collapse-solution"), if collapsed{"Show Revealed Example"}else{"Collapse Example"}).on_click(cx.listener(move|this,_,_,cx|{if !this.companion.collapsed.remove(&id){this.companion.collapsed.insert(id.clone());}cx.notify();})));
                if !collapsed {
                    body = body.child(note(solution.summary.clone()));
                    if let Some((id, editor)) = &self.companion.solution
                        && id == &run.id
                    {
                        body = body.child(
                            Editor::new(editor)
                                .readonly(true)
                                .aria_label("Revealed suggested solution, read only")
                                .h(px(230.0))
                                .w_full()
                                .min_w_0()
                                .bg(p.content)
                                .border_1()
                                .border_color(p.border)
                                .rounded(px(6.)),
                        );
                    }
                    let code = solution.code.clone();
                    body = body.child(
                        path_button(Button::new("copy-suggested-solution"), "Copy Example")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(code.clone()))
                            }),
                    );
                    body=body.children(solution.sections.iter().enumerate().map(|(i,section)|{let title=section.title.clone();div().id(("solution-section",i)).v_flex().min_w_0().gap_2().pt_3().border_t_1().border_color(cx.theme().border).child(div().font_semibold().child(note(title.clone()))).child(note(section.explanation.clone()))
                        .child(path_button(Button::new(("explain-section",i)), "Explain This Section").disabled(self.companion.busy).on_click(cx.listener(move|this,_,window,cx|{this.companion.question.update(cx,|field,cx|field.set_value(format!("Explain the '{}' section of the already revealed suggestion. Answer directly and focus on the relevant language concepts.",title),window,cx));this.run_companion(CompanionAction::Explain,window,cx);})))
                    })).children(solution.tradeoffs.iter().enumerate().map(|(i,text)|div().id(("solution-tradeoff",i)).child(note(format!("Tradeoff: {text}")))));
                }
            }
            Some(CompanionResponse::Review(review)) => {
                requested = review.requested_context.clone();
                body=body.child(panel_heading(note(format!("Model assessment: {:?}",review.conclusion)))).child(note(review.summary.clone())).child(panel_metadata(note("Model assessment, check results, and your step-completion decision are separate."),cx));
                if run.request.evidence.is_empty() {
                    body=body.child(panel_warning(note("No matching check receipt was available for this snapshot. This assessment does not establish that tests or compilation passed."),cx));
                }
                for (i, finding) in review.findings.iter().enumerate() {
                    let path = finding.path.clone();
                    let line = finding.line.max(1) as usize;
                    body = body.child(
                        solid_section(panel_body(cx).id(("review-finding", i)), cx)
                            .gap_2()
                            .p_3()
                            .border_1()
                            .when(finding.blocking, |d| d.border_color(p.warning))
                            .child(note(format!(
                                "{:?} · {}",
                                finding.category,
                                if finding.blocking {
                                    "blocks completion"
                                } else {
                                    "optional"
                                }
                            )))
                            .child(note(finding.problem.clone()))
                            .child(note(format!("Why: {}", finding.why)))
                            .child(note(format!("Next action: {}", finding.next_action)))
                            .when(!path.is_empty(), |d| {
                                d.child(
                                    path_button(
                                        Button::new(("finding-location", i)),
                                        format!("{}:{}", path, line),
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.open_file(
                                                path.clone(),
                                                false,
                                                Some(line),
                                                window,
                                                cx,
                                            )
                                        },
                                    )),
                                )
                            }),
                    );
                }
                body = body
                    .children(
                        review
                            .validation_needed
                            .iter()
                            .enumerate()
                            .map(|(i, text)| {
                                panel_warning(note(format!("Still needs validation: {text}")), cx)
                                    .id(("validation-gap", i))
                            }),
                    )
                    .child(
                        path_button(Button::new("review-checks"), "Open Local Checks").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.dev.panel = Panel::Checks;
                                cx.notify();
                            }),
                        ),
                    );
                for (i, evidence) in run.request.evidence.iter().enumerate() {
                    let id = evidence.id.clone();
                    body = body.child(
                        path_button(
                            Button::new(("review-evidence", i)),
                            format!(
                                "{} · {} · {}",
                                evidence.kind,
                                evidence.status,
                                id.chars().take(8).collect::<String>()
                            ),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.dev.selected_check = Some(id.clone());
                            this.dev.panel = Panel::Checks;
                            cx.notify();
                        })),
                    );
                }
            }
            Some(CompanionResponse::Agent(_)) => {
                body=body.child(note("Delegated implementation is available only in Takeover for explicit diff review and import."));
            }
            Some(CompanionResponse::Improvement(proposal)) => {
                body = body
                    .child(panel_heading(note(format!(
                        "{:?}: {:?}",
                        proposal.objective, proposal.decision
                    ))))
                    .child(note(proposal.summary.clone()))
                    .child(panel_metadata(
                        note(format!(
                            "Expected complexity (not a measurement): {}",
                            proposal.expected_complexity
                        )),
                        cx,
                    ))
                    .child(note(format!(
                        "Practical costs: {}",
                        proposal.practical_costs
                    )))
                    .child(note(format!("Benchmark plan: {}", proposal.benchmark_plan)));
                for (i, (label, lines)) in [
                    ("Preserve", &proposal.invariants),
                    ("Preconditions", &proposal.preconditions),
                    ("Tradeoffs", &proposal.tradeoffs),
                    ("Proposed steps", &proposal.suggested_steps),
                    ("Validation needed", &proposal.validation_needed),
                ]
                .into_iter()
                .enumerate()
                {
                    body = body.child(
                        div()
                            .id(("improvement-section", i))
                            .child(note(format!("{label}: {}", lines.join("\n")))),
                    );
                }
                let measured = run
                    .request
                    .evidence
                    .iter()
                    .filter(|e| {
                        e.kind == "python-benchmark"
                            && e.status == "Passed"
                            && proposal.evidence_ids.contains(&e.id)
                    })
                    .collect::<Vec<_>>();
                if measured.is_empty() {
                    body=body.child(panel_warning(note("No matching successful benchmark receipt cited. Performance claims are expectations, not measurements."),cx));
                }
                for (i, e) in measured.iter().enumerate() {
                    body = body.child(
                        solid_section(div().id(("improvement-measurement", i)), cx)
                            .min_w_0()
                            .p_3()
                            .bg(p.success_surface)
                            .text_color(p.success)
                            .child(note(format!(
                                "Recorded measurement {}: {}",
                                e.id, e.summary
                            ))),
                    );
                }
                body=body.child(panel_metadata(note("Suggestions remain proposals. No source change is authorized by this response."),cx));
            }
            Some(
                CompanionResponse::Discussion(_)
                | CompanionResponse::SpecDraft(_)
                | CompanionResponse::PlanDraft(_)
                | CompanionResponse::WalkthroughDraft(_)
                | CompanionResponse::ActionAssessment(_),
            ) => {}
            None => {}
        }
        if !requested.is_empty() {
            body = body
                .child(note(format!(
                    "More context requested: {}",
                    requested.join(", ")
                )))
                .child(
                    path_button(
                        Button::new("include-requested-context"),
                        "Include These Context Paths",
                    )
                    .disabled(self.companion.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.include_requested_context(requested.clone(), window, cx)
                    })),
                );
        }
        body.into_any_element()
    }
}
