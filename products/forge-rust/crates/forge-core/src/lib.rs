use forge_contracts::{provider_command, substitute_root, ProjectCapabilities, ProjectContract};
use forge_process::CommandSpec;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("operation is not exposed by the active project: {0}")]
    MissingOperation(String),
    #[error("project provider path is invalid: {0}")]
    MissingProvider(PathBuf),
}

#[derive(Debug, Clone)]
pub struct ProjectSession {
    pub root: PathBuf,
    pub contract: ProjectContract,
}

impl ProjectSession {
    pub fn load(root: impl AsRef<Path>) -> Result<Self, forge_contracts::ContractError> {
        let root = root.as_ref().to_path_buf();
        let contract = ProjectContract::load(&root)?;
        Ok(Self { root, contract })
    }

    #[must_use]
    pub fn provider_ready(&self) -> bool {
        self.contract
            .root_control_center
            .provider_path()
            .is_some_and(|provider| self.root.join(provider).is_file())
    }

    #[must_use]
    pub fn capabilities(&self) -> ProjectCapabilities {
        self.contract.capabilities(&self.root)
    }

    pub fn resolve(&self, operation: &str) -> Result<CommandSpec, ResolveError> {
        if let Some(provider) = self.contract.root_control_center.provider_path() {
            let provider_path = self.root.join(provider);
            if !provider_path.is_file() {
                return Err(ResolveError::MissingProvider(provider_path));
            }
            if let Some(provider_command) = provider_alias(operation) {
                return Ok(CommandSpec {
                    label: operation.to_owned(),
                    program: "python".to_owned(),
                    args: vec![
                        provider_path.to_string_lossy().into_owned(),
                        provider_command.to_owned(),
                        "--root".to_owned(),
                        self.root.to_string_lossy().into_owned(),
                    ],
                    cwd: self.root.clone(),
                });
            }
        }

        if let Some(command) = self.contract.command(operation) {
            let args = command
                .args
                .iter()
                .map(|arg| substitute_root(arg, &self.root))
                .collect();
            return Ok(CommandSpec {
                label: if command.label.is_empty() {
                    operation.to_owned()
                } else {
                    command.label.clone()
                },
                program: command.program.clone(),
                args,
                cwd: self.root.clone(),
            });
        }

        Err(ResolveError::MissingOperation(operation.to_owned()))
    }
}

#[must_use]
pub fn provider_alias(operation: &str) -> Option<&'static str> {
    provider_command(operation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_operations_map_to_project_provider_commands() {
        assert_eq!(provider_alias("gate.full"), Some("full"));
        assert_eq!(provider_alias("build.native"), Some("build"));
        assert_eq!(provider_alias("git.commit-push-green"), Some("commit-push-green"));
        assert_eq!(provider_alias("not.real"), None);
    }
}
