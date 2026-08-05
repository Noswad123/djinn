use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ConfigDoctorReport {
    pub(crate) source: String,
    pub(crate) checked_paths: Vec<String>,
    pub(crate) files: Vec<ConfigDoctorFileReport>,
    pub(crate) summary: ConfigDoctorSummary,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub(crate) struct ConfigDoctorSummary {
    pub(crate) checked_path_count: usize,
    pub(crate) readable_file_count: usize,
    pub(crate) mapped_count: usize,
    pub(crate) unsupported_count: usize,
    pub(crate) unknown_count: usize,
    pub(crate) secret_count: usize,
    pub(crate) error_count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ConfigDoctorFileReport {
    pub(crate) path: String,
    pub(crate) exists: bool,
    pub(crate) readable: bool,
    pub(crate) mapped: Vec<ConfigDoctorFinding>,
    pub(crate) unsupported: Vec<ConfigDoctorFinding>,
    pub(crate) unknown: Vec<ConfigDoctorFinding>,
    pub(crate) secrets: Vec<ConfigDoctorFinding>,
    pub(crate) errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ConfigDoctorFinding {
    pub(crate) pointer: String,
    pub(crate) concept: String,
    pub(crate) djinn_mapping: String,
    pub(crate) detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DjinnConfig {
    #[serde(default = "default_djinn_config_version")]
    pub(crate) version: u16,
    #[serde(default)]
    pub(crate) default_profile: Option<String>,
    #[serde(default)]
    pub(crate) providers: BTreeMap<String, DjinnConfigProvider>,
    #[serde(default)]
    pub(crate) profiles: BTreeMap<String, DjinnConfigProfile>,
    #[serde(default)]
    pub(crate) permissions: Vec<DjinnConfigPermission>,
    #[serde(default)]
    pub(crate) instructions: BTreeMap<String, DjinnConfigInstruction>,
    #[serde(default)]
    pub(crate) commands: BTreeMap<String, DjinnConfigCommandTemplate>,
    #[serde(default)]
    pub(crate) tools: BTreeMap<String, DjinnConfigTool>,
    #[serde(default)]
    pub(crate) agents: BTreeMap<String, DjinnConfigAgent>,
}

impl Default for DjinnConfig {
    fn default() -> Self {
        Self {
            version: default_djinn_config_version(),
            default_profile: None,
            providers: BTreeMap::new(),
            profiles: BTreeMap::new(),
            permissions: Vec::new(),
            instructions: BTreeMap::new(),
            commands: BTreeMap::new(),
            tools: BTreeMap::new(),
            agents: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(crate) struct DjinnConfigProvider {
    #[serde(rename = "type")]
    pub(crate) provider_type: String,
    #[serde(default)]
    pub(crate) auth: Option<String>,
    #[serde(default)]
    pub(crate) endpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(crate) struct DjinnConfigProfile {
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) instructions: Vec<String>,
    #[serde(default)]
    pub(crate) permissions: Vec<DjinnConfigPermission>,
    #[serde(default)]
    pub(crate) tools: Vec<String>,
    #[serde(default)]
    pub(crate) agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DjinnConfigPermission {
    pub(crate) action: String,
    #[serde(default = "default_permission_resource")]
    pub(crate) resource: String,
    pub(crate) effect: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(crate) struct DjinnConfigInstruction {
    #[serde(default)]
    pub(crate) path: Option<String>,
    #[serde(default)]
    pub(crate) text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(crate) struct DjinnConfigCommandTemplate {
    #[serde(default)]
    pub(crate) prompt: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(crate) struct DjinnConfigTool {
    #[serde(default)]
    pub(crate) enabled: Option<bool>,
    #[serde(default)]
    pub(crate) permission: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(crate) struct DjinnConfigAgent {
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) profile: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) instructions: Vec<String>,
    #[serde(default)]
    pub(crate) tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct DjinnConfigLoadReport {
    pub(crate) checked_paths: Vec<String>,
    pub(crate) files: Vec<DjinnConfigFileReport>,
    pub(crate) effective: DjinnConfig,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct DjinnConfigFileReport {
    pub(crate) path: String,
    pub(crate) exists: bool,
    pub(crate) readable: bool,
    pub(crate) errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ConfigImportPreview {
    pub(crate) source: String,
    pub(crate) mode: String,
    pub(crate) checked_paths: Vec<String>,
    pub(crate) readable_files: Vec<String>,
    pub(crate) patch: DjinnConfigPatchPreview,
    pub(crate) unsupported: Vec<ConfigDoctorFinding>,
    pub(crate) unknown: Vec<ConfigDoctorFinding>,
    pub(crate) secrets: Vec<ConfigDoctorFinding>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ConfigImportWriteReport {
    pub(crate) source: String,
    pub(crate) mode: String,
    pub(crate) path: String,
    pub(crate) overwritten: bool,
    pub(crate) merged: bool,
    pub(crate) summary: ConfigImportWriteSummary,
    pub(crate) config: DjinnConfig,
    pub(crate) unsupported: Vec<ConfigDoctorFinding>,
    pub(crate) unknown: Vec<ConfigDoctorFinding>,
    pub(crate) secrets: Vec<ConfigDoctorFinding>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub(crate) struct ConfigImportWriteSummary {
    pub(crate) applied_default_profile: Option<String>,
    pub(crate) preserved_default_profile: Option<String>,
    pub(crate) skipped_import_default_profile: Option<String>,
    pub(crate) added_providers: Vec<String>,
    pub(crate) skipped_providers: Vec<String>,
    pub(crate) added_profiles: Vec<String>,
    pub(crate) skipped_profiles: Vec<String>,
    pub(crate) added_shared_permissions: usize,
    pub(crate) skipped_shared_permissions: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ConfigExportPreview {
    pub(crate) target: String,
    pub(crate) mode: String,
    pub(crate) checked_paths: Vec<String>,
    pub(crate) readable_files: Vec<String>,
    pub(crate) config: Value,
    pub(crate) unsupported: Vec<ConfigDoctorFinding>,
    pub(crate) secrets: Vec<ConfigDoctorFinding>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ConfigExportWriteReport {
    pub(crate) target: String,
    pub(crate) mode: String,
    pub(crate) path: String,
    pub(crate) overwritten: bool,
    pub(crate) config: Value,
    pub(crate) unsupported: Vec<ConfigDoctorFinding>,
    pub(crate) secrets: Vec<ConfigDoctorFinding>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct DjinnConfigPatchPreview {
    pub(crate) version: u16,
    pub(crate) default_profile: Option<String>,
    pub(crate) providers: BTreeMap<String, DjinnProviderPatchPreview>,
    pub(crate) profiles: BTreeMap<String, DjinnProfilePatchPreview>,
    pub(crate) permissions: Vec<DjinnPermissionPatchPreview>,
}

impl Default for DjinnConfigPatchPreview {
    fn default() -> Self {
        Self {
            version: 1,
            default_profile: None,
            providers: BTreeMap::new(),
            profiles: BTreeMap::new(),
            permissions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub(crate) struct DjinnProviderPatchPreview {
    #[serde(rename = "type")]
    pub(crate) provider_type: String,
    pub(crate) auth: Option<String>,
    pub(crate) source_pointers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub(crate) struct DjinnProfilePatchPreview {
    pub(crate) model: Option<String>,
    pub(crate) instructions: Vec<String>,
    pub(crate) permissions: Vec<DjinnPermissionPatchPreview>,
    pub(crate) source_pointers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct DjinnPermissionPatchPreview {
    pub(crate) action: String,
    pub(crate) resource: String,
    pub(crate) effect: String,
    pub(crate) source_pointer: String,
}

fn default_djinn_config_version() -> u16 {
    1
}

fn default_permission_resource() -> String {
    "*".to_string()
}
