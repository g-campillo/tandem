use super::*;
use gpui_kit::component::input::{DefinitionProvider, Rope};
use lsp_types::{GotoDefinitionResponse, LocationLink};
use tandem_lsp::{LanguageManager, position_params};

#[derive(Clone)]
pub struct Provider {
    pub manager: LanguageManager,
    pub file: PathBuf,
    pub instance: String,
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
