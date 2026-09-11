use forge_state::ForgeStateStore;
use forge_toolchain::{doctor_project, ToolchainReport};
use forge_vcs::{inspect as inspect_git, GitState};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DIAGNOSTIC_SCHEMA: &str = "forge.diagnostics.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeDiagnosticReport {
    pub schema: String,
    pub project_root: PathBuf,
    pub observed_unix_ms: u64,
    pub project_contract_present: bool,
    pub toolchain: Option<ToolchainReport>,
    pub toolchain_error: Option<String>,
    pub git: Option<GitState>,
    pub git_error: Option<String>,
    pub forge_state_ready: bool,
    pub forge_state_root: Option<PathBuf>,
    pub forge_state_error: Option<String>,
    pub operation_receipts: usize,
    pub overall_ready: bool,
}

pub fn collect(project_root: &Path) -> ForgeDiagnosticReport {
    let (toolchain, toolchain_error) = match doctor_project(project_root) {
        Ok(report) => (Some(report), None),
        Err(error) => (None, Some(error)),
    };
    let (git, git_error) = match inspect_git(project_root) {
        Ok(report) => (Some(report), None),
        Err(error) => (None, Some(error)),
    };
    let (forge_state_ready, forge_state_root, forge_state_error) =
        match ForgeStateStore::open_default(project_root) {
            Ok(store) => (true, Some(store.root().to_path_buf()), None),
            Err(error) => (false, None, Some(error)),
        };
    let operation_root = project_root
        .join("artifacts")
        .join("forge-rust")
        .join("operations");
    let operation_receipts = count_receipts(&operation_root);
    let project_contract_present = project_root.join("project.control.json").is_file();
    let overall_ready = project_contract_present
        && toolchain.as_ref().is_some_and(|report| report.required_ready)
        && git.is_some()
        && forge_state_ready;
    ForgeDiagnosticReport {
        schema: DIAGNOSTIC_SCHEMA.to_owned(),
        project_root: project_root.to_path_buf(),
        observed_unix_ms: unix_ms(),
        project_contract_present,
        toolchain,
        toolchain_error,
        git,
        git_error,
        forge_state_ready,
        forge_state_root,
        forge_state_error,
        operation_receipts,
        overall_ready,
    }
}

pub fn write_report(project_root: &Path, destination: &Path) -> Result<ForgeDiagnosticReport, String> {
    let report = collect(project_root);
    let parent = destination
        .parent()
        .ok_or_else(|| format!("diagnostic path has no parent: {}", destination.display()))?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    fs::write(destination, bytes).map_err(|error| error.to_string())?;
    Ok(report)
}

fn count_receipts(root: &Path) -> usize {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| entry.path().join("receipt.json").is_file())
        .count()
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
    fn report_is_truthful_when_project_contract_missing() {
        let root = std::env::temp_dir().join(format!("forge-diagnostics-{}", unix_ms()));
        fs::create_dir_all(&root).expect("root");
        let report = collect(&root);
        assert!(!report.project_contract_present);
        assert!(!report.overall_ready);
        let _ = fs::remove_dir_all(root);
    }
}
