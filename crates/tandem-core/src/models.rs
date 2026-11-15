//! Persisted model choices and immutable runtime model snapshots. No protocol dependency.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelPreference {
    #[default]
    Automatic,
    Explicit {
        model: String,
    },
}
impl ModelPreference {
    pub fn validate(&self) -> Result<()> {
        if let Self::Explicit { model } = self {
            validate_model_identifier(model)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableModel {
    pub model: String,
    pub catalog_id: String,
    pub display_name: String,
    pub description: String,
    pub is_default: bool,
    pub supported_efforts: Vec<String>,
    /// None only when the runtime explicitly advertises no supported effort.
    pub default_effort: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCatalog {
    pub models: Vec<AvailableModel>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedModel {
    /// Runtime model identifier, distinct from the picker/catalog entry id.
    pub model: String,
    pub catalog_id: String,
    pub display_name: String,
    pub effort: Option<String>,
}
impl ResolvedModel {
    pub fn validate(&self) -> Result<()> {
        validate_model_identifier(&self.model)?;
        validate_model_identifier(&self.catalog_id)?;
        ensure!(
            !self.display_name.trim().is_empty()
                && self.display_name.len() <= 256
                && !self.display_name.chars().any(char::is_control),
            "The runtime model display name is invalid; refresh the model catalog"
        );
        if let Some(effort) = &self.effort {
            validate_model_identifier(effort)?;
        }
        Ok(())
    }
    pub fn same_configuration(&self, other: &Self) -> bool {
        self.model == other.model
            && self.catalog_id == other.catalog_id
            && self.effort == other.effort
    }
}

pub fn validate_model_identifier(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 200
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.:/".contains(&c)),
        "The runtime model identifier or effort is invalid; refresh the model catalog"
    );
    Ok(())
}
