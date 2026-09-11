use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

pub const PROJECT_SCHEMA: &str = "forge.project.v1";
pub const PROJECT_SCHEMA_VERSION: u32 = 1;
pub const CAPABILITIES_SCHEMA: &str = "forge.capabilities.v1";

#[derive(Debug, Error)]
pub enum ContractError {
    #[error("project contract not found: {0}")]
    Missing(PathBuf),
    #[error("failed to read project contract {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("failed to parse project contract {path}: {source}")]
    Parse { path: PathBuf, source: serde_json::Error },
    #[error("project contract schema is required")]
    MissingSchema,
    #[error("unsupported project contract schema: {0}")]
    Schema(String),
    #[error("project contract schema_version is required")]
    MissingSchemaVersion,
    #[error("unsupported project contract schema_version: {0}")]
    SchemaVersion(u32),
    #[error("invalid project contract: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProjectContract {
    pub schema: String,
    pub schema_version: Option<u32>,
    pub project: ProjectIdentity,
    pub root_control_center: RootControlCenter,
    pub commands: Vec<CommandDescriptor>,
    pub quality_gates: Vec<QualityGate>,
    pub updates: serde_json::Value,
    pub artifacts: Vec<ArtifactDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProjectIdentity {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub version: String,
    pub build: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RootControlCenter {
    pub launcher: String,
    pub entrypoint: String,
    pub machine_provider: String,
    pub python_provider: String,
    pub implementation_authority: String,
}

impl RootControlCenter {
    #[must_use]
    pub fn provider_path(&self) -> Option<&str> {
        for value in [&self.machine_provider, &self.python_provider] {
            if !value.trim().is_empty() {
                return Some(value.as_str());
            }
        }
        None
    }

    #[must_use]
    pub fn launcher_path(&self) -> Option<&str> {
        for value in [&self.launcher, &self.entrypoint] {
            if !value.trim().is_empty() {
                return Some(value.as_str());
            }
        }
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CommandDescriptor {
    pub key: String,
    pub label: String,
    pub category: String,
    pub risk: String,
    pub program: String,
    pub args: Vec<String>,
    pub mutates: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct QualityGate {
    pub key: String,
    pub label: String,
    pub stages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ArtifactDescriptor {
    pub key: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectCapabilities {
    pub schema: String,
    pub schema_version: u32,
    pub project: CapabilityProject,
    pub contract: CapabilityContract,
    pub provider: CapabilityProvider,
    pub operations: Vec<CapabilityOperation>,
    pub gates: Vec<CapabilityGate>,
    pub artifacts: Vec<CapabilityArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityProject {
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityContract {
    pub schema: String,
    pub schema_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityProvider {
    pub configured: bool,
    pub path: Option<String>,
    pub ready: bool,
    pub implementation_authority: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityOperation {
    pub key: String,
    pub label: String,
    pub category: String,
    pub risk: String,
    pub mutates: bool,
    pub source: String,
    pub provider_command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityGate {
    pub key: String,
    pub label: String,
    pub stages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityArtifact {
    pub key: String,
    pub path: String,
}

#[derive(Debug, Clone, Copy)]
pub struct CanonicalProviderOperation {
    pub key: &'static str,
    pub provider_command: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    pub risk: &'static str,
    pub mutates: bool,
}

pub const CANONICAL_PROVIDER_OPERATIONS: &[CanonicalProviderOperation] = &[
    CanonicalProviderOperation { key: "project.status", provider_command: "status-json", label: "Project status", category: "project", risk: "read_only", mutates: false },
    CanonicalProviderOperation { key: "gate.quick", provider_command: "quick", label: "Quick quality gate", category: "gate", risk: "local_mutation", mutates: true },
    CanonicalProviderOperation { key: "gate.fast", provider_command: "fast", label: "Fast quality gate", category: "gate", risk: "local_mutation", mutates: true },
    CanonicalProviderOperation { key: "gate.full", provider_command: "full", label: "Full quality gate", category: "gate", risk: "local_mutation", mutates: true },
    CanonicalProviderOperation { key: "build.native", provider_command: "build", label: "Build project", category: "build", risk: "local_mutation", mutates: true },
    CanonicalProviderOperation { key: "build.release", provider_command: "build-release", label: "Build release", category: "build", risk: "local_mutation", mutates: true },
    CanonicalProviderOperation { key: "run.gui", provider_command: "launch-gui", label: "Run project GUI", category: "run", risk: "local_mutation", mutates: true },
    CanonicalProviderOperation { key: "project.self-test", provider_command: "self-test", label: "Project control self-test", category: "diagnostics", risk: "read_only", mutates: false },
    CanonicalProviderOperation { key: "diagnostics.bundle", provider_command: "debug-bundle", label: "Create debug bundle", category: "diagnostics", risk: "local_mutation", mutates: true },
    CanonicalProviderOperation { key: "patch.status", provider_command: "patch-status", label: "Patch status", category: "updates", risk: "read_only", mutates: false },
    CanonicalProviderOperation { key: "patch.apply", provider_command: "patch-apply", label: "Apply validated patch queue", category: "updates", risk: "source_mutation", mutates: true },
    CanonicalProviderOperation { key: "git.status", provider_command: "git-status", label: "Git status", category: "source-control", risk: "read_only", mutates: false },
    CanonicalProviderOperation { key: "git.commit-green", provider_command: "commit-green", label: "Commit certified GREEN", category: "source-control", risk: "source_mutation", mutates: true },
    CanonicalProviderOperation { key: "git.commit-push-green", provider_command: "commit-push-green", label: "Commit and push certified GREEN", category: "source-control", risk: "network_mutation", mutates: true },
    CanonicalProviderOperation { key: "git.push", provider_command: "push", label: "Push committed source", category: "source-control", risk: "network_mutation", mutates: true },
    CanonicalProviderOperation { key: "doctor.status", provider_command: "doctor-json", label: "Project doctor status", category: "diagnostics", risk: "read_only", mutates: false },
];

#[must_use]
pub fn provider_command(operation: &str) -> Option<&'static str> {
    CANONICAL_PROVIDER_OPERATIONS
        .iter()
        .find(|item| item.key == operation)
        .map(|item| item.provider_command)
}

impl ProjectContract {
    pub fn load(root: &Path) -> Result<Self, ContractError> {
        let path = root.join("project.control.json");
        if !path.is_file() {
            return Err(ContractError::Missing(path));
        }
        let text = fs::read_to_string(&path).map_err(|source| ContractError::Read {
            path: path.clone(),
            source,
        })?;
        let contract: Self = serde_json::from_str(&text).map_err(|source| ContractError::Parse {
            path: path.clone(),
            source,
        })?;
        contract.validate()?;
        Ok(contract)
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema.trim().is_empty() {
            return Err(ContractError::MissingSchema);
        }
        if self.schema != PROJECT_SCHEMA {
            return Err(ContractError::Schema(self.schema.clone()));
        }
        let Some(schema_version) = self.schema_version else {
            return Err(ContractError::MissingSchemaVersion);
        };
        if schema_version != PROJECT_SCHEMA_VERSION {
            return Err(ContractError::SchemaVersion(schema_version));
        }
        require_non_empty("project.id", &self.project.id)?;
        require_non_empty("project.name", &self.project.name)?;
        require_non_empty("project.kind", &self.project.kind)?;

        if let Some(path) = self.root_control_center.provider_path() {
            validate_relative_contract_path("root_control_center provider", path)?;
        }
        if let Some(path) = self.root_control_center.launcher_path() {
            validate_relative_contract_path("root_control_center launcher", path)?;
        }

        let mut command_keys = BTreeSet::new();
        for command in &self.commands {
            require_non_empty("command.key", &command.key)?;
            require_non_empty("command.program", &command.program)?;
            if !command_keys.insert(command.key.as_str()) {
                return Err(ContractError::Invalid(format!("duplicate command key: {}", command.key)));
            }
        }

        let mut gate_keys = BTreeSet::new();
        for gate in &self.quality_gates {
            require_non_empty("quality_gate.key", &gate.key)?;
            if !gate_keys.insert(gate.key.as_str()) {
                return Err(ContractError::Invalid(format!("duplicate quality gate key: {}", gate.key)));
            }
            for stage in &gate.stages {
                if !command_keys.contains(stage.as_str()) {
                    return Err(ContractError::Invalid(format!(
                        "quality gate `{}` references unknown command stage `{stage}`",
                        gate.key
                    )));
                }
            }
        }

        let mut artifact_keys = BTreeSet::new();
        for artifact in &self.artifacts {
            require_non_empty("artifact.key", &artifact.key)?;
            require_non_empty("artifact.path", &artifact.path)?;
            validate_relative_contract_path("artifact.path", &artifact.path)?;
            if !artifact_keys.insert(artifact.key.as_str()) {
                return Err(ContractError::Invalid(format!("duplicate artifact key: {}", artifact.key)));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn capabilities(&self, root: &Path) -> ProjectCapabilities {
        let provider_path = self.root_control_center.provider_path().map(ToOwned::to_owned);
        let provider_ready = provider_path
            .as_deref()
            .is_some_and(|path| root.join(path).is_file());
        let provider_configured = provider_path.is_some();
        let mut operations = Vec::new();
        let mut seen = BTreeSet::new();

        if provider_configured {
            for operation in CANONICAL_PROVIDER_OPERATIONS {
                seen.insert(operation.key);
                operations.push(CapabilityOperation {
                    key: operation.key.to_owned(),
                    label: operation.label.to_owned(),
                    category: operation.category.to_owned(),
                    risk: operation.risk.to_owned(),
                    mutates: operation.mutates,
                    source: "project_provider".to_owned(),
                    provider_command: Some(operation.provider_command.to_owned()),
                });
            }
        }
        for command in &self.commands {
            if seen.insert(command.key.as_str()) {
                operations.push(CapabilityOperation {
                    key: command.key.clone(),
                    label: if command.label.trim().is_empty() { command.key.clone() } else { command.label.clone() },
                    category: command.category.clone(),
                    risk: command.risk.clone(),
                    mutates: command.mutates || command.risk.trim() != "read_only",
                    source: "project_contract".to_owned(),
                    provider_command: None,
                });
            }
        }
        operations.sort_by(|left, right| left.key.cmp(&right.key));

        ProjectCapabilities {
            schema: CAPABILITIES_SCHEMA.to_owned(),
            schema_version: 1,
            project: CapabilityProject {
                id: self.project.id.clone(),
                name: self.project.name.clone(),
                kind: self.project.kind.clone(),
            },
            contract: CapabilityContract {
                schema: self.schema.clone(),
                schema_version: self.schema_version.unwrap_or(PROJECT_SCHEMA_VERSION),
            },
            provider: CapabilityProvider {
                configured: provider_configured,
                path: provider_path,
                ready: provider_ready,
                implementation_authority: non_empty_owned(&self.root_control_center.implementation_authority),
            },
            operations,
            gates: self.quality_gates.iter().map(|gate| CapabilityGate {
                key: gate.key.clone(),
                label: if gate.label.trim().is_empty() { gate.key.clone() } else { gate.label.clone() },
                stages: gate.stages.clone(),
            }).collect(),
            artifacts: self.artifacts.iter().map(|artifact| CapabilityArtifact {
                key: artifact.key.clone(),
                path: artifact.path.clone(),
            }).collect(),
        }
    }

    #[must_use]
    pub fn command(&self, key: &str) -> Option<&CommandDescriptor> {
        self.commands.iter().find(|command| command.key == key)
    }

    #[must_use]
    pub fn gate(&self, key: &str) -> Option<&QualityGate> {
        self.quality_gates.iter().find(|gate| gate.key == key)
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        if self.project.name.trim().is_empty() {
            "Unnamed Project"
        } else {
            &self.project.name
        }
    }
}

fn require_non_empty(field: &str, value: &str) -> Result<(), ContractError> {
    if value.trim().is_empty() {
        return Err(ContractError::Invalid(format!("{field} must not be empty")));
    }
    Ok(())
}

fn non_empty_owned(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn validate_relative_contract_path(field: &str, value: &str) -> Result<(), ContractError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ContractError::Invalid(format!("{field} must not be empty")));
    }
    if value.as_bytes().get(1) == Some(&b':') {
        return Err(ContractError::Invalid(format!("{field} must be project-relative: {value}")));
    }
    let path = Path::new(value);
    if path.is_absolute() || path.components().any(|component| matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_))) {
        return Err(ContractError::Invalid(format!("{field} must be a safe project-relative path: {value}")));
    }
    Ok(())
}

#[must_use]
pub fn substitute_root(value: &str, root: &Path) -> String {
    value.replace("{root}", &root.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_contract() -> ProjectContract {
        ProjectContract {
            schema: PROJECT_SCHEMA.to_owned(),
            schema_version: Some(PROJECT_SCHEMA_VERSION),
            project: ProjectIdentity { id: "demo".into(), name: "Demo".into(), kind: "rust-workspace".into(), ..Default::default() },
            root_control_center: RootControlCenter { machine_provider: "tools/control/ProjectControlCenter.py".into(), ..Default::default() },
            commands: vec![CommandDescriptor { key: "build".into(), label: "Build".into(), category: "build".into(), risk: "local_mutation".into(), program: "cargo".into(), args: vec!["build".into()], mutates: true }],
            quality_gates: vec![QualityGate { key: "full".into(), label: "Full".into(), stages: vec!["build".into()] }],
            artifacts: vec![ArtifactDescriptor { key: "debug".into(), path: "artifacts/debug".into() }],
            ..Default::default()
        }
    }

    #[test]
    fn root_control_prefers_machine_provider() {
        let control = RootControlCenter {
            machine_provider: "tools/control/ProjectControlCenter.py".into(),
            python_provider: "legacy/provider.py".into(),
            ..Default::default()
        };
        assert_eq!(control.provider_path(), Some("tools/control/ProjectControlCenter.py"));
    }

    #[test]
    fn root_substitution_is_deterministic() {
        let root = Path::new("C:/Demo");
        assert_eq!(substitute_root("--root={root}", root), "--root=C:/Demo");
    }

    #[test]
    fn contract_rejects_unknown_gate_stage() {
        let mut contract = valid_contract();
        contract.quality_gates[0].stages = vec!["missing".into()];
        assert!(matches!(contract.validate(), Err(ContractError::Invalid(_))));
    }

    #[test]
    fn contract_rejects_unsupported_schema_version() {
        let mut contract = valid_contract();
        contract.schema_version = Some(99);
        assert!(matches!(contract.validate(), Err(ContractError::SchemaVersion(99))));
    }

    #[test]
    fn capabilities_merge_provider_and_contract_operations() {
        let contract = valid_contract();
        let capabilities = contract.capabilities(Path::new("C:/not-a-real-project"));
        assert_eq!(capabilities.schema, CAPABILITIES_SCHEMA);
        assert!(capabilities.operations.iter().any(|item| item.key == "gate.full" && item.source == "project_provider"));
        assert!(capabilities.operations.iter().any(|item| item.key == "build" && item.source == "project_contract"));
        assert!(!capabilities.provider.ready);
    }

    #[test]
    fn canonical_provider_command_map_is_stable() {
        assert_eq!(provider_command("gate.full"), Some("full"));
        assert_eq!(provider_command("git.commit-push-green"), Some("commit-push-green"));
        assert_eq!(provider_command("not.real"), None);
    }
}
