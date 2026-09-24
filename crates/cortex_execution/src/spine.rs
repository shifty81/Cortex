//! Cortex execution-spine authority.
//!
//! This module deliberately owns only request lifecycle/identity, safe retained
//! operational state, provider leases, project-relative path authority,
//! transaction ownership, and controller-side dependency-recovery bookkeeping.
//! It contains no provider/model reasoning text and no project-specific logic.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const STATE_SCHEMA: u32 = 1;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_STALE_AFTER: Duration = Duration::from_secs(30);
const PROVIDER_LEASE_STALE_AFTER: Duration = Duration::from_secs(60 * 10);
const PRE_MUTATION_OBSERVATION_LIMIT: u32 = 8;
const SOURCE_OBSERVATION_LIMIT: u32 = 2;
// M11U2: once source has changed, compiler feedback becomes the next authority.
// A second source mutation is forbidden until a verification tool has completed.
const SOURCE_MUTATION_LIMIT: u32 = 1;
const REPEATED_SOURCE_SIGNATURE_LIMIT: u32 = 3;
const UNVERIFIED_MUTATION_MAX_AGE: Duration = Duration::from_secs(45);
static EXECUTION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionPhase {
    Queued,
    Preparing,
    Inspecting,
    Grounding,
    Planning,
    Mutating,
    Formatting,
    Building,
    Testing,
    Launching,
    RuntimeVerifying,
    Completed,
    Failed,
    Cancelled,
}

impl ExecutionPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Preparing => "preparing",
            Self::Inspecting => "inspecting",
            Self::Grounding => "grounding",
            Self::Planning => "planning",
            Self::Mutating => "mutating",
            Self::Formatting => "formatting",
            Self::Building => "building",
            Self::Testing => "testing",
            Self::Launching => "launching",
            Self::RuntimeVerifying => "runtime_verifying",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse_wire(value: &str) -> Self {
        match value {
            "queued" => Self::Queued,
            "preparing" => Self::Preparing,
            "inspecting" => Self::Inspecting,
            "grounding" => Self::Grounding,
            "planning" => Self::Planning,
            "mutating" => Self::Mutating,
            "formatting" => Self::Formatting,
            "building" => Self::Building,
            "testing" => Self::Testing,
            "launching" => Self::Launching,
            "runtime_verifying" => Self::RuntimeVerifying,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => Self::Preparing,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionSnapshot {
    pub schema_version: u32,
    pub execution_id: String,
    pub conversation_id: String,
    pub workspace_root: String,
    pub mode: String,
    pub phase: ExecutionPhase,
    pub current: String,
    pub next_expected: String,
    pub current_tool: String,
    pub current_target: String,
    pub model: String,
    pub model_role: String,
    pub agent_iteration: u32,
    pub agent_iteration_budget: u32,
    pub missing_dependencies: Vec<String>,
    pub grounded_dependencies: Vec<String>,
    pub transaction_id: String,
    pub provider_scope: String,
    pub provider_last_activity_unix_ms: u128,
    pub tool_last_activity_unix_ms: u128,
    pub started_unix_ms: u128,
    pub updated_unix_ms: u128,
    pub terminal_error: String,
    pub owner_pid: u32,
}

impl ExecutionSnapshot {
    fn new(
        execution_id: String,
        conversation_id: String,
        workspace_root: &Path,
        mode: &str,
        iteration_budget: u32,
    ) -> Self {
        let now = unix_ms();
        Self {
            schema_version: STATE_SCHEMA,
            execution_id,
            conversation_id,
            workspace_root: workspace_root.display().to_string(),
            mode: mode.to_string(),
            phase: ExecutionPhase::Preparing,
            current: "Preparing authoritative project execution".into(),
            next_expected: "inspect project state".into(),
            current_tool: String::new(),
            current_target: String::new(),
            model: String::new(),
            model_role: String::new(),
            agent_iteration: 0,
            agent_iteration_budget: iteration_budget,
            missing_dependencies: Vec::new(),
            grounded_dependencies: Vec::new(),
            transaction_id: String::new(),
            provider_scope: String::new(),
            provider_last_activity_unix_ms: now,
            tool_last_activity_unix_ms: now,
            started_unix_ms: now,
            updated_unix_ms: now,
            terminal_error: String::new(),
            owner_pid: process::id(),
        }
    }

    pub fn elapsed_ms(&self) -> u128 {
        unix_ms().saturating_sub(self.started_unix_ms)
    }

    pub fn provider_activity_age_ms(&self) -> u128 {
        unix_ms().saturating_sub(self.provider_last_activity_unix_ms)
    }

    pub fn tool_activity_age_ms(&self) -> u128 {
        unix_ms().saturating_sub(self.tool_last_activity_unix_ms)
    }

    pub fn live_line(&self) -> String {
        let elapsed = self.elapsed_ms() as f64 / 1000.0;
        let mut parts = vec![format!(
            "{} · {} · {:.1}s",
            self.mode,
            self.phase.as_str(),
            elapsed
        )];
        if !self.current.is_empty() {
            parts.push(self.current.clone());
        }
        if !self.current_tool.is_empty() {
            if self.current_target.is_empty() {
                parts.push(format!("tool: {}", self.current_tool));
            } else {
                parts.push(format!(
                    "tool: {} → {}",
                    self.current_tool, self.current_target
                ));
            }
        }
        if !self.missing_dependencies.is_empty() {
            parts.push(format!(
                "grounding: {}",
                self.missing_dependencies.join(", ")
            ));
        }
        if !self.next_expected.is_empty() && !self.phase.is_terminal() {
            parts.push(format!("next: {}", self.next_expected));
        }
        if !self.terminal_error.is_empty() {
            parts.push(format!("error: {}", compact(&self.terminal_error, 220)));
        }
        parts.join("  |  ")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionResumeDecision {
    Resume,
    RollbackAbandoned,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderLeaseInfo {
    pub execution_id: String,
    pub owner_pid: u32,
    pub acquired_unix_ms: u128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExecutionProgressState {
    execution_id: String,
    source_observations: u32,
    source_mutations: u32,
    first_unverified_mutation_unix_ms: u128,
    last_semantic_progress_unix_ms: u128,
    last_signature_hash: u64,
    repeated_signature: u32,
    verification_required: bool,
}

impl ExecutionProgressState {
    fn new(execution_id: &str) -> Self {
        Self {
            execution_id: execution_id.to_string(),
            source_observations: 0,
            source_mutations: 0,
            first_unverified_mutation_unix_ms: 0,
            last_semantic_progress_unix_ms: unix_ms(),
            last_signature_hash: 0,
            repeated_signature: 0,
            verification_required: false,
        }
    }
}

pub struct ProviderLeaseGuard {
    library_root: PathBuf,
    execution_id: String,
    released: bool,
}

impl ProviderLeaseGuard {
    pub fn acquire(library_root: impl AsRef<Path>, execution_id: &str) -> Result<Self, String> {
        let library_root = library_root.as_ref().to_path_buf();
        let path = provider_lease_path(&library_root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }

        // Recursive Repair/verification continuations reuse the same execution.
        // Only the outermost scope owns release of that execution's provider lease.
        let already_owned = read_provider_leases(&library_root)?
            .iter()
            .any(|lease| lease.execution_id == execution_id && lease.owner_pid == process::id());
        if already_owned {
            touch_provider_lease(&library_root, execution_id)?;
            return Ok(Self {
                library_root,
                execution_id: execution_id.to_string(),
                released: true,
            });
        }

        let info = ProviderLeaseInfo {
            execution_id: execution_id.to_string(),
            owner_pid: process::id(),
            acquired_unix_ms: unix_ms(),
        };
        add_provider_lease(&path, &info)?;
        Ok(Self {
            library_root,
            execution_id: execution_id.to_string(),
            released: false,
        })
    }

    pub fn release(&mut self) -> Result<(), String> {
        if self.released {
            return Ok(());
        }
        release_provider_lease(&self.library_root, &self.execution_id)?;
        self.released = true;
        Ok(())
    }
}

impl Drop for ProviderLeaseGuard {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

pub fn begin_execution(
    workspace_root: impl AsRef<Path>,
    conversation_id: &str,
    mode: &str,
    iteration_budget: u32,
) -> Result<String, String> {
    let root = workspace_root.as_ref();
    let path = state_path(root);
    with_lock(&path, || {
        if let Some(mut previous) = read_snapshot_path(&path)? {
            if !previous.phase.is_terminal()
                && previous.owner_pid == process::id()
                && previous.conversation_id == conversation_id
                && execution_modes_compatible(&previous.mode, mode)
            {
                let depth = execution_scope_depth_for_workspace(root, &previous.execution_id)?
                    .max(1)
                    .saturating_add(1);
                write_execution_scope_depth(root, &previous.execution_id, depth)?;
                previous.mode = mode.to_string();
                previous.current = "Continuing authoritative execution session".into();
                previous.next_expected = "continue project verification/repair".into();
                previous.updated_unix_ms = unix_ms();
                write_snapshot_path(&path, &previous)?;
                return Ok(previous.execution_id);
            }

            if !previous.phase.is_terminal() {
                previous.phase = ExecutionPhase::Failed;
                previous.current = "Previous execution abandoned by a newer request".into();
                previous.next_expected.clear();
                previous.terminal_error =
                    "Superseded by a new authoritative execution session".into();
                previous.updated_unix_ms = unix_ms();
                if !previous.provider_scope.is_empty() {
                    let _ = release_provider_lease(
                        Path::new(&previous.provider_scope),
                        &previous.execution_id,
                    );
                }
                let _ = mark_transaction_terminal(root, &previous.execution_id, "abandoned");
            }
            write_snapshot_path(&history_path(root, &previous.execution_id), &previous)?;
        }

        let sequence = EXECUTION_SEQUENCE.fetch_add(1, Ordering::Relaxed) as u128;
        let execution_stamp = unix_nanos()
            .saturating_mul(1_000_000)
            .saturating_add(sequence % 1_000_000);
        let execution_id = format!(
            "exec-{}-{}-{}",
            execution_stamp,
            process::id(),
            sanitize_id(conversation_id)
        );
        let snapshot = ExecutionSnapshot::new(
            execution_id.clone(),
            conversation_id.to_string(),
            root,
            mode,
            iteration_budget,
        );
        write_snapshot_path(&path, &snapshot)?;
        write_execution_scope_depth(root, &execution_id, 1)?;
        write_progress_state(root, &ExecutionProgressState::new(&execution_id))?;
        Ok(execution_id)
    })
}

pub fn active_snapshot_for_workspace(
    workspace_root: impl AsRef<Path>,
) -> Result<Option<ExecutionSnapshot>, String> {
    read_snapshot_path(&state_path(workspace_root.as_ref()))
}

pub fn active_live_line(workspace_root: impl AsRef<Path>) -> Option<String> {
    active_snapshot_for_workspace(workspace_root)
        .ok()
        .flatten()
        .map(|snapshot| snapshot.live_line())
}

pub fn active_snapshot_for_owner(owner_pid: u32) -> Result<Option<ExecutionSnapshot>, String> {
    let base = executions_root();
    if !base.is_dir() {
        return Ok(None);
    }
    let mut selected: Option<ExecutionSnapshot> = None;
    for entry in fs::read_dir(base).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let active = entry.path().join("active.state");
        let Some(snapshot) = read_snapshot_path(&active)? else {
            continue;
        };
        if snapshot.owner_pid != owner_pid || snapshot.phase.is_terminal() {
            continue;
        }
        let replace_selected = match selected.as_ref() {
            Some(current) => execution_is_newer(&snapshot, current),
            None => true,
        };
        if replace_selected {
            selected = Some(snapshot);
        }
    }
    Ok(selected)
}

pub fn active_live_line_for_owner(owner_pid: u32) -> Option<String> {
    active_snapshot_for_owner(owner_pid)
        .ok()
        .flatten()
        .map(|snapshot| snapshot.live_line())
}

pub fn latest_snapshot_for_owner(owner_pid: u32) -> Result<Option<ExecutionSnapshot>, String> {
    let base = executions_root();
    if !base.is_dir() {
        return Ok(None);
    }
    let mut selected: Option<ExecutionSnapshot> = None;
    for entry in fs::read_dir(base).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let active = entry.path().join("active.state");
        let Some(snapshot) = read_snapshot_path(&active)? else {
            continue;
        };
        if snapshot.owner_pid != owner_pid {
            continue;
        }
        let replace_selected = match selected.as_ref() {
            Some(current) => execution_is_newer(&snapshot, current),
            None => true,
        };
        if replace_selected {
            selected = Some(snapshot);
        }
    }
    Ok(selected)
}

pub fn latest_live_line_for_owner(owner_pid: u32) -> Option<String> {
    latest_snapshot_for_owner(owner_pid)
        .ok()
        .flatten()
        .map(|snapshot| snapshot.live_line())
}

pub fn touch_provider_activity_for_owner(owner_pid: u32, _phase_hint: &str) -> Result<(), String> {
    let Some(snapshot) = active_snapshot_for_owner(owner_pid)? else {
        return Ok(());
    };
    mutate_snapshot(
        Path::new(&snapshot.workspace_root),
        &snapshot.execution_id,
        |state| {
            state.provider_last_activity_unix_ms = unix_ms();
            state.updated_unix_ms = unix_ms();
        },
    )?;
    if !snapshot.provider_scope.is_empty() {
        let _ = touch_provider_lease(
            PathBuf::from(snapshot.provider_scope),
            &snapshot.execution_id,
        );
    }
    Ok(())
}

pub fn record_provider_activity_for_owner(
    owner_pid: u32,
    phase_hint: &str,
    message: &str,
) -> Result<(), String> {
    let Some(snapshot) = active_snapshot_for_owner(owner_pid)? else {
        return Ok(());
    };
    record_provider_activity_for_workspace(
        PathBuf::from(&snapshot.workspace_root),
        &snapshot.execution_id,
        phase_hint,
        message,
    )?;
    if !snapshot.provider_scope.is_empty() {
        let _ = touch_provider_lease(
            PathBuf::from(snapshot.provider_scope),
            &snapshot.execution_id,
        );
    }
    Ok(())
}

pub fn record_stream_metadata_for_owner(
    owner_pid: u32,
    model: Option<&str>,
    role: Option<&str>,
    iteration: Option<u32>,
) -> Result<(), String> {
    let Some(snapshot) = active_snapshot_for_owner(owner_pid)? else {
        return Ok(());
    };
    mutate_snapshot(
        Path::new(&snapshot.workspace_root),
        &snapshot.execution_id,
        |state| {
            if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
                state.model = compact(model, 160);
            }
            if let Some(role) = role.filter(|value| !value.trim().is_empty()) {
                state.model_role = compact(role, 80);
            }
            if let Some(iteration) = iteration {
                state.agent_iteration = iteration;
            }
            state.updated_unix_ms = unix_ms();
        },
    )
}

pub fn transition_for_workspace(
    workspace_root: impl AsRef<Path>,
    execution_id: &str,
    phase: ExecutionPhase,
    current: impl Into<String>,
    next_expected: impl Into<String>,
) -> Result<(), String> {
    mutate_snapshot(workspace_root.as_ref(), execution_id, |snapshot| {
        if snapshot.phase.is_terminal() {
            return;
        }
        snapshot.phase = phase;
        snapshot.current = current.into();
        snapshot.next_expected = next_expected.into();
        snapshot.updated_unix_ms = unix_ms();
    })
}

pub fn set_model_for_workspace(
    workspace_root: impl AsRef<Path>,
    execution_id: &str,
    model: &str,
    role: &str,
) -> Result<(), String> {
    mutate_snapshot(workspace_root.as_ref(), execution_id, |snapshot| {
        snapshot.model = model.to_string();
        snapshot.model_role = role.to_string();
        snapshot.provider_last_activity_unix_ms = unix_ms();
        snapshot.updated_unix_ms = unix_ms();
    })
}

pub fn set_iteration_for_workspace(
    workspace_root: impl AsRef<Path>,
    execution_id: &str,
    iteration: u32,
) -> Result<(), String> {
    mutate_snapshot(workspace_root.as_ref(), execution_id, |snapshot| {
        snapshot.agent_iteration = iteration;
        snapshot.updated_unix_ms = unix_ms();
    })
}

pub fn record_provider_activity_for_workspace(
    workspace_root: impl AsRef<Path>,
    execution_id: &str,
    _phase_hint: &str,
    message: &str,
) -> Result<(), String> {
    mutate_snapshot(workspace_root.as_ref(), execution_id, |snapshot| {
        snapshot.provider_last_activity_unix_ms = unix_ms();
        snapshot.updated_unix_ms = unix_ms();
        if !message.trim().is_empty() {
            snapshot.current = compact(message, 300);
        }
    })
}

pub fn record_tool_for_workspace(
    workspace_root: impl AsRef<Path>,
    tool: &str,
    target: &str,
    started: bool,
) -> Result<(), String> {
    let root = workspace_root.as_ref();
    let Some(snapshot) = active_snapshot_for_workspace(root)? else {
        return Ok(());
    };
    if snapshot.phase.is_terminal() {
        return Ok(());
    }
    let id = snapshot.execution_id.clone();
    mutate_snapshot(root, &id, |state| {
        state.current_tool = tool.to_string();
        state.current_target = target.to_string();
        state.tool_last_activity_unix_ms = unix_ms();
        state.updated_unix_ms = unix_ms();
        state.phase = phase_for_tool(tool, state.phase);
        state.current = if started {
            format!("Executing {tool}")
        } else {
            format!("Completed {tool}")
        };
        state.next_expected = next_for_tool(tool, started).to_string();
    })
}

pub fn authorize_tool_attempt_for_workspace(
    workspace_root: impl AsRef<Path>,
    tool: &str,
    target: &str,
) -> Result<(), String> {
    let root = workspace_root.as_ref();

    // M10A9: the actual execution boundary owns semantic-progress accounting.
    // This cannot be bypassed by an internal execute_inner_once caller.
    stage_guard_for_workspace(root, tool, target)?;

    let Some(snapshot) = active_snapshot_for_workspace(root)? else {
        return Ok(());
    };
    if snapshot.phase.is_terminal()
        || snapshot.phase == ExecutionPhase::Grounding
        || !matches!(snapshot.mode.as_str(), "apply" | "repair")
        || is_verification_tool(tool)
        || !is_source_cycle_tool(tool)
    {
        return Ok(());
    }

    let mut progress = read_progress_state(root, &snapshot.execution_id)?;

    // A tool attempt is not mutation evidence. Arm M11U2 only after the
    // source tool reports a successful write from record_tool_result_for_workspace.
    if is_observation_tool(tool) {
        // M11U2: observation is useful, but unbounded pre-edit browsing made the
        // local model burn minutes without reaching a compiler-informed action.
        progress.source_observations = progress.source_observations.saturating_add(1);
        update_progress_signature(&mut progress, tool, target);
    }

    write_progress_state(root, &progress)
}
pub fn record_tool_result_for_workspace(
    workspace_root: impl AsRef<Path>,
    tool: &str,
    target: &str,
    tool_succeeded: bool,
) -> Result<(), String> {
    let root = workspace_root.as_ref();
    record_tool_for_workspace(root, tool, target, false)?;
    let Some(snapshot) = active_snapshot_for_workspace(root)? else {
        return Ok(());
    };
    if snapshot.phase.is_terminal() {
        return Ok(());
    }

    if is_mutating_source_tool(tool) && tool_succeeded {
        let mut progress = read_progress_state(root, &snapshot.execution_id)?;
        let now = unix_ms();
        if progress.first_unverified_mutation_unix_ms == 0 {
            // A successful edit ends the exploration phase. The next source-cycle
            // action must be compiler/quality verification.
            progress.source_observations = 0;
            progress.source_mutations = 0;
            progress.repeated_signature = 0;
            progress.last_signature_hash = 0;
            progress.first_unverified_mutation_unix_ms = now;
        }
        progress.source_mutations = progress.source_mutations.saturating_add(1);
        progress.last_semantic_progress_unix_ms = now;
        update_progress_signature(&mut progress, tool, target);
        write_progress_state(root, &progress)?;
    }

    if is_verification_tool(tool) {
        let mut progress = read_progress_state(root, &snapshot.execution_id)?;
        let now = unix_ms();
        progress.source_observations = 0;
        progress.source_mutations = 0;
        progress.first_unverified_mutation_unix_ms = 0;
        progress.last_semantic_progress_unix_ms = now;
        progress.repeated_signature = 0;
        progress.last_signature_hash = 0;
        progress.verification_required = false;
        write_progress_state(root, &progress)?;
    }

    Ok(())
}

pub fn stage_guard_for_workspace(
    workspace_root: impl AsRef<Path>,
    tool: &str,
    target: &str,
) -> Result<(), String> {
    let root = workspace_root.as_ref();
    let Some(snapshot) = active_snapshot_for_workspace(root)? else {
        return Ok(());
    };
    if snapshot.phase.is_terminal()
        || snapshot.phase == ExecutionPhase::Grounding
        || !matches!(snapshot.mode.as_str(), "apply" | "repair")
        || is_verification_tool(tool)
        || !is_source_cycle_tool(tool)
    {
        return Ok(());
    }

    let mut progress = read_progress_state(root, &snapshot.execution_id)?;
    let now = unix_ms();
    let mutation_age = if progress.first_unverified_mutation_unix_ms == 0 {
        0
    } else {
        now.saturating_sub(progress.first_unverified_mutation_unix_ms)
    };
    let after_mutation = progress.first_unverified_mutation_unix_ms != 0;
    let exhausted_before_mutation = !after_mutation
        && snapshot.mode == "repair"
        && is_observation_tool(tool)
        && progress.source_observations >= PRE_MUTATION_OBSERVATION_LIMIT;
    if exhausted_before_mutation {
        return Err(format!(
            "M11U2 repair exploration budget exhausted before a source mutation (observations={}, target={}). Use the compiler/dependency evidence already collected to make one bounded repair mutation, or run project validation to refresh the evidence. Do not continue source/status browsing.",
            progress.source_observations,
            compact(target, 120)
        ));
    }

    let exhausted_after_mutation = after_mutation
        && (progress.source_observations >= SOURCE_OBSERVATION_LIMIT
            || progress.source_mutations >= SOURCE_MUTATION_LIMIT
            || progress.repeated_signature >= REPEATED_SOURCE_SIGNATURE_LIMIT
            || mutation_age >= UNVERIFIED_MUTATION_MAX_AGE.as_millis());

    if exhausted_after_mutation {
        progress.verification_required = true;
        write_progress_state(root, &progress)?;
    }

    if progress.verification_required {
        mutate_snapshot(root, &snapshot.execution_id, |state| {
            if !state.phase.is_terminal() {
                state.phase = ExecutionPhase::Planning;
                state.current =
                    "Source-only progress budget exhausted after project mutation".into();
                state.next_expected =
                    "run project format/validation before more source work".into();
                state.updated_unix_ms = unix_ms();
            }
        })?;
        return Err(format!(
            "M11U2 mutation-to-validation barrier: project source has already been mutated and compiler feedback is now mandatory (observations={}, mutations={}, repeated={} target={}). Run build.project_format/build.project_validate now; use only their concrete diagnostics for the next repair. Do not inspect or rewrite more source until verification has completed.",
            progress.source_observations,
            progress.source_mutations,
            progress.repeated_signature,
            compact(target, 120)
        ));
    }

    Ok(())
}

pub fn execution_scope_depth_for_workspace(
    workspace_root: impl AsRef<Path>,
    execution_id: &str,
) -> Result<u32, String> {
    read_execution_scope_depth(workspace_root.as_ref(), execution_id)
}

pub fn begin_grounding_for_workspace(
    workspace_root: impl AsRef<Path>,
    packages: &[String],
) -> Result<Option<String>, String> {
    let root = workspace_root.as_ref();
    let Some(snapshot) = active_snapshot_for_workspace(root)? else {
        return Ok(None);
    };
    let id = snapshot.execution_id.clone();
    mutate_snapshot(root, &id, |state| {
        state.phase = ExecutionPhase::Grounding;
        state.current = "Ground Before Write recovery is grounding dependency source".into();
        state.next_expected =
            "collect exact dependency evidence and request regenerated source".into();
        state.missing_dependencies = unique_packages(packages);
        state.updated_unix_ms = unix_ms();
    })?;
    Ok(Some(id))
}

pub fn complete_grounding_for_workspace(
    workspace_root: impl AsRef<Path>,
    execution_id: &str,
) -> Result<(), String> {
    mutate_snapshot(workspace_root.as_ref(), execution_id, |state| {
        state.phase = ExecutionPhase::Planning;
        state.current = "Dependency grounding complete; regeneration is required".into();
        state.next_expected = "regenerate the blocked mutation from grounded evidence".into();
        state.missing_dependencies.clear();
        state.updated_unix_ms = unix_ms();
    })
}

pub fn grounded_dependencies_for_workspace(
    workspace_root: impl AsRef<Path>,
) -> Result<Vec<String>, String> {
    Ok(active_snapshot_for_workspace(workspace_root.as_ref())?
        .map(|snapshot| snapshot.grounded_dependencies)
        .unwrap_or_default())
}

pub fn record_grounded_dependency_for_workspace(
    workspace_root: impl AsRef<Path>,
    package: &str,
) -> Result<(), String> {
    let root = workspace_root.as_ref();
    let Some(snapshot) = active_snapshot_for_workspace(root)? else {
        return Ok(());
    };
    let package = package.trim();
    if package.is_empty() {
        return Ok(());
    }
    mutate_snapshot(root, &snapshot.execution_id, |state| {
        let mut grounded = state.grounded_dependencies.clone();
        grounded.push(package.to_string());
        state.grounded_dependencies = unique_packages(&grounded);
        state.missing_dependencies.retain(|value| value != package);
        state.updated_unix_ms = unix_ms();
    })
}

pub fn revoke_grounded_dependencies_for_workspace(
    workspace_root: impl AsRef<Path>,
    packages: &[String],
) -> Result<(), String> {
    if packages.is_empty() {
        return Ok(());
    }
    let root = workspace_root.as_ref();
    let Some(snapshot) = active_snapshot_for_workspace(root)? else {
        return Ok(());
    };
    let revoked = packages.iter().cloned().collect::<BTreeSet<_>>();
    mutate_snapshot(root, &snapshot.execution_id, |state| {
        state
            .grounded_dependencies
            .retain(|value| !revoked.contains(value));
        state.updated_unix_ms = unix_ms();
    })
}

pub fn fail_execution_for_workspace(
    workspace_root: impl AsRef<Path>,
    execution_id: &str,
    error: &str,
) -> Result<(), String> {
    let root = workspace_root.as_ref();
    let remaining = leave_execution_scope(root, execution_id)?;
    if remaining > 0 {
        return mutate_snapshot(root, execution_id, |state| {
            if state.phase.is_terminal() {
                return;
            }
            state.phase = ExecutionPhase::Testing;
            state.current =
                "Repair continuation returned failure to owning verification loop".into();
            state.next_expected = "owning verification/repair decision".into();
            state.updated_unix_ms = unix_ms();
        });
    }

    mutate_snapshot(root, execution_id, |state| {
        if state.phase.is_terminal() {
            return;
        }
        state.phase = ExecutionPhase::Failed;
        state.current = "Execution failed".into();
        state.next_expected.clear();
        state.terminal_error = compact(error, 600);
        state.updated_unix_ms = unix_ms();
    })?;
    mark_transaction_terminal(root, execution_id, "failed")
}

pub fn complete_execution_for_workspace(
    workspace_root: impl AsRef<Path>,
    execution_id: &str,
) -> Result<(), String> {
    let root = workspace_root.as_ref();
    let remaining = leave_execution_scope(root, execution_id)?;
    if remaining > 0 {
        return mutate_snapshot(root, execution_id, |state| {
            if state.phase.is_terminal() {
                return;
            }
            state.phase = ExecutionPhase::Testing;
            state.current =
                "Repair continuation completed; returning to owning verification loop".into();
            state.next_expected = "quality-gate verification".into();
            state.updated_unix_ms = unix_ms();
        });
    }

    mutate_snapshot(root, execution_id, |state| {
        if state.phase.is_terminal() {
            return;
        }
        state.phase = ExecutionPhase::Completed;
        state.current = "Execution completed and verified".into();
        state.next_expected.clear();
        state.terminal_error.clear();
        state.updated_unix_ms = unix_ms();
    })?;
    mark_transaction_terminal(root, execution_id, "completed")
}

pub fn cancel_active_execution_for_workspace(
    workspace_root: impl AsRef<Path>,
    reason: &str,
) -> Result<Option<String>, String> {
    let root = workspace_root.as_ref();
    let Some(snapshot) = active_snapshot_for_workspace(root)? else {
        return Ok(None);
    };
    if snapshot.phase.is_terminal() {
        return Ok(None);
    }
    let id = snapshot.execution_id.clone();
    mutate_snapshot(root, &id, |state| {
        if state.phase.is_terminal() {
            return;
        }
        state.phase = ExecutionPhase::Cancelled;
        state.current = "Execution cancelled".into();
        state.next_expected.clear();
        state.terminal_error = compact(reason, 300);
        state.updated_unix_ms = unix_ms();
    })?;
    if !snapshot.provider_scope.is_empty() {
        let _ = release_provider_lease(Path::new(&snapshot.provider_scope), &id);
    }
    let _ = fs::remove_file(execution_scope_depth_path(root));
    mark_transaction_terminal(root, &id, "cancelled")?;
    Ok(Some(id))
}

pub fn cancel_active_executions_for_owner(owner_pid: u32, reason: &str) -> Result<usize, String> {
    let base = executions_root();
    if !base.is_dir() {
        let _ = release_provider_leases_for_owner(owner_pid);
        return Ok(0);
    }
    let mut cancelled = 0usize;
    for entry in fs::read_dir(&base).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let active = entry.path().join("active.state");
        let Some(snapshot) = read_snapshot_path(&active)? else {
            continue;
        };
        if snapshot.owner_pid != owner_pid || snapshot.phase.is_terminal() {
            continue;
        }
        let root = PathBuf::from(&snapshot.workspace_root);
        if cancel_active_execution_for_workspace(&root, reason)?.is_some() {
            cancelled += 1;
        }
    }
    let _ = release_provider_leases_for_owner(owner_pid);
    Ok(cancelled)
}

pub fn active_execution_id_for_workspace(
    workspace_root: impl AsRef<Path>,
) -> Result<Option<String>, String> {
    Ok(active_snapshot_for_workspace(workspace_root)?.map(|value| value.execution_id))
}

pub fn bind_provider_scope_for_workspace(
    workspace_root: impl AsRef<Path>,
    execution_id: &str,
    library_root: impl AsRef<Path>,
) -> Result<(), String> {
    let library = library_root.as_ref();
    mutate_snapshot(workspace_root.as_ref(), execution_id, |state| {
        state.provider_scope = library.display().to_string();
        state.updated_unix_ms = unix_ms();
    })
}

pub fn read_provider_leases(
    library_root: impl AsRef<Path>,
) -> Result<Vec<ProviderLeaseInfo>, String> {
    let path = provider_lease_path(library_root.as_ref());
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let leases = read_provider_leases_unlocked(&path)?;
    if leases.is_empty() {
        let _ = fs::remove_file(path);
    } else {
        write_provider_leases(&path, &leases)?;
    }
    Ok(leases)
}

pub fn read_provider_lease(
    library_root: impl AsRef<Path>,
) -> Result<Option<ProviderLeaseInfo>, String> {
    Ok(read_provider_leases(library_root)?.into_iter().next())
}

pub fn provider_lease_active(library_root: impl AsRef<Path>) -> bool {
    read_provider_leases(library_root)
        .map(|leases| !leases.is_empty())
        .unwrap_or(false)
}

pub fn touch_provider_lease(
    library_root: impl AsRef<Path>,
    execution_id: &str,
) -> Result<(), String> {
    let path = provider_lease_path(library_root.as_ref());
    with_lock(&path, || {
        let mut leases = read_provider_leases_unlocked(&path)?;
        let now = unix_ms();
        let mut found = false;
        for lease in &mut leases {
            if lease.execution_id == execution_id {
                lease.owner_pid = process::id();
                lease.acquired_unix_ms = now;
                found = true;
            }
        }
        if !found {
            leases.push(ProviderLeaseInfo {
                execution_id: execution_id.to_string(),
                owner_pid: process::id(),
                acquired_unix_ms: now,
            });
        }
        write_provider_leases(&path, &leases)
    })
}

pub fn release_provider_lease(
    library_root: impl AsRef<Path>,
    execution_id: &str,
) -> Result<(), String> {
    let path = provider_lease_path(library_root.as_ref());
    if !path.exists() {
        return Ok(());
    }
    with_lock(&path, || {
        let mut leases = read_provider_leases_unlocked(&path)?;
        leases.retain(|lease| lease.execution_id != execution_id);
        if leases.is_empty() {
            let _ = fs::remove_file(&path);
            Ok(())
        } else {
            write_provider_leases(&path, &leases)
        }
    })
}

pub fn release_provider_leases_for_owner(owner_pid: u32) -> Result<usize, String> {
    let base = provider_lease_root();
    if !base.is_dir() {
        return Ok(0);
    }
    let mut released = 0usize;
    for entry in fs::read_dir(&base).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("lease") {
            continue;
        }
        let before = read_provider_leases_unlocked(&path)?;
        let mut after = before.clone();
        after.retain(|lease| lease.owner_pid != owner_pid);
        released += before.len().saturating_sub(after.len());
        if after.is_empty() {
            let _ = fs::remove_file(&path);
        } else {
            write_provider_leases(&path, &after)?;
        }
    }
    Ok(released)
}

pub fn claim_transaction_for_workspace(
    workspace_root: impl AsRef<Path>,
    transaction_id: &str,
) -> Result<(), String> {
    let root = workspace_root.as_ref();
    let Some(snapshot) = active_snapshot_for_workspace(root)? else {
        return Ok(());
    };
    let path = transaction_owner_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = format!(
        "{}\t{}\t{}\tactive\n",
        snapshot.execution_id,
        transaction_id,
        unix_ms()
    );
    fs::write(&path, text).map_err(|error| error.to_string())?;
    mutate_snapshot(root, &snapshot.execution_id, |state| {
        state.transaction_id = transaction_id.to_string();
        state.updated_unix_ms = unix_ms();
    })
}

pub fn transaction_resume_decision(
    workspace_root: impl AsRef<Path>,
    execution_id: &str,
    transaction_id: &str,
) -> Result<TransactionResumeDecision, String> {
    let path = transaction_owner_path(workspace_root.as_ref());
    if !path.is_file() {
        return Ok(TransactionResumeDecision::RollbackAbandoned);
    }
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let fields = text.trim().split('\t').collect::<Vec<_>>();
    if fields.len() < 4 {
        return Ok(TransactionResumeDecision::RollbackAbandoned);
    }
    let owner = fields[0];
    let tx = fields[1];
    let status = fields[3];
    if owner == execution_id && tx == transaction_id && status == "active" {
        Ok(TransactionResumeDecision::Resume)
    } else {
        Ok(TransactionResumeDecision::RollbackAbandoned)
    }
}

pub fn mark_transaction_terminal(
    workspace_root: impl AsRef<Path>,
    execution_id: &str,
    status: &str,
) -> Result<(), String> {
    let path = transaction_owner_path(workspace_root.as_ref());
    if !path.is_file() {
        return Ok(());
    }
    let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let fields = text.trim().split('\t').collect::<Vec<_>>();
    if fields.len() < 2 || fields[0] != execution_id {
        return Ok(());
    }
    let tx = fields[1];
    fs::write(
        path,
        format!("{}\t{}\t{}\t{}\n", execution_id, tx, unix_ms(), status),
    )
    .map_err(|error| error.to_string())
}

pub fn parse_ground_before_write_packages(error: &str) -> Vec<String> {
    let marker = "Exact dependency-source evidence is missing for:";
    let Some(start) = error.find(marker) else {
        return Vec::new();
    };
    let tail = &error[start + marker.len()..];
    let end = tail.find('.').or(tail.find('\n')).unwrap_or(tail.len());
    unique_packages(
        &tail[..end]
            .split(',')
            .map(|value| value.trim().trim_matches('`').to_string())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>(),
    )
}

pub fn canonical_project_relative(
    workspace_root: impl AsRef<Path>,
    input: impl AsRef<str>,
) -> Result<String, String> {
    PathAuthority::new(workspace_root).resolve(input.as_ref())
}

#[derive(Clone, Debug)]
pub struct PathAuthority {
    root: PathBuf,
    root_key: String,
    root_name: String,
}

impl PathAuthority {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        let root = workspace_root.as_ref().to_path_buf();
        let root_key = windows_key(&root.display().to_string());
        let root_name = root
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        Self {
            root,
            root_key,
            root_name,
        }
    }

    pub fn resolve(&self, input: &str) -> Result<String, String> {
        let trimmed = input.trim().trim_matches('"');
        if trimmed.is_empty() || trimmed == "." {
            return Ok(".".to_string());
        }
        let cleaned = strip_verbatim_prefix(trimmed);
        let is_windows_absolute = looks_windows_absolute(&cleaned);
        let candidate_path = PathBuf::from(&cleaned);
        let mut relative = if candidate_path.is_absolute() || is_windows_absolute {
            let candidate_key = windows_key(&cleaned);
            if candidate_key == self.root_key {
                ".".to_string()
            } else {
                let prefix = format!("{}\\", self.root_key.trim_end_matches('\\'));
                if !candidate_key.starts_with(&prefix) {
                    return Err(format!(
                        "unsafe workspace path: {} is outside {}",
                        input,
                        self.root.display()
                    ));
                }
                candidate_key[prefix.len()..].to_string()
            }
        } else {
            cleaned.replace('/', "\\")
        };

        relative = relative.trim_start_matches(&['\\', '/'][..]).to_string();
        if !self.root_name.is_empty() {
            let project_prefix = format!("{}\\", self.root_name.to_ascii_lowercase());
            let lower = relative.to_ascii_lowercase();
            if lower.starts_with(&project_prefix) {
                relative = relative[project_prefix.len()..].to_string();
            }
        }

        if relative.is_empty() {
            return Ok(".".to_string());
        }

        let path = Path::new(&relative);
        let mut safe = Vec::<String>::new();
        for component in path.components() {
            match component {
                Component::Normal(value) => safe.push(value.to_string_lossy().to_string()),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(format!("unsafe workspace path: {input}"));
                }
            }
        }
        Ok(safe.join("/"))
    }
}

fn is_concrete_launch_tool(tool: &str) -> bool {
    matches!(
        tool,
        "process.launch"
            | "process.run"
            | "runtime.launch"
            | "runtime.run"
            | "project.launch"
            | "project.run"
            | "app.launch"
            | "app.run"
            | "build.project_launch"
            | "build.project_run"
            | "development.launch"
    ) || tool.starts_with("process.launch.")
        || tool.starts_with("runtime.launch.")
}
fn phase_for_tool(tool: &str, current: ExecutionPhase) -> ExecutionPhase {
    if tool.starts_with("source.read") || tool == "source.list" || tool == "source.search" {
        if current == ExecutionPhase::Grounding {
            return current;
        }
        return ExecutionPhase::Inspecting;
    }
    if tool.starts_with("source.write")
        || tool.starts_with("source.replace")
        || tool.starts_with("source.begin")
        || tool.starts_with("source.checkpoint")
    {
        return ExecutionPhase::Mutating;
    }
    if tool.contains("format") || tool.contains("fmt") {
        return ExecutionPhase::Formatting;
    }
    if tool.contains("test") || tool.contains("clippy") || tool.contains("validate") {
        return ExecutionPhase::Testing;
    }
    if tool.contains("build") || tool.contains("cargo_check") || tool.contains("cargo check") {
        return ExecutionPhase::Building;
    }
    if tool == "development.run_advance" {
        return ExecutionPhase::Planning;
    }
    if is_concrete_launch_tool(tool) {
        return ExecutionPhase::Launching;
    }
    current
}

fn next_for_tool(tool: &str, started: bool) -> &'static str {
    if started {
        return "tool result";
    }
    if tool.starts_with("source.") {
        return "next source/build action";
    }
    if tool.contains("build") || tool.contains("cargo") {
        return "verification or launch";
    }
    "next execution stage"
}

fn mutate_snapshot(
    workspace_root: &Path,
    execution_id: &str,
    mutator: impl FnOnce(&mut ExecutionSnapshot),
) -> Result<(), String> {
    let path = state_path(workspace_root);
    with_lock(&path, || {
        let Some(mut snapshot) = read_snapshot_path(&path)? else {
            return Ok(());
        };
        if snapshot.execution_id != execution_id {
            return Ok(());
        }
        mutator(&mut snapshot);
        write_snapshot_path(&path, &snapshot)
    })
}

fn unique_packages(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn execution_modes_compatible(previous: &str, next: &str) -> bool {
    previous == next
        || (matches!(previous, "apply" | "repair") && matches!(next, "apply" | "repair"))
}

fn execution_scope_depth_path(root: &Path) -> PathBuf {
    state_root(root).join("scope.depth")
}

fn read_execution_scope_depth(root: &Path, execution_id: &str) -> Result<u32, String> {
    let path = execution_scope_depth_path(root);
    if !path.is_file() {
        return Ok(0);
    }
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut fields = text.trim().split('\t');
    let owner = fields.next().unwrap_or_default();
    let depth = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or_default();
    if owner == execution_id {
        Ok(depth)
    } else {
        Ok(0)
    }
}

fn write_execution_scope_depth(root: &Path, execution_id: &str, depth: u32) -> Result<(), String> {
    let path = execution_scope_depth_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(
        path,
        format!("{}\t{}\n", sanitize_text(execution_id), depth),
    )
    .map_err(|error| error.to_string())
}

fn leave_execution_scope(root: &Path, execution_id: &str) -> Result<u32, String> {
    let depth = read_execution_scope_depth(root, execution_id)?;
    if depth > 1 {
        let remaining = depth - 1;
        write_execution_scope_depth(root, execution_id, remaining)?;
        Ok(remaining)
    } else {
        let _ = fs::remove_file(execution_scope_depth_path(root));
        Ok(0)
    }
}

fn progress_path(root: &Path) -> PathBuf {
    state_root(root).join("progress.state")
}

fn read_progress_state(root: &Path, execution_id: &str) -> Result<ExecutionProgressState, String> {
    let path = progress_path(root);
    if !path.is_file() {
        return Ok(ExecutionProgressState::new(execution_id));
    }
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let fields = text.trim().split('\t').collect::<Vec<_>>();
    if fields.len() != 8 || fields[0] != execution_id {
        return Ok(ExecutionProgressState::new(execution_id));
    }
    Ok(ExecutionProgressState {
        execution_id: fields[0].to_string(),
        source_observations: fields[1].parse().unwrap_or_default(),
        source_mutations: fields[2].parse().unwrap_or_default(),
        first_unverified_mutation_unix_ms: fields[3].parse().unwrap_or_default(),
        last_semantic_progress_unix_ms: fields[4].parse().unwrap_or_default(),
        last_signature_hash: fields[5].parse().unwrap_or_default(),
        repeated_signature: fields[6].parse().unwrap_or_default(),
        verification_required: fields[7] == "1",
    })
}

fn write_progress_state(root: &Path, progress: &ExecutionProgressState) -> Result<(), String> {
    let path = progress_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(
        path,
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            sanitize_text(&progress.execution_id),
            progress.source_observations,
            progress.source_mutations,
            progress.first_unverified_mutation_unix_ms,
            progress.last_semantic_progress_unix_ms,
            progress.last_signature_hash,
            progress.repeated_signature,
            if progress.verification_required { 1 } else { 0 }
        ),
    )
    .map_err(|error| error.to_string())
}

fn update_progress_signature(progress: &mut ExecutionProgressState, tool: &str, target: &str) {
    let signature = fnv1a(format!("{}\0{}", tool, target).as_bytes());
    if signature == progress.last_signature_hash {
        progress.repeated_signature = progress.repeated_signature.saturating_add(1);
    } else {
        progress.last_signature_hash = signature;
        progress.repeated_signature = 1;
    }
}

fn is_mutating_source_tool(tool: &str) -> bool {
    matches!(
        tool,
        "source.write_text" | "source.replace_text" | "source.apply_patch"
    )
}

fn is_observation_tool(tool: &str) -> bool {
    tool.starts_with("source.read")
        || tool == "source.list"
        || tool == "source.search"
        || matches!(
            tool,
            "workspace.status"
                | "project.status"
                | "project.profile"
                | "source.transaction_status"
                | "source.transaction_files"
        )
}

fn is_source_cycle_tool(tool: &str) -> bool {
    is_observation_tool(tool) || is_mutating_source_tool(tool)
}

fn is_verification_tool(tool: &str) -> bool {
    tool.starts_with("build.")
        || tool.contains("test")
        || tool.contains("clippy")
        || tool.contains("validate")
        || tool.contains("format")
}

fn state_path(root: &Path) -> PathBuf {
    state_root(root).join("active.state")
}

fn history_path(root: &Path, execution_id: &str) -> PathBuf {
    state_root(root)
        .join("history")
        .join(format!("{}.state", sanitize_id(execution_id)))
}

fn transaction_owner_path(root: &Path) -> PathBuf {
    state_root(root).join("transaction.owner")
}

fn state_root(root: &Path) -> PathBuf {
    executions_root().join(format!(
        "{:016x}",
        fnv1a(windows_key(&root.display().to_string()).as_bytes())
    ))
}

fn executions_root() -> PathBuf {
    cortex_runtime_root().join("executions")
}

fn provider_lease_root() -> PathBuf {
    cortex_runtime_root().join("provider-leases")
}

fn cortex_runtime_root() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Open2D").join("Cortex")
}

fn provider_lease_path(library_root: &Path) -> PathBuf {
    provider_lease_root().join(format!(
        "{:016x}.lease",
        fnv1a(windows_key(&library_root.display().to_string()).as_bytes())
    ))
}

fn add_provider_lease(path: &Path, info: &ProviderLeaseInfo) -> Result<(), String> {
    with_lock(path, || {
        let mut leases = read_provider_leases_unlocked(path)?;
        leases.retain(|lease| lease.execution_id != info.execution_id);
        leases.push(info.clone());
        write_provider_leases(path, &leases)
    })
}

#[cfg(windows)]
fn provider_lease_owner_alive(pid: u32) -> bool {
    use std::ffi::c_void;

    type Handle = *mut c_void;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    const ERROR_ACCESS_DENIED: u32 = 5;

    unsafe extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        fn GetLastError() -> u32;
    }

    if pid == 0 {
        return false;
    }

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // Fail closed for protected-but-live processes: access denied must
        // never make Cortex tear down another active execution's provider.
        return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
    }

    let mut exit_code = 0u32;
    let queried = unsafe { GetExitCodeProcess(handle, &mut exit_code) } != 0;
    unsafe {
        let _ = CloseHandle(handle);
    }
    queried && exit_code == STILL_ACTIVE
}

#[cfg(unix)]
fn provider_lease_owner_alive(pid: u32) -> bool {
    pid != 0 && Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(not(any(windows, unix)))]
fn provider_lease_owner_alive(pid: u32) -> bool {
    pid != 0
}
fn read_provider_leases_unlocked(path: &Path) -> Result<Vec<ProviderLeaseInfo>, String> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let now = unix_ms();
    Ok(text
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let execution_id = parts.next()?.to_string();
            let owner_pid = parts.next()?.parse::<u32>().ok()?;
            let acquired_unix_ms = parts.next()?.parse::<u128>().ok()?;
            if execution_id.is_empty()
                || owner_pid == 0
                || !provider_lease_owner_alive(owner_pid)
                || now.saturating_sub(acquired_unix_ms) > PROVIDER_LEASE_STALE_AFTER.as_millis()
            {
                return None;
            }
            Some(ProviderLeaseInfo {
                execution_id,
                owner_pid,
                acquired_unix_ms,
            })
        })
        .collect())
}

fn write_provider_leases(path: &Path, leases: &[ProviderLeaseInfo]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = leases
        .iter()
        .map(|info| {
            format!(
                "{}\t{}\t{}\n",
                sanitize_text(&info.execution_id),
                info.owner_pid,
                info.acquired_unix_ms
            )
        })
        .collect::<String>();
    fs::write(path, text).map_err(|error| error.to_string())
}

fn read_snapshot_path(path: &Path) -> Result<Option<ExecutionSnapshot>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let mut text = String::new();
    fs::File::open(path)
        .map_err(|error| error.to_string())?
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    Ok(Some(parse_snapshot(&text)))
}

fn write_snapshot_path(path: &Path, snapshot: &ExecutionSnapshot) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temp = path.with_extension(format!("tmp-{}", process::id()));
    let mut file = fs::File::create(&temp).map_err(|error| error.to_string())?;
    file.write_all(serialize_snapshot(snapshot).as_bytes())
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&temp, path).map_err(|error| error.to_string())
}

fn serialize_snapshot(snapshot: &ExecutionSnapshot) -> String {
    let deps = snapshot
        .missing_dependencies
        .iter()
        .map(|value| sanitize_text(value))
        .collect::<Vec<_>>()
        .join(",");
    [
        ("schema", snapshot.schema_version.to_string()),
        ("execution_id", sanitize_text(&snapshot.execution_id)),
        ("conversation_id", sanitize_text(&snapshot.conversation_id)),
        ("workspace_root", sanitize_text(&snapshot.workspace_root)),
        ("mode", sanitize_text(&snapshot.mode)),
        ("phase", snapshot.phase.as_str().to_string()),
        ("current", sanitize_text(&snapshot.current)),
        ("next_expected", sanitize_text(&snapshot.next_expected)),
        ("current_tool", sanitize_text(&snapshot.current_tool)),
        ("current_target", sanitize_text(&snapshot.current_target)),
        ("model", sanitize_text(&snapshot.model)),
        ("model_role", sanitize_text(&snapshot.model_role)),
        ("agent_iteration", snapshot.agent_iteration.to_string()),
        (
            "agent_iteration_budget",
            snapshot.agent_iteration_budget.to_string(),
        ),
        ("missing_dependencies", deps),
        (
            "grounded_dependencies",
            snapshot
                .grounded_dependencies
                .iter()
                .map(|value| sanitize_text(value))
                .collect::<Vec<_>>()
                .join(","),
        ),
        ("transaction_id", sanitize_text(&snapshot.transaction_id)),
        ("provider_scope", sanitize_text(&snapshot.provider_scope)),
        (
            "provider_last_activity_unix_ms",
            snapshot.provider_last_activity_unix_ms.to_string(),
        ),
        (
            "tool_last_activity_unix_ms",
            snapshot.tool_last_activity_unix_ms.to_string(),
        ),
        ("started_unix_ms", snapshot.started_unix_ms.to_string()),
        ("updated_unix_ms", snapshot.updated_unix_ms.to_string()),
        ("terminal_error", sanitize_text(&snapshot.terminal_error)),
        ("owner_pid", snapshot.owner_pid.to_string()),
    ]
    .into_iter()
    .map(|(key, value)| format!("{key}={value}\n"))
    .collect()
}

fn parse_snapshot(text: &str) -> ExecutionSnapshot {
    let value = |key: &str| -> String {
        text.lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .unwrap_or_default()
            .to_string()
    };
    ExecutionSnapshot {
        schema_version: value("schema").parse().unwrap_or(STATE_SCHEMA),
        execution_id: value("execution_id"),
        conversation_id: value("conversation_id"),
        workspace_root: value("workspace_root"),
        mode: value("mode"),
        phase: ExecutionPhase::parse_wire(&value("phase")),
        current: value("current"),
        next_expected: value("next_expected"),
        current_tool: value("current_tool"),
        current_target: value("current_target"),
        model: value("model"),
        model_role: value("model_role"),
        agent_iteration: value("agent_iteration").parse().unwrap_or_default(),
        agent_iteration_budget: value("agent_iteration_budget").parse().unwrap_or_default(),
        missing_dependencies: value("missing_dependencies")
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect(),
        grounded_dependencies: value("grounded_dependencies")
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect(),
        transaction_id: value("transaction_id"),
        provider_scope: value("provider_scope"),
        provider_last_activity_unix_ms: value("provider_last_activity_unix_ms")
            .parse()
            .unwrap_or_default(),
        tool_last_activity_unix_ms: value("tool_last_activity_unix_ms")
            .parse()
            .unwrap_or_default(),
        started_unix_ms: value("started_unix_ms").parse().unwrap_or_default(),
        updated_unix_ms: value("updated_unix_ms").parse().unwrap_or_default(),
        terminal_error: value("terminal_error"),
        owner_pid: value("owner_pid").parse().unwrap_or_default(),
    }
}

fn with_lock<T>(path: &Path, action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let lock = path.with_extension("lock");
    if let Some(parent) = lock.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let started = Instant::now();
    loop {
        match OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(file) => {
                let guard = LockGuard {
                    path: lock,
                    _file: file,
                };
                let result = action();
                drop(guard);
                return result;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if lock_is_stale(&lock) {
                    let _ = fs::remove_file(&lock);
                    continue;
                }
                if started.elapsed() >= LOCK_TIMEOUT {
                    return Err(format!(
                        "timed out waiting for Cortex execution state lock: {}",
                        lock.display()
                    ));
                }
                thread::sleep(Duration::from_millis(15));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn lock_is_stale(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    modified
        .elapsed()
        .map(|elapsed| elapsed >= LOCK_STALE_AFTER)
        .unwrap_or(false)
}

struct LockGuard {
    path: PathBuf,
    _file: fs::File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn strip_verbatim_prefix(value: &str) -> String {
    let normalized = value.replace('/', "\\");
    if let Some(rest) = normalized.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{}", rest);
    }
    normalized
        .strip_prefix(r"\\?\")
        .unwrap_or(&normalized)
        .to_string()
}

fn looks_windows_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    (bytes.len() >= 3
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
        && bytes[0].is_ascii_alphabetic())
        || value.starts_with(r"\\")
}

fn windows_key(value: &str) -> String {
    strip_verbatim_prefix(value)
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

fn sanitize_text(value: &str) -> String {
    value
        .replace(&['\r', '\n', '\t'][..], " ")
        .trim()
        .to_string()
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

fn compact(value: &str, max: usize) -> String {
    let value = sanitize_text(value);
    if value.chars().count() <= max {
        return value;
    }
    value
        .chars()
        .take(max.saturating_sub(1))
        .collect::<String>()
        + "…"
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn execution_stamp(execution_id: &str) -> u128 {
    execution_id
        .strip_prefix("exec-")
        .and_then(|value| value.split('-').next())
        .and_then(|value| value.parse::<u128>().ok())
        .unwrap_or_default()
}

fn execution_is_newer(candidate: &ExecutionSnapshot, current: &ExecutionSnapshot) -> bool {
    let candidate_stamp = execution_stamp(&candidate.execution_id);
    let current_stamp = execution_stamp(&current.execution_id);
    candidate_stamp > current_stamp
        || (candidate_stamp == current_stamp
            && (candidate.started_unix_ms > current.started_unix_ms
                || (candidate.started_unix_ms == current.started_unix_ms
                    && (candidate.updated_unix_ms > current.updated_unix_ms
                        || (candidate.updated_unix_ms == current.updated_unix_ms
                            && candidate.execution_id > current.execution_id)))))
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "cortex-m10-spine-{}-{}-{}",
            name,
            process::id(),
            unix_ms()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        root
    }

    #[test]
    fn execution_state_machine_retains_terminal_state() {
        let root = temp_root("state");
        let id = begin_execution(&root, "conversation-1", "repair", 8).unwrap();
        transition_for_workspace(
            &root,
            &id,
            ExecutionPhase::Inspecting,
            "Reading project source",
            "mutate source",
        )
        .unwrap();
        complete_execution_for_workspace(&root, &id).unwrap();
        let snapshot = active_snapshot_for_workspace(&root).unwrap().unwrap();
        assert_eq!(snapshot.phase, ExecutionPhase::Completed);
        assert!(snapshot.live_line().contains("completed"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ground_before_write_packages_are_batched_and_deduplicated() {
        let packages = parse_ground_before_write_packages(
            "Ground Before Write blocked this API-sensitive mutation. Exact dependency-source evidence is missing for: chrono, wgpu, winit, wgpu. Use source.search.",
        );
        assert_eq!(packages, vec!["chrono", "wgpu", "winit"]);
    }

    #[test]
    fn grounded_dependencies_persist_across_execution_snapshot_reloads() {
        let root = temp_root("grounded-ledger");
        let id = begin_execution(&root, "conversation-ground", "repair", 8).unwrap();
        begin_grounding_for_workspace(&root, &["wgpu".into()]).unwrap();
        record_grounded_dependency_for_workspace(&root, "wgpu").unwrap();
        assert_eq!(
            grounded_dependencies_for_workspace(&root).unwrap(),
            vec!["wgpu".to_string()]
        );
        complete_grounding_for_workspace(&root, &id).unwrap();
        let snapshot = active_snapshot_for_workspace(&root).unwrap().unwrap();
        assert_eq!(snapshot.phase, ExecutionPhase::Planning);
        assert_eq!(snapshot.grounded_dependencies, vec!["wgpu".to_string()]);
        assert!(snapshot.missing_dependencies.is_empty());

        revoke_grounded_dependencies_for_workspace(&root, &["wgpu".into()]).unwrap();
        assert!(grounded_dependencies_for_workspace(&root)
            .unwrap()
            .is_empty());
        cancel_active_execution_for_workspace(&root, "test complete").unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn project_relative_paths_are_canonical_and_duplicate_root_is_removed() {
        let authority = PathAuthority::new(PathBuf::from(r"C:\Users\Shifty\Desktop\hello3d"));
        assert_eq!(authority.resolve(".").unwrap(), ".");
        assert_eq!(authority.resolve("src/main.rs").unwrap(), "src/main.rs");
        assert_eq!(
            authority.resolve("hello3d/src/main.rs").unwrap(),
            "src/main.rs"
        );
        assert_eq!(
            authority
                .resolve(r"\\?\C:\Users\Shifty\Desktop\hello3d\src\main.rs")
                .unwrap(),
            "src/main.rs"
        );
        assert!(authority
            .resolve(r"C:\Windows\System32\kernel32.dll")
            .is_err());
    }

    #[test]
    fn stale_transaction_owner_does_not_cross_execution_identity() {
        let root = temp_root("tx");
        let first = begin_execution(&root, "conversation-1", "repair", 8).unwrap();
        claim_transaction_for_workspace(&root, "tx-1").unwrap();
        assert_eq!(
            transaction_resume_decision(&root, &first, "tx-1").unwrap(),
            TransactionResumeDecision::Resume
        );
        let second = begin_execution(&root, "conversation-2", "repair", 8).unwrap();
        assert_eq!(
            transaction_resume_decision(&root, &second, "tx-1").unwrap(),
            TransactionResumeDecision::RollbackAbandoned
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn latest_owner_snapshot_retains_terminal_failure_for_live_run() {
        let root = temp_root("terminal-owner");
        let id = begin_execution(&root, "conversation-terminal", "repair", 8).unwrap();
        let test_owner = u32::MAX - 101;
        mutate_snapshot(&root, &id, |snapshot| snapshot.owner_pid = test_owner).unwrap();

        fail_execution_for_workspace(&root, &id, "synthetic failure").unwrap();
        let latest = latest_snapshot_for_owner(test_owner).unwrap().unwrap();
        assert_eq!(latest.execution_id, id);
        assert_eq!(latest.phase, ExecutionPhase::Failed);
        assert!(latest.live_line().contains("synthetic failure"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn owner_snapshot_selection_follows_execution_creation_not_late_old_updates() {
        let first_root = temp_root("owner-order-first");
        let second_root = temp_root("owner-order-second");
        let first = begin_execution(&first_root, "conversation-first", "repair", 8).unwrap();
        let second = begin_execution(&second_root, "conversation-second", "repair", 8).unwrap();
        let test_owner = u32::MAX - 102;
        mutate_snapshot(&first_root, &first, |snapshot| {
            snapshot.owner_pid = test_owner
        })
        .unwrap();
        mutate_snapshot(&second_root, &second, |snapshot| {
            snapshot.owner_pid = test_owner
        })
        .unwrap();

        transition_for_workspace(
            &first_root,
            &first,
            ExecutionPhase::Inspecting,
            "Late update from older execution",
            "none",
        )
        .unwrap();

        let latest = latest_snapshot_for_owner(test_owner).unwrap().unwrap();
        assert_eq!(latest.execution_id, second);

        let _ = fs::remove_dir_all(first_root);
        let _ = fs::remove_dir_all(second_root);
    }

    #[test]
    fn recursive_repair_continuation_reuses_execution_and_scope_depth() {
        let root = temp_root("continuation");
        let first = begin_execution(&root, "conversation-1", "repair", 8).unwrap();
        let second = begin_execution(&root, "conversation-1", "repair", 8).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            execution_scope_depth_for_workspace(&root, &first).unwrap(),
            2
        );

        complete_execution_for_workspace(&root, &second).unwrap();
        let snapshot = active_snapshot_for_workspace(&root).unwrap().unwrap();
        assert!(!snapshot.phase.is_terminal());
        assert_eq!(
            execution_scope_depth_for_workspace(&root, &first).unwrap(),
            1
        );

        complete_execution_for_workspace(&root, &first).unwrap();
        let snapshot = active_snapshot_for_workspace(&root).unwrap().unwrap();
        assert_eq!(snapshot.phase, ExecutionPhase::Completed);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn source_cycle_requires_verification_immediately_after_mutation() {
        let root = temp_root("stage-guard");
        let id = begin_execution(&root, "conversation-1", "repair", 8).unwrap();

        authorize_tool_attempt_for_workspace(&root, "source.write_text", "src/main.rs").unwrap();
        record_tool_result_for_workspace(&root, "source.write_text", "src/main.rs", true).unwrap();
        let blocked =
            authorize_tool_attempt_for_workspace(&root, "source.read", "src/main.rs").unwrap_err();
        assert!(blocked.contains("mutation-to-validation barrier"));

        assert!(authorize_tool_attempt_for_workspace(&root, "build.project_validate", "").is_ok());
        record_tool_result_for_workspace(&root, "build.project_validate", "", false).unwrap();
        assert!(authorize_tool_attempt_for_workspace(&root, "source.read", "src/main.rs").is_ok());

        cancel_active_execution_for_workspace(&root, "test complete").unwrap();
        let _ = fs::remove_dir_all(root);
        let _ = id;
    }

    #[test]
    fn source_cycle_barrier_is_target_independent_after_mutation() {
        let root = temp_root("stage-guard-distinct-targets");
        let id = begin_execution(&root, "conversation-1", "repair", 8).unwrap();

        authorize_tool_attempt_for_workspace(&root, "source.write_text", "src/main.rs").unwrap();
        record_tool_result_for_workspace(&root, "source.write_text", "src/main.rs", true).unwrap();
        let blocked =
            authorize_tool_attempt_for_workspace(&root, "source.read", "src/different_target.rs")
                .unwrap_err();
        assert!(blocked.contains("mutation-to-validation barrier"));

        cancel_active_execution_for_workspace(&root, "test complete").unwrap();
        let _ = fs::remove_dir_all(root);
        let _ = id;
    }

    #[test]
    fn tool_attempt_authority_resets_pre_mutation_observations_at_edit_boundary() {
        let root = temp_root("pre-mutation-observations");
        let id = begin_execution(&root, "conversation-1", "repair", 8).unwrap();

        for _ in 0..SOURCE_OBSERVATION_LIMIT {
            authorize_tool_attempt_for_workspace(&root, "source.read", "Cargo.toml").unwrap();
        }
        authorize_tool_attempt_for_workspace(&root, "source.write_text", "src/main.rs").unwrap();
        record_tool_result_for_workspace(&root, "source.write_text", "src/main.rs", true).unwrap();
        let blocked =
            authorize_tool_attempt_for_workspace(&root, "source.read", "src/main.rs").unwrap_err();
        assert!(blocked.contains("mutation-to-validation barrier"));
        record_tool_result_for_workspace(&root, "build.project_validate", "", false).unwrap();
        assert!(authorize_tool_attempt_for_workspace(&root, "source.read", "src/main.rs").is_ok());

        cancel_active_execution_for_workspace(&root, "test complete").unwrap();
        let _ = fs::remove_dir_all(root);
        let _ = id;
    }

    #[test]
    fn failed_mutation_attempt_does_not_arm_validation_barrier() {
        let root = temp_root("m11u2-failed-mutation-is-not-mutation");
        let id = begin_execution(&root, "conversation-1", "repair", 8).unwrap();

        authorize_tool_attempt_for_workspace(&root, "source.write_text", ".cargo/config").unwrap();
        record_tool_result_for_workspace(&root, "source.write_text", ".cargo/config", false)
            .unwrap();

        assert!(
            authorize_tool_attempt_for_workspace(&root, "source.write_text", "src/main.rs").is_ok()
        );

        cancel_active_execution_for_workspace(&root, "test complete").unwrap();
        let _ = fs::remove_dir_all(root);
        let _ = id;
    }

    #[test]
    fn development_run_advance_does_not_claim_launching() {
        assert_eq!(
            phase_for_tool("development.run_advance", ExecutionPhase::Testing),
            ExecutionPhase::Planning
        );
        assert_eq!(
            phase_for_tool("process.launch", ExecutionPhase::Building),
            ExecutionPhase::Launching
        );
    }

    #[test]
    fn provider_hints_cannot_claim_launch_without_tool_evidence() {
        let root = temp_root("provider-phase");
        let id = begin_execution(&root, "conversation-1", "repair", 8).unwrap();
        record_tool_result_for_workspace(&root, "source.read", "src/main.rs", true).unwrap();
        record_provider_activity_for_workspace(&root, &id, "launching", "provider active").unwrap();
        let snapshot = active_snapshot_for_workspace(&root).unwrap().unwrap();
        assert_eq!(snapshot.phase, ExecutionPhase::Inspecting);
        cancel_active_execution_for_workspace(&root, "test complete").unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn nested_provider_lease_does_not_release_outer_execution_lease() {
        let root = temp_root("nested-lease");
        let outer = ProviderLeaseGuard::acquire(&root, "exec-same").unwrap();
        {
            let _inner = ProviderLeaseGuard::acquire(&root, "exec-same").unwrap();
            assert!(provider_lease_active(&root));
        }
        assert!(provider_lease_active(&root));
        drop(outer);
        assert!(!provider_lease_active(&root));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn dead_provider_lease_owner_is_pruned_immediately() {
        let root = temp_root("dead-provider-owner");
        let path = provider_lease_path(&root);
        add_provider_lease(
            &path,
            &ProviderLeaseInfo {
                execution_id: "dead-owner-execution".into(),
                owner_pid: u32::MAX,
                acquired_unix_ms: unix_ms(),
            },
        )
        .unwrap();

        assert!(read_provider_leases(&root).unwrap().is_empty());
        let _ = fs::remove_file(path);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn provider_lease_is_reference_counted_by_execution() {
        let root = temp_root("lease");
        let mut first = ProviderLeaseGuard::acquire(&root, "exec-1").unwrap();
        let mut second = ProviderLeaseGuard::acquire(&root, "exec-2").unwrap();
        assert_eq!(read_provider_leases(&root).unwrap().len(), 2);
        first.release().unwrap();
        assert!(provider_lease_active(&root));
        second.release().unwrap();
        assert!(!provider_lease_active(&root));
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn m11u2_second_mutation_is_blocked_until_validation() {
        let root = temp_root("m11u2-mutation-barrier");
        let id = begin_execution(&root, "conversation-1", "repair", 8).unwrap();

        authorize_tool_attempt_for_workspace(&root, "source.write_text", "src/main.rs").unwrap();
        record_tool_result_for_workspace(&root, "source.write_text", "src/main.rs", true).unwrap();
        let blocked =
            authorize_tool_attempt_for_workspace(&root, "source.replace_text", "src/main.rs")
                .unwrap_err();
        assert!(blocked.contains("mutation-to-validation barrier"));

        assert!(authorize_tool_attempt_for_workspace(&root, "build.project_validate", "").is_ok());
        record_tool_result_for_workspace(&root, "build.project_validate", "", false).unwrap();
        assert!(
            authorize_tool_attempt_for_workspace(&root, "source.replace_text", "src/main.rs")
                .is_ok()
        );

        cancel_active_execution_for_workspace(&root, "test complete").unwrap();
        let _ = fs::remove_dir_all(root);
        let _ = id;
    }

    #[test]
    fn m11u2_repair_pre_mutation_browsing_is_bounded() {
        let root = temp_root("m11u2-pre-mutation-budget");
        let id = begin_execution(&root, "conversation-1", "repair", 8).unwrap();

        for index in 0..PRE_MUTATION_OBSERVATION_LIMIT {
            authorize_tool_attempt_for_workspace(&root, "source.read", &format!("src/{index}.rs"))
                .unwrap();
        }
        let blocked = authorize_tool_attempt_for_workspace(&root, "source.read", "src/too_many.rs")
            .unwrap_err();
        assert!(blocked.contains("repair exploration budget exhausted"));
        assert!(
            authorize_tool_attempt_for_workspace(&root, "source.write_text", "src/main.rs").is_ok()
        );

        cancel_active_execution_for_workspace(&root, "test complete").unwrap();
        let _ = fs::remove_dir_all(root);
        let _ = id;
    }
}
