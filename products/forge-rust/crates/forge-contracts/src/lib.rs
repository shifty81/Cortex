use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContractError {
    #[error("project contract not found: {0}")]
    Missing(PathBuf),
    #[error("failed to read project contract {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("failed to parse project contract {path}: {source}")]
    Parse { path: PathBuf, source: serde_json::Error },
    #[error("unsupported project contract schema: {0}")]
    Schema(String),
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
        if !contract.schema.is_empty() && contract.schema != "forge.project.v1" {
            return Err(ContractError::Schema(contract.schema));
        }
        Ok(contract)
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

#[must_use]
pub fn substitute_root(value: &str, root: &Path) -> String {
    value.replace("{root}", &root.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
