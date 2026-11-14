use super::companion::PreferenceScope;
use super::panels::note;
use super::*;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Editor, Input, Textarea};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;

impl Workspace {
    pub(super) fn companion_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.companion.busy || self.agent.busy;
        let guidance = self.effective_guidance();
        let task = self.active_task();
        let selected = self
            .companion
            .runs
            .iter()
            .find(|r| Some(&r.id) == self.companion.selected_run.as_ref());
        div().v_flex().gap_3()
            .child(div().h_flex().gap_2().child(div().font_semibold().child(note(if self.agent.busy{"Agent takeover · isolated copy"}else{"Companion · AI edits off"}))).child(Button::new("connection-details").label(match self.companion.connection.as_ref().map(|c|&c.kind){Some(tandem_companion::ConnectionKind::Ready)=>"ChatGPT connected",Some(tandem_companion::ConnectionKind::UsageLimited)=>"Usage limited",Some(tandem_companion::ConnectionKind::SignedOut)=>"Sign in",_=>"Connection"}).on_click(cx.listener(|this,_,_,cx|{this.companion.show_connection= !this.companion.show_connection;cx.notify();}))))
            .when(self.companion.show_connection||self.companion.connection.is_none(),|d|d.child(note(self.companion.connection.as_ref().map(|c|c.detail.clone()).unwrap_or_else(||"Use your ChatGPT subscription. Editing works offline.".into())))
                .when_some(self.companion.connection.as_ref(),|d,c|d.children(c.limits.iter().enumerate().map(|(i,line)|div().id(("usage",i)).child(note(line.clone())))))
                .child(div().h_flex().gap_1().child(Button::new("connect-companion").label("Connect / Refresh").disabled(busy).on_click(cx.listener(|this,_,_,cx|this.connect_companion(false,cx))))
                    .child(Button::new("login-companion").label("ChatGPT Sign In").disabled(busy).on_click(cx.listener(|this,_,_,cx|this.connect_companion(true,cx)))))
                .when(self.companion.connection.is_some(),|d|d.child(Button::new("disconnect-companion").label("Disconnect").disabled(busy).on_click(cx.listener(|this,_,_,cx|this.disconnect_companion(cx))))))
            .child(note(self.companion.status.clone()))
            .when(busy,|d|d.child(Button::new("cancel-companion").label(if self.agent.busy{"Stop Takeover"}else{"Cancel Companion Work"}).on_click(cx.listener(|this,_,_,cx|{for cancel in this.agent.cancel.iter().chain(this.companion.cancel.iter()){cancel.store(true,Ordering::Relaxed);}this.companion.status="Cancellation requested; completed work is retained.".into();cx.notify();}))))
            .when_some(task,|d,task|{
                let total=task.steps.len();let complete=task.steps.iter().filter(|s|s.status==StepStatus::Complete).count();
                d.child(div().font_semibold().child(note(task.goal.clone()))).child(note(format!("Plan {} · {complete}/{total} steps complete",task.revision)))
                    .child(Button::new("plan-details").label(if self.companion.show_plan{"Hide Plan Details"}else{"Plan Details"}).on_click(cx.listener(|this,_,_,cx|{this.companion.show_plan= !this.companion.show_plan;cx.notify();})))
                    .when(self.companion.show_plan,|d|d.child(note(task.analysis.clone())).children(task.acceptance.iter().enumerate().map(|(i,criterion)|div().id(("task-criterion",i)).child(note(format!("Acceptance {}: {criterion}",i+1)))))
                    .children(task.unknowns.iter().enumerate().map(|(i,unknown)|div().id(("task-unknown",i)).child(note(format!("Unknown: {unknown}"))))))
                    .children(task.steps.iter().enumerate().map(|(i,step)|{
                        let id=step.id.clone();let active=task.active_step.as_ref()==Some(&id);
                        div().id(("companion-step",i)).v_flex().gap_2().p_2().border_1().border_color(cx.theme().border)
                            .child(note(format!("{}. {} · {:?}",i+1,step.title,step.status)))
                            .when(active,|d|d.child(note(step.why.clone())).child(note(format!("Files: {}",step.files.join(", ")))).children(step.files.iter().enumerate().map(|(j,path)|{let path=path.clone();Button::new(("step-file",j)).label(format!("Open {path}")).on_click(cx.listener(move|this,_,window,cx|this.open_file(path.clone(),true,None,window,cx)))}))
                                .children(step.done_when.iter().enumerate().map(|(j,c)|div().id(("done",j)).child(note(format!("Done when: {c}")))))
                                .child(Button::new("complete-companion-step").label("Mark Complete (my decision)").disabled(busy).on_click(cx.listener(|this,_,_,cx|this.task_progress(None,cx)))))
                            .when(!active&&step.status!=StepStatus::Complete,|d|d.child(Button::new(SharedString::from(format!("work-step:{id}"))).label("Work on This Step").disabled(busy).on_click(cx.listener(move|this,_,_,cx|this.task_progress(Some(id.clone()),cx)))))
                    }))
            })
            .child(self.guidance_controls(cx))
            .child(Textarea::new(&self.companion.question).aria_label("Companion task or question"))
            .child(Button::new("analyze-task").label(if task.is_some(){"Analyze / Revise Plan"}else{"Analyze & Propose Plan"}).disabled(busy||self.project.is_none()).on_click(cx.listener(|this,_,window,cx|this.run_companion(CompanionAction::Analyze,window,cx))))
            .when(task.is_none(),|d|d.child(div().h_flex().gap_1().child(Button::new("ask-before-plan").label("Ask a Question").disabled(busy).on_click(cx.listener(|this,_,window,cx|this.run_companion(CompanionAction::Ask,window,cx))))
                .child(Button::new("explain-before-plan").label("Explain Selected Code / Plan").disabled(busy).on_click(cx.listener(|this,_,window,cx|this.run_companion(CompanionAction::Explain,window,cx))))))
            .when(task.is_some(),|d|d.child(div().h_flex().flex_wrap().gap_1().children([
                (CompanionAction::Guide,"Guide This Step"),(CompanionAction::Hint,"Hint"),(CompanionAction::Explain,"Explain"),(CompanionAction::Ask,"Ask"),(CompanionAction::Reveal,"Reveal Suggested Solution"),(CompanionAction::Review,"Review This Step")
            ].into_iter().map(|(action,label)|Button::new(label).label(label).disabled(busy).on_click(cx.listener(move|this,_,window,cx|this.run_companion(action,window,cx))))))
                .child(note("Reveal displays an optional example. It never inserts code or grants editing permission. Review is explicit; local diagnostics run separately.")))
            .child(note("Improve selected code · proposals only"))
            .child(div().h_flex().gap_1().children([(CompanionAction::ImproveClarity,"Clarity"),(CompanionAction::ImproveSpeed,"Speed"),(CompanionAction::ImproveBalanced,"Balanced")].into_iter().map(|(action,label)|Button::new(label).label(label).disabled(busy||self.active.is_none()).on_click(cx.listener(move|this,_,window,cx|this.run_companion(action,window,cx))))))
            .when_some(selected,|d,run|d.child(self.companion_response(run,&guidance,cx)))
            .child(Button::new("context-details").label(if self.companion.show_context{"Hide Context Details"}else{"Context & Applicability"}).on_click(cx.listener(|this,_,_,cx|{this.companion.show_context= !this.companion.show_context;if this.companion.show_context{this.refresh_companion_context(cx);}cx.notify();})))
            .when(self.companion.show_context,|d|d

            .child(note("Uses eligible open files, step files and project configuration. Without open files, starts with a small source selection. Preview or add relative paths below. Secrets and ignored/generated inputs are excluded."))
            .child(Textarea::new(&self.companion.context_paths).aria_label("Additional companion context paths"))
            .child(Button::new("refresh-context").label(if self.companion.context_busy{"Capturing context…"}else{"Refresh Context / Applicability"}).disabled(self.companion.context_busy||self.loading).on_click(cx.listener(|this,_,_,cx|this.refresh_companion_context(cx))))
            .when_some(self.companion.context.as_ref(),|d,context|d.child(note(format!("{} included files · {} omitted · snapshot {}",context.files.len(),context.omitted.len(),context.id.chars().take(10).collect::<String>())))
                .children(context.files.iter().enumerate().map(|(i,file)|div().id(("context-file",i)).child(note(format!("{}{}",file.path,if file.unsaved{" · unsaved"}else{""})))))
                .children(context.omitted.iter().take(6).enumerate().map(|(i,file)|div().id(("context-omitted",i)).child(note(format!("Omitted {}: {}",file.path,file.reason))))))
)
            .child(Button::new("new-companion-task").label("New Task").disabled(busy).on_click(cx.listener(|this,_,window,cx|this.new_companion_task(window,cx))))
            .when(!self.companion.runs.is_empty(),|d|d.child(Button::new("companion-history").label("Conversation History").on_click(cx.listener(|this,_,_,cx|{this.companion.show_history= !this.companion.show_history;cx.notify();}))))
            .when(self.companion.show_history,|d|d.children(self.companion.runs.iter().take(12).map(|run|{
                let id=run.id.clone();Button::new(SharedString::from(format!("model-history:{id}"))).label(format!("{:?} · {} · {:?}",run.request.action,run.request.language,run.status)).on_click(cx.listener(move|this,_,window,cx|this.select_model_run(&id,window,cx)))
            })))
            .into_any_element()
    }
    fn guidance_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let effective = self.effective_guidance();
        let patch = self.current_patch();
        div().v_flex().gap_2().p_2().border_1().border_color(cx.theme().border)
            .child(div().h_flex().gap_1().child(note("Guided language"))
                .children(["rust","python"].into_iter().map(|language|Button::new(language).label(language).when(self.companion.language==language,|b|b.primary()).on_click(cx.listener(move|this,_,_,cx|{this.companion.language=language.into();cx.notify();})))))
            .child(note(format!("{} instructions · {} explanations · {} solutions",effective.instructions,effective.explanation,effective.solution)))
            .when(self.companion.show_settings,|d|d.child(note(format!("Sources: instructions from {}; explanations from {}; solution policy from {}",effective.instruction_source,effective.explanation_source,effective.solution_source))))
            .child(Button::new("guidance-preferences").label(if self.companion.show_settings{"Hide Preferences"}else{"Configure Assistance"}).on_click(cx.listener(|this,_,_,cx|{this.companion.show_settings= !this.companion.show_settings;cx.notify();})))
            .when(self.companion.show_settings,|d|d.child(note("Choose a scope. Each field cycles independently; Inherit removes that override. Step preferences expire on completion or revision."))
                .child(div().h_flex().flex_wrap().gap_1().children([(PreferenceScope::Global,"Global"),(PreferenceScope::Language,"Language"),(PreferenceScope::Project,"Project"),(PreferenceScope::ProjectLanguage,"Project language"),(PreferenceScope::Step,"Step"),(PreferenceScope::StepLanguage,"Step language")].into_iter().map(|(scope,label)|{
                    let needs_step=matches!(scope,PreferenceScope::Step|PreferenceScope::StepLanguage);Button::new(label).label(label).when(self.companion.scope==scope,|b|b.primary()).disabled(needs_step&&self.active_task().and_then(|t|t.active_step.as_ref()).is_none()).on_click(cx.listener(move|this,_,_,cx|{this.companion.scope=scope;cx.notify();}))
                })))
                .child(Button::new("instruction-preference").label(format!("Instruction override: {}",patch.instructions.map(|p|format!("{p}")).unwrap_or("Inherit".into()))).on_click(cx.listener(|this,_,_,cx|this.change_guidance("instructions",cx))))
                .child(Button::new("explanation-preference").label(format!("Explanation override: {}",patch.explanation.map(|p|format!("{p}")).unwrap_or("Inherit".into()))).on_click(cx.listener(|this,_,_,cx|this.change_guidance("explanation",cx))))
                .child(Button::new("solution-preference").label(format!("Solution override: {}",patch.solution.map(|p|format!("{p}")).unwrap_or("Inherit".into()))).on_click(cx.listener(|this,_,_,cx|this.change_guidance("solution",cx))))
                .child(Button::new("reset-guidance").label("Reset This Scope to Inherited").on_click(cx.listener(|this,_,_,cx|this.change_guidance("reset",cx))))
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
        let mut body=div().id("selected-model-result").v_flex().gap_3().p_2().border_1().border_color(cx.theme().border)
            .child(div().font_semibold().child(note(format!("{:?} · {:?}",run.request.action,run.status))))
            .child(note(format!("Request: {}",run.request.question)))
            .child(note(if run.status!=ModelStatus::Completed{"No current assessment is available from this request."}else if current{"Applies to the verified current context and plan"}else{"Historical or unverified context. Refresh applicability; changes require a new review."}))
            .child(note(run.detail.clone()));
        if !display {
            return body.when(run.response.is_some(),|d|d.child(note("This response is hidden because its guidance policy or language differs. Choose the matching language/settings or request a new response."))).into_any_element();
        }
        let mut requested = vec![];
        match &run.response {
            Some(CompanionResponse::Plan(plan)) => {
                requested = plan.requested_context.clone();
                body = body
                    .child(note(format!("Proposed outcome: {}", plan.goal)))
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
                            .v_flex()
                            .gap_1()
                            .child(note(format!("{}. {}", i + 1, step.title)))
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
                    Button::new("edit-proposed-plan")
                        .label(if self.companion.edit_proposal {
                            "Close Plan Editor"
                        } else {
                            "Edit Proposed Plan"
                        })
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
                                    .v_flex()
                                    .gap_1()
                                    .child(note(format!("Edit step {}", i + 1)))
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
                    Button::new("accept-companion-plan")
                        .label("Accept This Plan")
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
                            .v_flex()
                            .gap_1()
                            .child(note(section.title.clone()))
                            .child(note(section.explanation.clone()))
                    }))
                    .when(!response.next_action.trim().is_empty(), |d| {
                        d.child(note(format!("Next: {}", response.next_action)))
                    });
            }
            Some(CompanionResponse::Solution(solution)) => {
                let id = run.id.clone();
                let collapsed = self.companion.collapsed.contains(&id);
                body=body.child(note("One possible implementation. Your implementation may differ. No editor change or permission grant has occurred."))
                    .child(Button::new("collapse-solution").label(if collapsed{"Show Revealed Example"}else{"Collapse Example"}).on_click(cx.listener(move|this,_,_,cx|{if !this.companion.collapsed.remove(&id){this.companion.collapsed.insert(id.clone());}cx.notify();})));
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
                                .w_full(),
                        );
                    }
                    let code = solution.code.clone();
                    body = body.child(
                        Button::new("copy-suggested-solution")
                            .label("Copy Example")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(code.clone()))
                            }),
                    );
                    body=body.children(solution.sections.iter().enumerate().map(|(i,section)|{let title=section.title.clone();div().id(("solution-section",i)).v_flex().gap_1().child(note(title.clone())).child(note(section.explanation.clone()))
                        .child(Button::new(("explain-section",i)).label("Explain This Section").disabled(self.companion.busy).on_click(cx.listener(move|this,_,window,cx|{this.companion.question.update(cx,|field,cx|field.set_value(format!("Explain the '{}' section of the already revealed suggestion. Answer directly and focus on the relevant language concepts.",title),window,cx));this.run_companion(CompanionAction::Explain,window,cx);})))
                    })).children(solution.tradeoffs.iter().enumerate().map(|(i,text)|div().id(("solution-tradeoff",i)).child(note(format!("Tradeoff: {text}")))));
                }
            }
            Some(CompanionResponse::Review(review)) => {
                requested = review.requested_context.clone();
                body=body.child(note(format!("Model assessment: {:?}",review.conclusion))).child(note(review.summary.clone())).child(note("Model assessment, check results, and your step-completion decision are separate."));
                if run.request.evidence.is_empty() {
                    body=body.child(note("No matching check receipt was available for this snapshot. This assessment does not establish that tests or compilation passed."));
                }
                for (i, finding) in review.findings.iter().enumerate() {
                    let path = finding.path.clone();
                    let line = finding.line.max(1) as usize;
                    body = body.child(
                        div()
                            .id(("review-finding", i))
                            .v_flex()
                            .gap_2()
                            .p_2()
                            .border_1()
                            .border_color(cx.theme().border)
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
                                    Button::new(("finding-location", i))
                                        .label(format!("{}:{}", path, line))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.open_file(
                                                path.clone(),
                                                false,
                                                Some(line),
                                                window,
                                                cx,
                                            )
                                        })),
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
                                div()
                                    .id(("validation-gap", i))
                                    .child(note(format!("Still needs validation: {text}")))
                            }),
                    )
                    .child(
                        Button::new("review-checks")
                            .label("Open Local Checks")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.dev.panel = Panel::Checks;
                                cx.notify();
                            })),
                    );
                for (i, evidence) in run.request.evidence.iter().enumerate() {
                    let id = evidence.id.clone();
                    body = body.child(
                        Button::new(("review-evidence", i))
                            .label(format!(
                                "{} · {} · {}",
                                evidence.kind,
                                evidence.status,
                                id.chars().take(8).collect::<String>()
                            ))
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
                    .child(note(format!(
                        "{:?}: {:?}",
                        proposal.objective, proposal.decision
                    )))
                    .child(note(proposal.summary.clone()))
                    .child(note(format!(
                        "Expected complexity (not a measurement): {}",
                        proposal.expected_complexity
                    )))
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
                    body=body.child(note("No matching successful benchmark receipt cited. Performance claims are expectations, not measurements."));
                }
                for (i, e) in measured.iter().enumerate() {
                    body = body.child(div().id(("improvement-measurement", i)).child(note(
                        format!("Recorded measurement {}: {}", e.id, e.summary),
                    )));
                }
                body=body.child(note("Suggestions remain proposals. No source change is authorized by this response."));
            }
            None => {}
        }
        if !requested.is_empty() {
            body = body
                .child(note(format!(
                    "More context requested: {}",
                    requested.join(", ")
                )))
                .child(
                    Button::new("include-requested-context")
                        .label("Include These Context Paths")
                        .disabled(self.companion.busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.include_requested_context(requested.clone(), window, cx)
                        })),
                );
        }
        body.into_any_element()
    }
}
