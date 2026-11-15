//! Parsing for the pinned 0.169.0 model/list protocol, separate from shared storage types.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;
use tandem_core::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogPage {
    data: Vec<ProtocolModel>,
    next_cursor: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProtocolModel {
    id: String,
    model: String,
    display_name: String,
    description: String,
    hidden: bool,
    is_default: bool,
    supported_reasoning_efforts: Vec<EffortOption>,
    default_reasoning_effort: String,
    // This is the pinned protocol's documented serde default, not a model guess.
    #[serde(default = "default_modalities")]
    input_modalities: Vec<String>,
}
fn default_modalities() -> Vec<String> {
    vec!["text".into(), "image".into()]
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EffortOption {
    reasoning_effort: String,
    description: String,
}

pub(crate) fn parse_catalog_page(value: Value) -> Result<(Vec<AvailableModel>, Option<String>)> {
    let page: CatalogPage = serde_json::from_value(value).map_err(|_| {
        anyhow::anyhow!("The runtime model catalog is malformed; refresh availability")
    })?;
    ensure!(
        page.data.len() <= 100,
        "The runtime model catalog page exceeds 100 entries"
    );
    if let Some(cursor) = &page.next_cursor {
        ensure!(
            !cursor.is_empty() && cursor.len() <= 4096 && !cursor.chars().any(char::is_control),
            "The runtime returned an invalid model catalog cursor"
        );
    }
    let mut models = Vec::new();
    for entry in page.data {
        validate_model_identifier(&entry.id)?;
        validate_model_identifier(&entry.model)?;
        validate_model_identifier(&entry.default_reasoning_effort)?;
        ensure!(
            !entry.hidden,
            "The runtime returned a hidden model in the visible catalog"
        );
        ensure!(
            entry.input_modalities.iter().any(|m| m == "text"),
            "The runtime catalog contains a model without text support"
        );
        ensure!(
            entry.description.len() <= 4096,
            "The runtime model description exceeds its limit"
        );
        let mut efforts = BTreeSet::new();
        for effort in &entry.supported_reasoning_efforts {
            validate_model_identifier(&effort.reasoning_effort)?;
            ensure!(
                effort.description.len() <= 4096 && efforts.insert(effort.reasoning_effort.clone()),
                "The runtime model has invalid or duplicate supported efforts"
            );
        }
        let default_effort = if efforts.is_empty() {
            None
        } else {
            ensure!(
                efforts.contains(&entry.default_reasoning_effort),
                "The runtime model's default reasoning effort is unsupported; refresh availability"
            );
            Some(entry.default_reasoning_effort)
        };
        ResolvedModel {
            model: entry.model.clone(),
            catalog_id: entry.id.clone(),
            display_name: entry.display_name.clone(),
            effort: default_effort.clone(),
        }
        .validate()?;
        models.push(AvailableModel {
            model: entry.model,
            catalog_id: entry.id,
            display_name: entry.display_name,
            description: entry.description,
            is_default: entry.is_default,
            supported_efforts: efforts.into_iter().collect(),
            default_effort,
        });
    }
    Ok((models, page.next_cursor))
}

pub(crate) fn validate_catalog(catalog: &ModelCatalog) -> Result<()> {
    ensure!(
        !catalog.models.is_empty(),
        "No available text models were returned; refresh availability"
    );
    ensure!(
        catalog.models.len() <= 1000,
        "The runtime model catalog exceeds 1000 entries"
    );
    let mut ids = BTreeSet::new();
    let mut models = BTreeSet::new();
    for model in &catalog.models {
        ensure!(
            ids.insert(&model.catalog_id) && models.insert(&model.model),
            "The runtime model catalog contains duplicate identities; refresh availability"
        );
    }
    ensure!(
        catalog.models.iter().filter(|m| m.is_default).count() == 1,
        "The runtime model catalog must advertise exactly one account default; refresh availability"
    );
    Ok(())
}

pub fn resolve_model(
    catalog: &ModelCatalog,
    preference: &ModelPreference,
) -> Result<ResolvedModel> {
    preference.validate()?;
    validate_catalog(catalog)?;
    let selected = match preference {
        ModelPreference::Automatic => catalog.models.iter().find(|m| m.is_default),
        ModelPreference::Explicit { model } => catalog.models.iter().find(|m| &m.model == model),
    }.context("The selected model is unavailable for this account. Refresh availability or select another model; the saved choice is retained.")?;
    ensure!(
        selected
            .default_effort
            .as_ref()
            .map_or(selected.supported_efforts.is_empty(), |effort| selected
                .supported_efforts
                .contains(effort)),
        "The selected model's default reasoning effort is unsupported"
    );
    let resolved = ResolvedModel {
        model: selected.model.clone(),
        catalog_id: selected.catalog_id.clone(),
        display_name: selected.display_name.clone(),
        effort: selected.default_effort.clone(),
    };
    resolved.validate()?;
    Ok(resolved)
}
