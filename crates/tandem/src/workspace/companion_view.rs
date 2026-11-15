use super::companion::PreferenceScope;
use super::panels::note;
use super::*;
use crate::appearance::{
    compact_button, navigation_button, panel_body, panel_metadata, path_button, solid_section,
    wrapping_button,
};
use gpui_kit::component::button::Button;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
impl Workspace {
    pub(super) fn guidance_controls(&self, cx: &mut Context<Self>) -> AnyElement {
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
                .child(note("AutomaticPreview is a standing request for a suggestion for the current action/language after document approvals and Start. It still grants no source-edit authority. Hidden and OnRequest both keep implementation content out until you reveal it.")))
            .into_any_element()
    }
}
