use super::*;
use gpui_kit::component::input::{CompletionProvider, DefinitionProvider, Rope};
use lsp_types::{CompletionContext, CompletionResponse, GotoDefinitionResponse, LocationLink};
use tandem_lsp::{LanguageManager, position_params};

#[derive(Clone)]
pub struct Provider {
    pub manager: LanguageManager,
    pub file: PathBuf,
    pub instance: String,
}
impl CompletionProvider for Provider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let provider = self.clone();
        let text = text.to_string();
        cx.background_spawn(async move {
            let mut params = position_params(&text, offset)?;
            params["context"] = serde_json::to_value(trigger)?;
            let reply = provider
                .manager
                .request(
                    "textDocument/completion",
                    &provider.file,
                    &provider.instance,
                    &text,
                    params,
                )
                .await?;
            anyhow::ensure!(
                provider.manager.is_current(&reply.stamp),
                "Stale completion"
            );
            let response: CompletionResponse = if reply.value.is_null() {
                CompletionResponse::Array(vec![])
            } else {
                serde_json::from_value(reply.value)?
            };
            let mut items = match response {
                CompletionResponse::Array(items) => items,
                CompletionResponse::List(list) => list.items,
            };
            let prefix = text[..offset]
                .chars()
                .rev()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>();
            items.retain(|item| {
                item.label.starts_with(&prefix)
                    && item.insert_text_format != Some(lsp_types::InsertTextFormat::SNIPPET)
            });
            for item in &mut items {
                item.command = None;
                item.additional_text_edits = None;
            }
            items.truncate(100);
            Ok(CompletionResponse::Array(items))
        })
    }
    fn is_completion_trigger(&self, _: usize, text: &str, _: &mut App) -> bool {
        text == "."
            || text == ":"
            || text
                .chars()
                .last()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
    }
}
impl DefinitionProvider for Provider {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        let provider = self.clone();
        let text = text.to_string();
        cx.background_spawn(async move {
            let reply = provider
                .manager
                .request(
                    "textDocument/definition",
                    &provider.file,
                    &provider.instance,
                    &text,
                    position_params(&text, offset)?,
                )
                .await?;
            anyhow::ensure!(
                provider.manager.is_current(&reply.stamp),
                "Stale definition"
            );
            if reply.value.is_null() {
                return Ok(vec![]);
            }
            let response: GotoDefinitionResponse = serde_json::from_value(reply.value)?;
            Ok(match response {
                GotoDefinitionResponse::Link(links) => links,
                other => {
                    let locations = match other {
                        GotoDefinitionResponse::Scalar(location) => vec![location],
                        GotoDefinitionResponse::Array(locations) => locations,
                        _ => unreachable!(),
                    };
                    locations
                        .into_iter()
                        .map(|location| LocationLink {
                            origin_selection_range: None,
                            target_uri: location.uri,
                            target_range: location.range,
                            target_selection_range: location.range,
                        })
                        .collect()
                }
            })
        })
    }
}
