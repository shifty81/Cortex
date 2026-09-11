use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub const TOOLCHAIN_REPORT_SCHEMA: &str = "forge.toolchain_report.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolRequirement {
    pub tool: String,
    pub required: bool,
    pub minimum_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolStatus {
    pub tool: String,
    pub required: bool,
    pub available: bool,
    pub executable: Option<PathBuf>,
    pub version_output: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolchainReport {
    pub schema: String,
    pub project_root: PathBuf,
    pub observed_unix_ms: u64,
    pub statuses: Vec<ToolStatus>,
    pub required_ready: bool,
}

pub fn requirements_from_project(root: &Path) -> Result<Vec<ToolRequirement>, String> {
    let path = root.join("project.control.json");
    let bytes = fs::read(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let mut requirements = Vec::new();
    for item in value
        .get("requirements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(tool) = item.get("tool").and_then(Value::as_str) else {
            continue;
        };
        requirements.push(ToolRequirement {
            tool: tool.to_owned(),
            required: item.get("required").and_then(Value::as_bool).unwrap_or(false),
            minimum_version: item
                .get("minimum_version")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
    }
    Ok(requirements)
}

pub fn doctor_project(root: &Path) -> Result<ToolchainReport, String> {
    let requirements = requirements_from_project(root)?;
    let statuses = requirements
        .iter()
        .map(probe_requirement)
        .collect::<Vec<_>>();
    let required_ready = statuses
        .iter()
        .filter(|status| status.required)
        .all(|status| status.available);
    Ok(ToolchainReport {
        schema: TOOLCHAIN_REPORT_SCHEMA.to_owned(),
        project_root: root.to_path_buf(),
        observed_unix_ms: unix_ms(),
        statuses,
        required_ready,
    })
}

pub fn probe_requirement(requirement: &ToolRequirement) -> ToolStatus {
    let executable = find_on_path(&requirement.tool);
    let version_output = executable.as_ref().and_then(|path| probe_version(path));
    let available = executable.is_some();
    let detail = if available {
        match (&requirement.minimum_version, &version_output) {
            (Some(minimum), Some(version)) => format!(
                "available; declared minimum {minimum}; reported `{}`",
                version.lines().next().unwrap_or(version)
            ),
            (_, Some(version)) => format!(
                "available; reported `{}`",
                version.lines().next().unwrap_or(version)
            ),
            _ => "available; version probe returned no text".to_owned(),
        }
    } else if requirement.required {
        "required tool not found on PATH".to_owned()
    } else {
        "optional tool not found on PATH".to_owned()
    };
    ToolStatus {
        tool: requirement.tool.clone(),
        required: requirement.required,
        available,
        executable,
        version_output,
        detail,
    }
}

pub fn find_on_path(program: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(program);
    if candidate.components().count() > 1 && candidate.is_file() {
        return Some(candidate);
    }
    let path = std::env::var_os("PATH")?;
    #[cfg(windows)]
    let extensions = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
        .split(';')
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for root in std::env::split_paths(&path) {
        let direct = root.join(program);
        if direct.is_file() {
            return Some(direct);
        }
        #[cfg(windows)]
        for extension in &extensions {
            let path = root.join(format!("{program}{extension}"));
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

fn probe_version(program: &Path) -> Option<String> {
    for args in [["--version"], ["-V"], ["version"]] {
        let output = Command::new(program).args(args).output().ok()?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            let text = if stdout.is_empty() { stderr } else { stdout };
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_required_tool_is_not_ready() {
        let status = probe_requirement(&ToolRequirement {
            tool: "forge-tool-that-should-not-exist-12345".to_owned(),
            required: true,
            minimum_version: None,
        });
        assert!(!status.available);
        assert!(status.detail.contains("required"));
    }
}
