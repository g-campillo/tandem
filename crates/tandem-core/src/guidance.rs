use crate::{StepStatus, TaskPlan, content_hash};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstructionDetail {
    HighLevel,
    #[default]
    Standard,
    Detailed,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExplanationDepth {
    #[default]
    Concise,
    Standard,
    Detailed,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SolutionPolicy {
    #[default]
    Hidden,
    OnRequest,
    AutomaticPreview,
}
impl std::fmt::Display for InstructionDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::HighLevel => "High-level",
            Self::Standard => "Standard",
            Self::Detailed => "Detailed",
        })
    }
}
impl std::fmt::Display for ExplanationDepth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Concise => "Concise",
            Self::Standard => "Standard",
            Self::Detailed => "Detailed",
        })
    }
}
impl std::fmt::Display for SolutionPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Hidden => "Hidden",
            Self::OnRequest => "On request",
            Self::AutomaticPreview => "Automatic preview",
        })
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuidancePatch {
    pub instructions: Option<InstructionDetail>,
    pub explanation: Option<ExplanationDepth>,
    pub solution: Option<SolutionPolicy>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProjectGuidance {
    pub defaults: GuidancePatch,
    pub languages: BTreeMap<String, GuidancePatch>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StepGuidance {
    pub plan_revision: u64,
    pub defaults: GuidancePatch,
    pub languages: BTreeMap<String, GuidancePatch>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GuidanceSettings {
    pub sequence: u64,
    pub defaults: GuidancePatch,
    pub languages: BTreeMap<String, GuidancePatch>,
    pub projects: BTreeMap<String, ProjectGuidance>,
    pub steps: BTreeMap<String, StepGuidance>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EffectiveGuidance {
    pub instructions: InstructionDetail,
    pub explanation: ExplanationDepth,
    pub solution: SolutionPolicy,
    pub instruction_source: String,
    pub explanation_source: String,
    pub solution_source: String,
}
impl EffectiveGuidance {
    pub fn fingerprint(&self) -> String {
        content_hash(serde_json::to_string(self).unwrap().as_bytes())
    }
    fn apply(&mut self, patch: &GuidancePatch, source: &str) {
        if let Some(value) = patch.instructions {
            self.instructions = value;
            self.instruction_source = source.into();
        }
        if let Some(value) = patch.explanation {
            self.explanation = value;
            self.explanation_source = source.into();
        }
        if let Some(value) = patch.solution {
            self.solution = value;
            self.solution_source = source.into();
        }
    }
}
impl GuidanceSettings {
    pub fn effective(
        &self,
        project: &str,
        language: &str,
        task: Option<&TaskPlan>,
    ) -> EffectiveGuidance {
        let mut result = EffectiveGuidance {
            instructions: InstructionDetail::default(),
            explanation: ExplanationDepth::default(),
            solution: SolutionPolicy::default(),
            instruction_source: "Built-in default".into(),
            explanation_source: "Built-in default".into(),
            solution_source: "Built-in default".into(),
        };
        result.apply(&self.defaults, "Global");
        if let Some(patch) = self.languages.get(language) {
            result.apply(patch, &format!("Global {language}"));
        }
        if let Some(project) = self.projects.get(project) {
            result.apply(&project.defaults, "Project");
            if let Some(patch) = project.languages.get(language) {
                result.apply(patch, &format!("Project {language}"));
            }
        }
        if let Some(task) = task
            && task.project == project
            && let Some(id) = &task.active_step
            && task
                .steps
                .iter()
                .any(|s| &s.id == id && s.status == StepStatus::Active)
            && let Some(step) = self.steps.get(id)
            && step.plan_revision == task.revision
        {
            result.apply(&step.defaults, "Current step");
            if let Some(patch) = step.languages.get(language) {
                result.apply(patch, &format!("Current step {language}"));
            }
        }
        result
    }
    pub fn clear_step(&mut self, id: &str) {
        if self.steps.remove(id).is_some() {
            self.sequence += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_language_fields_resolve_independently_and_step_overrides_expire() {
        let mut settings = GuidanceSettings::default();
        settings.defaults.instructions = Some(InstructionDetail::HighLevel);
        settings.languages.insert(
            "rust".into(),
            GuidancePatch {
                explanation: Some(ExplanationDepth::Detailed),
                ..Default::default()
            },
        );
        settings.projects.insert(
            "p".into(),
            ProjectGuidance {
                defaults: GuidancePatch {
                    explanation: Some(ExplanationDepth::Standard),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let mut task =
            TaskPlan::new("p".into(), "Goal".into(), vec![], vec!["Step".into()]).unwrap();
        task.start_step(&task.steps[0].id.clone()).unwrap();
        settings.steps.insert(
            task.steps[0].id.clone(),
            StepGuidance {
                plan_revision: task.revision,
                languages: BTreeMap::from([(
                    "python".into(),
                    GuidancePatch {
                        explanation: Some(ExplanationDepth::Detailed),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            },
        );
        let rust = settings.effective("p", "rust", Some(&task));
        assert_eq!(rust.explanation, ExplanationDepth::Standard);
        assert_eq!(rust.explanation_source, "Project");
        assert_eq!(rust.instructions, InstructionDetail::HighLevel);
        assert_eq!(rust.solution, SolutionPolicy::Hidden);
        assert_eq!(
            settings
                .effective("p", "python", Some(&task))
                .explanation_source,
            "Current step python"
        );
        task.complete_active_manually(vec![]).unwrap();
        assert_eq!(
            settings
                .effective("p", "python", Some(&task))
                .explanation_source,
            "Project"
        );
        settings.projects.get_mut("p").unwrap().defaults.explanation = None;
        assert_eq!(
            settings
                .effective("p", "rust", Some(&task))
                .explanation_source,
            "Global rust"
        );
    }
}
