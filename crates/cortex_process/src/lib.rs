//! Controlled build and process execution for Cortex.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn configure_background_command(command: &mut Command) {
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionIsolation {
    Trusted,
    Restricted,
    Sandboxed,
    PrivilegedBroker,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClass {
    CpuHeavy,
    GpuHeavy,
    DiskHeavy,
    Interactive,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResourceWorkload {
    ModelInference,
    ImageGeneration,
    Build,
    Indexing,
    InteractiveGame,
    Utility,
}

impl ResourceWorkload {
    pub fn classes(self) -> Vec<ResourceClass> {
        match self {
            Self::ModelInference | Self::ImageGeneration => vec![ResourceClass::GpuHeavy],
            Self::Build => vec![ResourceClass::CpuHeavy, ResourceClass::DiskHeavy],
            Self::Indexing => vec![ResourceClass::DiskHeavy],
            Self::InteractiveGame => vec![ResourceClass::Interactive, ResourceClass::GpuHeavy],
            Self::Utility => Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MachineResourceSnapshot {
    pub logical_cpu_count: usize,
    pub total_memory_bytes: Option<u64>,
    pub available_memory_bytes: Option<u64>,
    pub gpu_name: Option<String>,
    pub total_vram_bytes: Option<u64>,
    pub available_vram_bytes: Option<u64>,
    pub captured_unix_ms: u128,
}

impl MachineResourceSnapshot {
    pub fn local_baseline() -> Self {
        Self {
            logical_cpu_count: std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(1),
            total_memory_bytes: None,
            available_memory_bytes: None,
            gpu_name: None,
            total_vram_bytes: None,
            available_vram_bytes: None,
            captured_unix_ms: process_unix_ms(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceLease {
    pub id: String,
    pub workload: ResourceWorkload,
    pub owner: String,
    pub classes: Vec<ResourceClass>,
    pub acquired_unix_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourcePolicy {
    pub serialize_gpu_heavy_work: bool,
    pub serialize_disk_heavy_work: bool,
    pub pause_indexing_for_interactive_work: bool,
}

impl Default for ResourcePolicy {
    fn default() -> Self {
        Self {
            serialize_gpu_heavy_work: true,
            serialize_disk_heavy_work: false,
            pause_indexing_for_interactive_work: true,
        }
    }
}

#[derive(Default)]
pub struct ResourceLeaseManager {
    policy: ResourcePolicy,
    active: BTreeMap<String, ResourceLease>,
}

impl ResourceLeaseManager {
    pub fn with_policy(policy: ResourcePolicy) -> Self {
        Self {
            policy,
            active: BTreeMap::new(),
        }
    }

    pub fn acquire(
        &mut self,
        workload: ResourceWorkload,
        owner: impl Into<String>,
    ) -> Result<ResourceLease, String> {
        let owner = owner.into();
        let classes = workload.classes();
        for lease in self.active.values() {
            if self.policy.serialize_gpu_heavy_work
                && classes.contains(&ResourceClass::GpuHeavy)
                && lease.classes.contains(&ResourceClass::GpuHeavy)
            {
                return Err(format!(
                    "GPU-heavy Cortex work is already active: {} ({:?})",
                    lease.owner, lease.workload
                ));
            }
            if self.policy.serialize_disk_heavy_work
                && classes.contains(&ResourceClass::DiskHeavy)
                && lease.classes.contains(&ResourceClass::DiskHeavy)
            {
                return Err(format!(
                    "disk-heavy Cortex work is already active: {} ({:?})",
                    lease.owner, lease.workload
                ));
            }
            if self.policy.pause_indexing_for_interactive_work
                && matches!(workload, ResourceWorkload::InteractiveGame)
                && matches!(lease.workload, ResourceWorkload::Indexing)
            {
                return Err(format!(
                    "Cortex indexing should yield before interactive work starts: {}",
                    lease.owner
                ));
            }
        }

        let now = process_unix_ms();
        let id = format!("{now}-{}-lease", self.active.len());
        let lease = ResourceLease {
            id: id.clone(),
            workload,
            owner,
            classes,
            acquired_unix_ms: now,
        };
        self.active.insert(id, lease.clone());
        Ok(lease)
    }

    pub fn release(&mut self, id: &str) -> bool {
        self.active.remove(id).is_some()
    }

    pub fn active(&self) -> Vec<ResourceLease> {
        self.active.values().cloned().collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManagedProcessRecord {
    pub pid: u32,
    pub label: String,
    pub project_id: Option<String>,
    pub program: String,
    pub args: Vec<String>,
    pub isolation: ExecutionIsolation,
    pub started_unix_ms: u128,
    pub cortex_owned: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UiAutomationAction {
    InspectTree,
    Focus,
    Invoke,
    SetValue,
    Toggle,
    Select,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UiAutomationOperation {
    pub application: String,
    pub target_name: String,
    pub target_automation_id: Option<String>,
    pub action: UiAutomationAction,
    pub value: Option<String>,
}

impl UiAutomationOperation {
    pub fn is_read_only(&self) -> bool {
        matches!(self.action, UiAutomationAction::InspectTree)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandResult {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub duration_ms: u128,
    pub stdout: String,
    pub stderr: String,
    pub diagnostics: Vec<CompilerDiagnostic>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompilerDiagnostic {
    pub level: String,
    pub code: Option<String>,
    pub message: String,
    pub rendered: Option<String>,
    pub file: Option<PathBuf>,
    pub line_start: Option<u64>,
    pub column_start: Option<u64>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectOperation {
    Format,
    Validate,
    Test,
    Lint,
    Build,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectCommandProfile {
    pub kind: String,
    pub operation: ProjectOperation,
    pub program: String,
    pub args: Vec<String>,
}

pub fn detect_project_command(
    root: &Path,
    operation: ProjectOperation,
) -> Option<ProjectCommandProfile> {
    if root.join("Cargo.toml").is_file() {
        let args = match operation {
            ProjectOperation::Format => vec!["fmt".into(), "--".into(), "--check".into()],
            ProjectOperation::Validate => vec!["check".into(), "--message-format=json".into()],
            ProjectOperation::Test => vec!["test".into(), "--workspace".into()],
            ProjectOperation::Lint => vec![
                "clippy".into(),
                "--workspace".into(),
                "--all-targets".into(),
                "--".into(),
                "-D".into(),
                "warnings".into(),
            ],
            ProjectOperation::Build => vec!["build".into(), "--message-format=json".into()],
        };
        return Some(ProjectCommandProfile {
            kind: "cargo".into(),
            operation,
            program: "cargo".into(),
            args,
        });
    }
    if root.join("package.json").is_file() {
        let args = match operation {
            ProjectOperation::Format => vec!["run".into(), "format".into(), "--if-present".into()],
            ProjectOperation::Validate => vec!["run".into(), "check".into(), "--if-present".into()],
            ProjectOperation::Test => vec!["test".into(), "--if-present".into()],
            ProjectOperation::Lint => vec!["run".into(), "lint".into(), "--if-present".into()],
            ProjectOperation::Build => vec!["run".into(), "build".into(), "--if-present".into()],
        };
        return Some(ProjectCommandProfile {
            kind: "node".into(),
            operation,
            program: "npm".into(),
            args,
        });
    }
    if root.join("CMakeLists.txt").is_file() {
        let build_dir = ".cortex/build/cmake";
        let (program, args) = match operation {
            ProjectOperation::Format | ProjectOperation::Lint => return None,
            ProjectOperation::Validate => (
                "cmake",
                vec!["-S".into(), ".".into(), "-B".into(), build_dir.into()],
            ),
            ProjectOperation::Build => (
                "cmake",
                vec![
                    "--build".into(),
                    build_dir.into(),
                    "--config".into(),
                    "Debug".into(),
                ],
            ),
            ProjectOperation::Test => (
                "ctest",
                vec![
                    "--test-dir".into(),
                    build_dir.into(),
                    "-C".into(),
                    "Debug".into(),
                    "--output-on-failure".into(),
                ],
            ),
        };
        return Some(ProjectCommandProfile {
            kind: "cmake".into(),
            operation,
            program: program.into(),
            args,
        });
    }
    let gradle_wrapper = if cfg!(windows) && root.join("gradlew.bat").is_file() {
        Some(root.join("gradlew.bat"))
    } else if root.join("gradlew").is_file() {
        Some(root.join("gradlew"))
    } else {
        None
    };
    if root.join("build.gradle").is_file()
        || root.join("build.gradle.kts").is_file()
        || gradle_wrapper.is_some()
    {
        let task = match operation {
            ProjectOperation::Format => return None,
            ProjectOperation::Validate => "classes",
            ProjectOperation::Test => "test",
            ProjectOperation::Lint => "check",
            ProjectOperation::Build => "build",
        };
        return Some(ProjectCommandProfile {
            kind: "gradle".into(),
            operation,
            program: gradle_wrapper
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_else(|| "gradle".into()),
            args: vec![task.into()],
        });
    }
    if root.join("pyproject.toml").is_file() || root.join("requirements.txt").is_file() {
        let args = match operation {
            ProjectOperation::Validate => {
                vec!["-m".into(), "compileall".into(), "-q".into(), ".".into()]
            }
            ProjectOperation::Test => vec!["-m".into(), "unittest".into(), "discover".into()],
            ProjectOperation::Format | ProjectOperation::Lint | ProjectOperation::Build => {
                return None
            }
        };
        return Some(ProjectCommandProfile {
            kind: "python".into(),
            operation,
            program: "python".into(),
            args,
        });
    }
    let solution = fs_first_extension(root, "sln").or_else(|| fs_first_extension(root, "csproj"));
    if solution.is_some() {
        let mut args = vec![match operation {
            ProjectOperation::Format | ProjectOperation::Lint => return None,
            ProjectOperation::Test => "test".into(),
            ProjectOperation::Validate | ProjectOperation::Build => "build".into(),
        }];
        if let Some(solution) = solution {
            args.push(solution.to_string_lossy().to_string());
        }
        return Some(ProjectCommandProfile {
            kind: "dotnet".into(),
            operation,
            program: "dotnet".into(),
            args,
        });
    }
    None
}

fn cargo_package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for raw in manifest.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package {
            if let Some(value) = line.strip_prefix("name") {
                let value = value.trim_start();
                if let Some(value) = value.strip_prefix('=') {
                    let value = value.trim().trim_matches('"').trim_matches('\'');
                    if !value.is_empty() {
                        return Some(value.to_string());
                    }
                }
            }
        }
    }
    None
}

fn fs_first_extension(root: &Path, extension: &str) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        })
}

#[derive(Default)]
pub struct ProcessService {
    owned: BTreeMap<u32, OwnedProcess>,
}

struct OwnedProcess {
    label: String,
    child: Child,
    record: ManagedProcessRecord,
}

impl ProcessService {
    pub fn run_project_operation(
        &self,
        workspace: &Path,
        operation: ProjectOperation,
    ) -> Result<CommandResult, String> {
        let profile = detect_project_command(workspace, operation).ok_or_else(|| {
            format!(
                "Cortex does not yet have a safe {:?} command profile for {}",
                operation,
                workspace.display()
            )
        })?;
        let args = profile.args.iter().map(String::as_str).collect::<Vec<_>>();
        self.run_capture(
            workspace,
            &profile.program,
            &args,
            profile.program.eq_ignore_ascii_case("cargo")
                && matches!(
                    operation,
                    ProjectOperation::Validate | ProjectOperation::Build
                ),
        )
    }

    pub fn run_cargo(&self, workspace: &Path, args: &[&str]) -> Result<CommandResult, String> {
        self.run_capture(workspace, "cargo", args, true)
    }

    pub fn run_capture(
        &self,
        workspace: &Path,
        program: &str,
        args: &[&str],
        parse_cargo_json: bool,
    ) -> Result<CommandResult, String> {
        self.run_capture_with_policy(workspace, program, args, parse_cargo_json, true)
    }

    /// Execute the exact declared command without deterministic preflight mutation.
    /// Project quality gates use this path so a check remains a check; repair loops
    /// may continue using `run_capture`, which preserves formatter remediation.
    pub fn run_capture_exact(
        &self,
        workspace: &Path,
        program: &str,
        args: &[&str],
        parse_cargo_json: bool,
    ) -> Result<CommandResult, String> {
        self.run_capture_with_policy(workspace, program, args, parse_cargo_json, false)
    }

    fn run_capture_with_policy(
        &self,
        workspace: &Path,
        program: &str,
        args: &[&str],
        parse_cargo_json: bool,
        allow_preflight_remediation: bool,
    ) -> Result<CommandResult, String> {
        ensure_allowed_program(program)?;
        // H68B deterministic formatter runtime bridge. Governed project quality
        // gates call run_capture_exact and therefore never mutate source here.
        if allow_preflight_remediation {
            if let Some(remediation) =
                cortex_execution::deterministic_preflight_remediation(program, args)
            {
                let remediation_args = remediation
                    .args
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                let remediation_result = self.run_capture_with_policy(
                    workspace,
                    &remediation.program,
                    &remediation_args,
                    false,
                    false,
                )?;
                if !remediation_result.success {
                    return Ok(remediation_result);
                }
            }
        }

        let started = Instant::now();
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_background_command(&mut command);
        let output = command
            .output()
            .map_err(|error| format!("failed to run {program}: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let diagnostics = if parse_cargo_json {
            parse_cargo_diagnostics(&stdout)
                .into_iter()
                .chain(parse_cargo_diagnostics(&stderr))
                .collect()
        } else {
            Vec::new()
        };
        Ok(CommandResult {
            program: program.to_string(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            cwd: workspace.to_path_buf(),
            exit_code: output.status.code(),
            success: output.status.success(),
            duration_ms: started.elapsed().as_millis(),
            stdout,
            stderr,
            diagnostics,
        })
    }

    pub fn spawn_owned(
        &mut self,
        workspace: &Path,
        label: impl Into<String>,
        program: &str,
        args: &[String],
    ) -> Result<u32, String> {
        self.spawn_managed(
            workspace,
            label,
            None,
            program,
            args,
            ExecutionIsolation::Trusted,
        )
    }

    pub fn spawn_managed(
        &mut self,
        workspace: &Path,
        label: impl Into<String>,
        project_id: Option<String>,
        program: &str,
        args: &[String],
        isolation: ExecutionIsolation,
    ) -> Result<u32, String> {
        ensure_allowed_program(program)?;
        if matches!(isolation, ExecutionIsolation::PrivilegedBroker) {
            return Err(
                "privileged Cortex processes must be launched by the separate privileged broker"
                    .into(),
            );
        }
        let label = label.into();
        let child = Command::new(program)
            .args(args)
            .current_dir(workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("failed to launch {program}: {error}"))?;
        let pid = child.id();
        let record = ManagedProcessRecord {
            pid,
            label: label.clone(),
            project_id,
            program: program.to_string(),
            args: args.to_vec(),
            isolation,
            started_unix_ms: process_unix_ms(),
            cortex_owned: true,
        };
        self.owned.insert(
            pid,
            OwnedProcess {
                label,
                child,
                record,
            },
        );
        Ok(pid)
    }

    pub fn spawn_project_executable(
        &mut self,
        workspace: &Path,
        label: impl Into<String>,
        project_id: Option<String>,
        executable: &Path,
        args: &[String],
    ) -> Result<u32, String> {
        let workspace = std::fs::canonicalize(workspace)
            .map_err(|error| format!("failed to resolve project root: {error}"))?;
        let executable = std::fs::canonicalize(executable)
            .map_err(|error| format!("failed to resolve project executable: {error}"))?;
        if !executable.starts_with(&workspace) {
            return Err(format!(
                "Cortex refuses to launch an executable outside the active project: {}",
                executable.display()
            ));
        }
        if !executable.is_file() {
            return Err(format!(
                "project executable does not exist: {}",
                executable.display()
            ));
        }
        let label = label.into();
        let child = Command::new(&executable)
            .args(args)
            .current_dir(&workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("failed to launch project executable: {error}"))?;
        let pid = child.id();
        let record = ManagedProcessRecord {
            pid,
            label: label.clone(),
            project_id,
            program: executable.display().to_string(),
            args: args.to_vec(),
            isolation: ExecutionIsolation::Restricted,
            started_unix_ms: process_unix_ms(),
            cortex_owned: true,
        };
        self.owned.insert(
            pid,
            OwnedProcess {
                label,
                child,
                record,
            },
        );
        Ok(pid)
    }

    pub fn resolve_rust_debug_binary(&self, workspace: &Path) -> Result<PathBuf, String> {
        let manifest = std::fs::read_to_string(workspace.join("Cargo.toml"))
            .map_err(|error| format!("failed to read Cargo.toml: {error}"))?;
        let name = cargo_package_name(&manifest)
            .ok_or_else(|| "Cargo.toml does not contain a package name".to_string())?;
        let leaf = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name
        };
        Ok(workspace.join("target").join("debug").join(leaf))
    }

    pub fn managed_records(&self) -> Vec<ManagedProcessRecord> {
        self.owned
            .values()
            .map(|process| process.record.clone())
            .collect()
    }

    pub fn statuses(&mut self) -> Vec<OwnedProcessStatus> {
        let mut statuses = Vec::new();
        let ids: Vec<u32> = self.owned.keys().copied().collect();
        for pid in ids {
            if let Some(process) = self.owned.get_mut(&pid) {
                let state = match process.child.try_wait() {
                    Ok(Some(status)) => OwnedProcessState::Exited(status.code()),
                    Ok(None) => OwnedProcessState::Running,
                    Err(error) => OwnedProcessState::Unknown(error.to_string()),
                };
                statuses.push(OwnedProcessStatus {
                    pid,
                    label: process.label.clone(),
                    state,
                });
            }
        }
        statuses
    }

    pub fn stop(&mut self, pid: u32) -> Result<bool, String> {
        let Some(mut process) = self.owned.remove(&pid) else {
            return Ok(false);
        };
        if !process.record.cortex_owned {
            self.owned.insert(pid, process);
            return Err(format!(
                "Cortex refuses to stop pid {pid} because it is not recorded as Cortex-owned"
            ));
        }
        process
            .child
            .kill()
            .map_err(|error| format!("failed to stop pid {pid}: {error}"))?;
        let _ = process.child.wait();
        Ok(true)
    }

    pub fn stop_all(&mut self) {
        let ids: Vec<u32> = self.owned.keys().copied().collect();
        for pid in ids {
            let _ = self.stop(pid);
        }
    }
}

impl Drop for ProcessService {
    fn drop(&mut self) {
        self.stop_all();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OwnedProcessStatus {
    pub pid: u32,
    pub label: String,
    pub state: OwnedProcessState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnedProcessState {
    Running,
    Exited(Option<i32>),
    Unknown(String),
}

fn process_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn ensure_allowed_program(program: &str) -> Result<(), String> {
    let leaf = Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    if matches!(
        leaf.as_str(),
        "cargo"
            | "cargo.exe"
            | "rustfmt"
            | "rustfmt.exe"
            | "code"
            | "code.cmd"
            | "powershell"
            | "powershell.exe"
            | "pwsh"
            | "pwsh.exe"
            | "git"
            | "git.exe"
            | "npm"
            | "npm.cmd"
            | "pnpm"
            | "pnpm.cmd"
            | "yarn"
            | "yarn.cmd"
            | "cmake"
            | "cmake.exe"
            | "dotnet"
            | "dotnet.exe"
            | "msbuild"
            | "msbuild.exe"
            | "ctest"
            | "ctest.exe"
            | "gradle"
            | "gradle.exe"
            | "gradlew"
            | "gradlew.bat"
            | "python"
            | "python.exe"
            | "py"
            | "py.exe"
    ) {
        Ok(())
    } else {
        Err(format!(
            "program is not permitted by Cortex process policy: {program}"
        ))
    }
}

fn parse_cargo_diagnostics(text: &str) -> Vec<CompilerDiagnostic> {
    let mut result = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("reason").and_then(Value::as_str) != Some("compiler-message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let primary_span = message
            .get("spans")
            .and_then(Value::as_array)
            .and_then(|spans| {
                spans
                    .iter()
                    .find(|span| span.get("is_primary").and_then(Value::as_bool) == Some(true))
            });
        result.push(CompilerDiagnostic {
            level: message
                .get("level")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            code: message
                .pointer("/code/code")
                .and_then(Value::as_str)
                .map(str::to_string),
            message: message
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            rendered: message
                .get("rendered")
                .and_then(Value::as_str)
                .map(str::to_string),
            file: primary_span
                .and_then(|span| span.get("file_name"))
                .and_then(Value::as_str)
                .map(PathBuf::from),
            line_start: primary_span
                .and_then(|span| span.get("line_start"))
                .and_then(Value::as_u64),
            column_start: primary_span
                .and_then(|span| span.get("column_start"))
                .and_then(Value::as_u64),
        });
    }
    result
}

pub fn wait_for_file(path: &Path, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_cargo_project_operations() {
        let root =
            std::env::temp_dir().join(format!("cortex-process-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='demo'\nversion='0.1.0'\n",
        )
        .unwrap();
        let profile = detect_project_command(&root, ProjectOperation::Validate).unwrap();
        assert_eq!(profile.program, "cargo");
        assert_eq!(profile.args, vec!["check", "--message-format=json"]);
        let format = detect_project_command(&root, ProjectOperation::Format).unwrap();
        assert_eq!(format.args, vec!["fmt", "--", "--check"]);
        let lint = detect_project_command(&root, ProjectOperation::Lint).unwrap();
        assert!(lint.args.contains(&"clippy".to_string()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cmake_project_configures_before_build_and_uses_cortex_build_dir() {
        let root =
            std::env::temp_dir().join(format!("cortex-cmake-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.20)\nproject(demo LANGUAGES CXX)\n",
        )
        .unwrap();

        let validate = detect_project_command(&root, ProjectOperation::Validate).unwrap();
        assert_eq!(validate.program, "cmake");
        assert_eq!(validate.args, vec!["-S", ".", "-B", ".cortex/build/cmake"]);

        let build = detect_project_command(&root, ProjectOperation::Build).unwrap();
        assert_eq!(
            build.args,
            vec!["--build", ".cortex/build/cmake", "--config", "Debug"]
        );

        let test = detect_project_command(&root, ProjectOperation::Test).unwrap();
        assert_eq!(test.program, "ctest");
        assert!(test.args.contains(&".cortex/build/cmake".to_string()));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn arbitrary_program_is_denied() {
        assert!(ensure_allowed_program("format-c-drive.exe").is_err());
    }
}
