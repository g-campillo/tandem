use super::companion_state::{
    CompanionTab, HistoricalRow, historical_run, historical_task, model_label,
};
use super::panels::note;
use super::*;
use crate::appearance::{
    navigation_button, panel_body, panel_heading, panel_metadata, panel_warning, path_button,
    solid_section, wrapping_button,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Editor, Textarea};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;

impl Workspace {
    pub(super) fn companion_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.companion.busy || self.agent.busy;
        let mut body = panel_body(cx)
            .child(panel_heading(note("Companion")))
            .child(panel_metadata(note(self.companion.status.clone()), cx))
            .child(
                div().h_flex().flex_wrap().gap_2().children(
                    [
                        (CompanionTab::Conversation, "Conversation"),
                        (CompanionTab::Spec, "Spec"),
                        (CompanionTab::Plan, "Plan"),
                    ]
                    .into_iter()
                    .map(|(tab, label)| {
                        navigation_button(
                            wrapping_button(Button::new(label), label),
                            self.companion.tab == tab,
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.companion.tab = tab;
                            cx.notify();
                        }))
                    }),
                ),
            )
            .child(
                path_button(Button::new("connection-details"), "Connection & model").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.companion.show_connection = !this.companion.show_connection;
                        cx.notify();
                    }),
                ),
            )
            .when(self.companion.show_connection, |d| {
                d.child(self.model_controls(cx))
            })
            .when(busy, |d| {
                d.child(
                    path_button(Button::new("cancel-conversation"), "Cancel active work")
                        .danger()
                        .on_click(cx.listener(|this, _, _, cx| {
                            for c in this.companion.cancel.iter().chain(this.agent.cancel.iter()) {
                                c.store(true, Ordering::Relaxed);
                            }
                            this.companion.status =
                                "Cancellation requested; saved input and staged work are retained."
                                    .into();
                            cx.notify();
                        })),
                )
            })
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        wrapping_button(Button::new("new-conversation"), "New Task")
                            .disabled(busy || self.project.is_none())
                            .on_click(cx.listener(|this, _, w, cx| this.new_companion_task(w, cx))),
                    )
                    .child(
                        wrapping_button(Button::new("conversation-history"), "History").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.companion.show_history = !this.companion.show_history;
                                cx.notify();
                            }),
                        ),
                    ),
            );
        if self.companion.show_history {
            body = body
                .children(self.companion.sessions.iter().enumerate().map(|(i, s)| {
                    let id = s.id.clone();
                    let title = s
                        .spec
                        .as_ref()
                        .map(|s| s.summary.clone())
                        .or_else(|| s.plan.as_ref().map(|p| p.task.goal.clone()))
                        .unwrap_or_else(|| format!("Conversation {}", i + 1));
                    navigation_button(
                        path_button(
                            Button::new(SharedString::from(format!("session:{id}"))),
                            title,
                        ),
                        self.companion.session.as_ref().is_some_and(|s| s.id == id),
                        cx,
                    )
                    .disabled(busy)
                    .on_click(
                        cx.listener(move |this, _, w, cx| this.select_conversation(&id, w, cx)),
                    )
                }))
                .children(
                    self.dev
                        .tasks
                        .iter()
                        .filter(|t| {
                            !self.companion.sessions.iter().any(|s| {
                                s.plan.as_ref().is_some_and(|p| p.task.id == t.id)
                                    || s.legacy_task.as_ref().is_some_and(|l| l.task_id == t.id)
                            })
                        })
                        .map(|task| {
                            let id = task.id.clone();
                            path_button(
                                Button::new(SharedString::from(format!("legacy:{id}"))),
                                format!("Legacy · {}", task.goal),
                            )
                            .disabled(busy)
                            .on_click(cx.listener(
                                move |this, _, w, cx| this.legacy_conversation(&id, false, w, cx),
                            ))
                        }),
                );
        }
        if self.companion.show_history {
            body = body.children(
                self.companion
                    .runs
                    .iter()
                    .filter(|r| r.binding.task_id.is_none() && r.request.guided.is_none())
                    .map(|r| {
                        let id = r.id.clone();
                        path_button(
                            Button::new(SharedString::from(format!("legacy-run:{id}"))),
                            format!("Legacy conversation · {}", r.request.question),
                        )
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, w, cx| this.legacy_run(&id, w, cx)))
                    }),
            );
        }
        body = body.child(if self.companion.tab == CompanionTab::Conversation {
            self.conversation_view(cx)
        } else {
            self.documents_view(cx)
        });
        body.into_any_element()
    }
    fn model_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.companion.busy || self.agent.busy;
        let mut body = solid_section(panel_body(cx), cx)
            .p_3()
            .border_1()
            .child(note(
                self.companion
                    .connection
                    .as_ref()
                    .map(|s| s.detail.clone())
                    .unwrap_or(
                        "AI disconnected. Editor, save, terminal and local checks work offline."
                            .into(),
                    ),
            ))
            .child(panel_heading(note(format!(
                "Model: {}",
                model_label(
                    &self.companion.model_preference,
                    self.companion.catalog.as_ref()
                )
            ))))
            .when_some(self.companion.resolved_model.as_ref(), |d, m| {
                d.child(panel_metadata(
                    note(format!("Last run: {} · {}", m.display_name, m.model)),
                    cx,
                ))
            })
            .when_some(self.companion.model_error.as_ref(), |d, e| {
                d.child(panel_warning(note(e.clone()), cx))
            })
            .child(
                path_button(
                    Button::new("automatic-model"),
                    "Automatic (account default)",
                )
                .disabled(busy)
                .on_click(
                    cx.listener(|this, _, _, cx| this.choose_model(ModelPreference::Automatic, cx)),
                ),
            );
        if let Some(catalog) = &self.companion.catalog {
            body = body.children(catalog.models.iter().map(|m| {
                let model = m.model.clone();
                navigation_button(
                    path_button(
                        Button::new(SharedString::from(format!("model:{}", m.model))),
                        m.display_name.clone(),
                    ),
                    self.companion.model_preference
                        == ModelPreference::Explicit {
                            model: model.clone(),
                        },
                    cx,
                )
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.choose_model(
                        ModelPreference::Explicit {
                            model: model.clone(),
                        },
                        cx,
                    )
                }))
            }));
        } else {
            body = body.child(panel_metadata(
                note("Refresh to load the account's available models."),
                cx,
            ));
        }
        body.child(
            path_button(Button::new("refresh-models"), "Connect / refresh models")
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.connect_companion(false, cx))),
        )
        .child(
            path_button(Button::new("login-companion"), "ChatGPT sign in")
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.connect_companion(true, cx))),
        )
        .child(
            path_button(Button::new("disconnect-companion"), "Disconnect")
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.disconnect_companion(cx))),
        )
        .into_any_element()
    }
    fn conversation_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.companion.busy || self.agent.busy;
        let mut body = panel_body(cx);
        if self.companion.session.is_none()
            && let Some(task) = self.active_task()
        {
            let id = task.id.clone();
            body=body.child(panel_heading(note(format!("Legacy task: {}",task.goal)))).child(note(task.analysis.clone()))
                .child(panel_warning(note("Historical completion does not authorize new implementation. Continue to create a linked conversation with a spec and dependent plan."),cx))
                .child(path_button(Button::new("adopt-legacy-task"),"Continue in a conversation").disabled(busy).on_click(cx.listener(move|this,_,w,cx|this.legacy_conversation(&id,true,w,cx))));
            body = body.children(
                historical_task(task)
                    .into_iter()
                    .map(|r| self.historical_row(r, cx)),
            );
            for run in self
                .companion
                .runs
                .iter()
                .filter(|r| r.binding.task_id.as_deref() == Some(&task.id))
            {
                body = body.child(panel_metadata(
                    note(format!("Historical request: {}", run.request.question)),
                    cx,
                ));
                body = body.children(
                    historical_run(run)
                        .into_iter()
                        .map(|r| self.historical_row(r, cx)),
                );
            }
        }
        if self.companion.session.is_none()
            && let Some(run) = self
                .companion
                .runs
                .iter()
                .find(|r| Some(&r.id) == self.companion.selected_run.as_ref())
        {
            body = body.child(note(format!(
                "Historical request: {}",
                run.request.question
            )));
            body = body.children(
                historical_run(run)
                    .into_iter()
                    .map(|r| self.historical_row(r, cx)),
            );
        }
        if self
            .companion
            .events
            .first()
            .is_some_and(|e| e.sequence > 1)
        {
            body = body.child(
                path_button(
                    Button::new("older-conversation"),
                    "Load earlier conversation",
                )
                .on_click(cx.listener(|this, _, _, cx| this.older_conversation(cx))),
            );
        }
        for event in &self.companion.events {
            let row = match &event.payload {
                ConversationPayload::HumanMessage { text } => Some(format!("You\n{text}")),
                ConversationPayload::AssistantMessage { text } => {
                    Some(format!("Companion\n{text}"))
                }
                ConversationPayload::QuestionAsked { question } => {
                    Some(format!("Question\n{}", question.prompt))
                }
                ConversationPayload::QuestionAnswered { answer } => Some(format!(
                    "Your answer: {}{}",
                    answer.choice_id.as_deref().unwrap_or(""),
                    answer
                        .custom_text
                        .as_ref()
                        .map(|s| format!(" {s}"))
                        .unwrap_or_default()
                )),
                ConversationPayload::DocumentProposed { proposal } => Some(match proposal {
                    DocumentProposal::Spec(_) => {
                        "Spec proposal ready. Inspect it in Spec, then Save and Approve separately."
                            .into()
                    }
                    DocumentProposal::Plan(_) => {
                        "Plan proposal ready. Inspect it in Plan, then Save and Approve separately."
                            .into()
                    }
                }),
                ConversationPayload::DocumentSaved { kind, document_ref } => {
                    Some(format!("{kind:?} version {} saved", document_ref.revision))
                }
                ConversationPayload::DocumentApproved { kind, approval } => Some(format!(
                    "{kind:?} version {} approved",
                    approval.document_ref.revision
                )),
                ConversationPayload::ResearchActivity { activity } => match activity {
                    ResearchActivity::PublicBriefPrepared { brief, model } => Some(format!(
                        "Public research · {}\nTopics: {}\nQueries: {}\nURLs: {}",
                        model.display_name,
                        brief.topics.join("; "),
                        brief.queries.join("; "),
                        brief.urls.join("; ")
                    )),
                    ResearchActivity::PublicSearchObserved { observation } => Some(format!(
                        "Public search · {:?}: {}",
                        observation.phase, observation.query
                    )),
                    ResearchActivity::Stopped {
                        detail,
                        continue_available,
                        ..
                    } => Some(format!(
                        "Research: {detail}{}",
                        if *continue_available {
                            " You can explicitly Continue research."
                        } else {
                            ""
                        }
                    )),
                    ResearchActivity::Received { status, .. } => {
                        Some(format!("Research receipt: {status:?}"))
                    }
                    ResearchActivity::Requested { .. } => {
                        Some("Researching relevant context…".into())
                    }
                },
                ConversationPayload::OperationOutcome { outcome, .. } => Some(match outcome {
                    OperationOutcome::Completed { summary } => summary.clone(),
                    OperationOutcome::Failed { detail }
                    | OperationOutcome::Cancelled { detail }
                    | OperationOutcome::Interrupted { detail } => detail.clone(),
                }),
                _ => None,
            };
            if let Some(text) = row {
                body = body.child(
                    solid_section(
                        panel_body(cx).id(SharedString::from(format!("event:{}", event.id))),
                        cx,
                    )
                    .p_3()
                    .border_1()
                    .child(note(text)),
                );
            }
            if matches!(event.payload, ConversationPayload::AssistantMessage { .. })
                && let Some(run) = self.companion.runs.iter().find(|r| {
                    Some(&r.id) == event.run_id.as_ref()
                        && matches!(r.response, Some(CompanionResponse::Improvement(_)))
                })
            {
                body = body.children(
                    historical_run(run)
                        .into_iter()
                        .map(|r| self.historical_row(r, cx)),
                );
            }
            if let ConversationPayload::ResearchFindings { findings } = &event.payload {
                for finding in findings {
                    body = body.child(note(finding.summary.clone()));
                    for (i, citation) in finding.citations.iter().enumerate() {
                        body = body.child(self.source_link(
                            &citation.locator,
                            format!("Source {}", i + 1),
                            cx,
                        ));
                    }
                }
            }
        }
        for (i, receipt) in self.companion.receipts.iter().enumerate() {
            body = body.child(panel_metadata(
                note(format!(
                    "Receipt {} · {:?} · {} / {} bytes · {}",
                    i + 1,
                    receipt.status,
                    receipt.limits.returned_bytes,
                    receipt.limits.max_content_bytes,
                    receipt.content_hash
                )),
                cx,
            ));
            for source in &receipt.sources {
                body = body.child(self.source_link(&source.locator, "Research source".into(), cx));
            }
            for omission in &receipt.omissions {
                body = body.child(panel_warning(note(omission.reason.clone()), cx));
            }
        }
        if let Some(session) = &self.companion.session {
            if let Some(question) = &session.pending_question {
                let mut card = solid_section(panel_body(cx), cx)
                    .p_3()
                    .border_1()
                    .child(panel_heading(note(question.prompt.clone())));
                for choice in &question.choices {
                    let question = question.id.clone();
                    let choice_id = choice.id.clone();
                    card = card.child(
                        path_button(
                            Button::new(SharedString::from(format!("answer:{choice_id}"))),
                            format!("{} · {}", choice.label, choice.description),
                        )
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.answer_companion(&question, Some(choice_id.clone()), w, cx)
                        })),
                    );
                }
                let id = question.id.clone();
                body = body.child(
                    card.child(
                        Textarea::new(&self.companion.custom_answer)
                            .aria_label("Custom clarification answer"),
                    )
                    .child(
                        path_button(Button::new("custom-answer"), "Submit custom answer")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.answer_companion(&id, None, w, cx)
                            })),
                    ),
                );
            }
            if let Ok((walkthrough, action)) = session.current_action() {
                body = body
                    .child(panel_heading(note(format!(
                        "Current action {} of {}",
                        walkthrough.cursor + 1,
                        walkthrough.actions.len()
                    ))))
                    .child(note(action.what.clone()))
                    .child(note(action.why.clone()))
                    .child(note(format!("Expected: {}", action.expected_result)));
            }
            if let Some(plan) = &session.plan {
                body = body
                    .child(panel_heading(note("Plan outline · inspection only")))
                    .children(plan.task.steps.iter().enumerate().map(|(i, step)| {
                        let id = step.id.clone();
                        path_button(
                            Button::new(SharedString::from(format!("inspect:{id}"))),
                            format!("{}. {} · {:?}", i + 1, step.title, step.status),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.companion.inspected_step = Some(id.clone());
                            cx.notify();
                        }))
                    }));
                if let Some(step) = plan
                    .task
                    .steps
                    .iter()
                    .find(|s| Some(&s.id) == self.companion.inspected_step.as_ref())
                {
                    body = body
                        .child(note(step.why.clone()))
                        .child(note(format!("Done when: {}", step.done_when.join("; "))));
                }
            }
        }
        body = body
            .child(
                path_button(Button::new("show-context"), "Context details").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.companion.show_context = !this.companion.show_context;
                        cx.notify();
                    },
                )),
            )
            .when(self.companion.show_context, |d| {
                d.child(
                    Textarea::new(&self.companion.context_paths)
                        .aria_label("Additional context paths"),
                )
                .child(
                    path_button(Button::new("refresh-context"), "Refresh context")
                        .disabled(self.companion.context_busy)
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_companion_context(cx))),
                )
                .when_some(self.companion.context.as_ref(), |d, c| {
                    d.child(note(format!(
                        "{} files · {} omissions · {}",
                        c.files.len(),
                        c.omitted.len(),
                        if self.companion.context_epoch == self.companion.code_epoch {
                            "current capture"
                        } else {
                            "historical capture"
                        }
                    )))
                    .children(c.files.iter().map(|f| note(f.path.clone())))
                })
            })
            .child(self.guidance_controls(cx))
            .child(Textarea::new(&self.companion.question).aria_label("Conversation message"))
            .child(
                div().h_flex().flex_wrap().gap_2().children(
                    [
                        (CompanionAction::Ask, "Send"),
                        (CompanionAction::Analyze, "Analyze / discuss"),
                        (CompanionAction::DraftSpec, "Draft Spec"),
                        (CompanionAction::DraftPlan, "Draft Plan"),
                        (CompanionAction::Guide, "Prepare current action"),
                        (CompanionAction::Hint, "Hint"),
                        (CompanionAction::Explain, "Explain"),
                        (CompanionAction::ImproveClarity, "Improve clarity"),
                        (CompanionAction::ImproveSpeed, "Improve speed"),
                        (CompanionAction::ImproveBalanced, "Improve balance"),
                        (CompanionAction::Review, "Review current action"),
                        (CompanionAction::Reveal, "Reveal suggested solution"),
                    ]
                    .into_iter()
                    .map(|(action, label)| {
                        wrapping_button(Button::new(label), label)
                            .disabled(busy || self.companion.session.is_none())
                            .on_click(
                                cx.listener(move |this, _, w, cx| {
                                    this.run_companion(action, w, cx)
                                }),
                            )
                    }),
                ),
            )
            .child(
                path_button(Button::new("continue-research"), "Continue research")
                    .disabled(busy || self.companion.session.is_none())
                    .on_click(cx.listener(|this, _, w, cx| {
                        this.start_conversation(
                            tandem_companion::ConversationIntent::Message(
                                "Continue the unfinished research within a new bounded operation."
                                    .into(),
                            ),
                            w,
                            cx,
                        )
                    })),
            );
        if self.companion.solution_scope.as_ref().is_some_and(|scope| {
            super::companion_state::solution_visible(
                scope,
                self.companion.session.as_ref(),
                &self.companion.language,
                &self.effective_guidance(),
            )
        }) && let Some((summary, editor)) = &self.companion.solution
        {
            body = body
                .child(panel_heading(note("Revealed suggestion · read only")))
                .child(note(summary.clone()))
                .child(
                    Editor::new(editor)
                        .readonly(true)
                        .aria_label("Explicitly revealed suggested solution")
                        .h(px(230.)),
                );
        }
        body.into_any_element()
    }
    fn historical_row(&self, row: HistoricalRow, cx: &mut Context<Self>) -> AnyElement {
        let mut body = panel_body(cx).child(note(row.text));
        if let Some((path, line)) = row.source {
            let label = format!("Historical source: {path}:{line}");
            body = body.child(
                path_button(
                    Button::new(SharedString::from(format!("history-source:{path}:{line}"))),
                    label,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_file(path.clone(), false, Some(line as usize), window, cx)
                })),
            );
        }
        if let Some(id) = row.check_id
            && self.dev.checks.iter().any(|c| c.id == id)
        {
            body = body.child(
                path_button(
                    Button::new(SharedString::from(format!("history-check:{id}"))),
                    "Inspect saved check receipt",
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.dev.selected_check = Some(id.clone());
                    this.dev.panel = super::tools::Panel::Checks;
                    cx.notify();
                })),
            );
        }
        body.into_any_element()
    }
    fn source_link(
        &self,
        locator: &SourceLocator,
        label: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match locator {
            SourceLocator::PublicUrl { url } => {
                let url = url.clone();
                path_button(
                    Button::new(SharedString::from(format!("source:{url}"))),
                    format!("{label}: {url}"),
                )
                .on_click(move |_, _, cx| cx.open_url(&url))
                .into_any_element()
            }
            SourceLocator::FileRange {
                path, start_line, ..
            } => {
                let path = path.clone();
                let line = *start_line;
                path_button(
                    Button::new(SharedString::from(format!("source:{path}:{line}"))),
                    format!("{label}: {path}:{line}"),
                )
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.open_file(path.clone(), false, Some(line as usize), w, cx)
                }))
                .into_any_element()
            }
            _ => panel_metadata(note(label), cx).into_any_element(),
        }
    }
}
