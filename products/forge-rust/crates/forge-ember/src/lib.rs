use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const EMBER_HOST_SCHEMA: &str = "forge.ember_host.v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmberHostState {
    Unconfigured,
    Configured,
    Ready,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmberEditorSurface {
    pub id: String,
    pub label: String,
    pub executable: Option<PathBuf>,
    pub args: Vec<String>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmberProjectAdapter {
    pub schema: String,
    pub project_kinds: Vec<String>,
    pub ipc_endpoint: Option<String>,
    pub surfaces: Vec<EmberEditorSurface>,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmberHostStatus {
    pub schema: String,
    pub state: EmberHostState,
    pub project_root: PathBuf,
    pub manifest_path: Option<PathBuf>,
    pub adapter: Option<EmberProjectAdapter>,
    pub missing_required_surfaces: Vec<String>,
    pub detail: String,
}

pub fn inspect(project_root: &Path) -> Result<EmberHostStatus, String> {
    let manifest_path = project_root.join("ember.host.json");
    if !manifest_path.is_file() {
        return Ok(EmberHostStatus {
            schema: EMBER_HOST_SCHEMA.to_owned(),
            state: EmberHostState::Unconfigured,
            project_root: project_root.to_path_buf(),
            manifest_path: None,
            adapter: None,
            missing_required_surfaces: Vec::new(),
            detail: "project does not expose an Ember host manifest yet".to_owned(),
        });
    }
    let bytes = fs::read(&manifest_path).map_err(|error| error.to_string())?;
    let adapter: EmberProjectAdapter =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    validate_adapter(&adapter)?;
    let mut missing = Vec::new();
    for surface in &adapter.surfaces {
        if surface.required {
            let Some(executable) = &surface.executable else {
                missing.push(surface.id.clone());
                continue;
            };
            let path = project_root.join(executable);
            if !path.is_file() {
                missing.push(surface.id.clone());
            }
        }
    }
    let state = if missing.is_empty() {
        EmberHostState::Ready
    } else {
        EmberHostState::Degraded
    };
    Ok(EmberHostStatus {
        schema: EMBER_HOST_SCHEMA.to_owned(),
        state,
        project_root: project_root.to_path_buf(),
        manifest_path: Some(manifest_path),
        adapter: Some(adapter),
        missing_required_surfaces: missing.clone(),
        detail: if missing.is_empty() {
            "Ember project adapter is ready for Forge hosting".to_owned()
        } else {
            format!("missing required Ember surfaces: {}", missing.join(", "))
        },
    })
}

pub fn validate_adapter(adapter: &EmberProjectAdapter) -> Result<(), String> {
    if adapter.schema != EMBER_HOST_SCHEMA {
        return Err(format!("unsupported Ember host schema: {}", adapter.schema));
    }
    for surface in &adapter.surfaces {
        if surface.id.trim().is_empty() {
            return Err("Ember surface id cannot be empty".to_owned());
        }
        if let Some(executable) = &surface.executable {
            if executable.is_absolute()
                || executable
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
            {
                return Err(format!(
                    "Ember executable must be project-relative: {}",
                    executable.display()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_absolute_editor_executable() {
        let adapter = EmberProjectAdapter {
            schema: EMBER_HOST_SCHEMA.to_owned(),
            project_kinds: vec!["game".to_owned()],
            ipc_endpoint: None,
            surfaces: vec![EmberEditorSurface {
                id: "world".to_owned(),
                label: "World".to_owned(),
                executable: Some(PathBuf::from("/absolute/editor")),
                args: Vec::new(),
                required: true,
            }],
            capabilities: Vec::new(),
        };
        assert!(validate_adapter(&adapter).is_err());
    }
}
