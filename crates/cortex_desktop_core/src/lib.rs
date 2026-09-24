//! Toolkit-neutral controller/state for the standalone Cortex desktop.

use cortex_adapter_git::{GitAdapter, ShadowGitStore};
use cortex_artifacts::{discover as discover_artifacts, ArtifactEntry};
use cortex_client::CortexClient;
use cortex_context::ContextStore;
use cortex_conversation::{Conversation, ConversationRole, ConversationStore};
use cortex_development::{discover_roadmap, proposed_roadmap, DevelopmentStore};
use cortex_execution::spine::{
    begin_execution, bind_provider_scope_for_workspace, cancel_active_executions_for_owner,
    claim_transaction_for_workspace, complete_execution_for_workspace,
    fail_execution_for_workspace, transition_for_workspace, ExecutionPhase, ProviderLeaseGuard,
};
use cortex_jobs::{ActivityKind, ActivityStore};
use cortex_jobs::{TaskStatus, TaskStore};
use cortex_model_host::{discover_models, ModelHostRegistry, ModelHostState};
use cortex_observability::{
    new_event_id, new_trace_id, EventSeverity, ProjectEvent, ProjectEventKind,
    ProjectObservability, TraceContext,
};
use cortex_permissions::PermissionPolicy;
use cortex_plugin::PluginRegistry;
use cortex_process::{ProcessService, ProjectOperation};
use cortex_protocol::{CortexStreamEvent, CortexStreamKind};
use cortex_provider_lmstudio::LmStudioProvider;
use cortex_registry::{
    CatalogFileClass, DriveCatalogReport, LibraryCandidate, LibraryScanReport, MigrationPlan,
    WorkspaceRegistry,
};
use cortex_review::ReviewSnapshot;
use cortex_service::{ServiceRegistry, ServiceState, ServiceStatus};
use cortex_settings::CortexSettings;
use cortex_universal::{
    choose_repair_strategy, plan_project, BuildSystemKind as UniversalBuildSystemKind,
    LanguageKind as UniversalLanguageKind, RepairSignal, RepairStrategy,
};
use cortex_vault::{LibraryItemKind, LibraryMemoryDatabase};
use cortex_workspace::{Workspace, WorkspaceProfile};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::Cell;
use std::fs;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const RUNTIME_CERTIFICATION_PROMPT: &str =
    "Cortex runtime certification probe. Reply exactly with CORTEX_OK and nothing else.";
const RUNTIME_CERTIFICATION_TITLE: &str = "Cortex runtime certification [internal]";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneKind {
    Conversations,
    Chat,
    Workspace,
    Context,
    Changes,
    Terminal,
    Build,
    Logs,
    Tasks,
    Artifacts,
    Vault,
    Settings,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesktopLayout {
    pub left: Vec<PaneKind>,
    pub center: Vec<PaneKind>,
    pub right: Vec<PaneKind>,
    pub bottom: Vec<PaneKind>,
}

impl Default for DesktopLayout {
    fn default() -> Self {
        Self {
            left: vec![PaneKind::Conversations, PaneKind::Workspace],
            center: vec![PaneKind::Chat, PaneKind::Workspace, PaneKind::Vault],
            right: vec![PaneKind::Context],
            bottom: vec![
                PaneKind::Terminal,
                PaneKind::Build,
                PaneKind::Logs,
                PaneKind::Tasks,
            ],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesktopBootstrap {
    pub schema_version: u32,
    pub workspace: WorkspaceProfile,
    pub state_root: PathBuf,
    pub service: Option<ServiceState>,
    pub conversations: Vec<Conversation>,
    pub layout: DesktopLayout,
    pub settings: CortexSettings,
    pub permissions: PermissionPolicy,
}

impl DesktopBootstrap {
    pub fn load(workspace_root: impl AsRef<Path>) -> Result<Self, String> {
        let bootstrap =
            Workspace::open(workspace_root.as_ref()).map_err(|error| error.to_string())?;
        let registry = WorkspaceRegistry::open_default()?;
        let _ = registry.ensure_library_root_for_workspace(workspace_root.as_ref())?;
        let _ = registry.ensure_cortex_self_registered()?;
        let record = registry.attach(&bootstrap)?;
        let workspace = registry.open_workspace(&record)?;
        let state_root = workspace.cortex_state_dir();
        let service = ServiceRegistry::new(&state_root, workspace.root())?.read()?;
        let conversations = ConversationStore::open(&state_root, workspace.root())?
            .list(false)?
            .into_iter()
            .filter(|conversation| !is_internal_runtime_certification_conversation(conversation))
            .collect();
        let settings = CortexSettings::load_or_default(&state_root)?;
        let permissions = PermissionPolicy::load_or_default(&state_root.join("permissions.json"))?;
        Ok(Self {
            schema_version: 2,
            workspace: workspace.profile().clone(),
            state_root,
            service,
            conversations,
            layout: DesktopLayout::default(),
            settings,
            permissions,
        })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DesktopChatBlock {
    #[serde(default)]
    pub message_id: String,
    pub role: String,
    pub label: String,
    pub text: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub created_unix_ms: u128,
    #[serde(default)]
    pub feedback_score: i8,
    #[serde(default = "default_chat_revision")]
    pub revision: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DesktopWorkbenchDocument {
    pub path: String,
    pub language: String,
    pub content: String,
    pub bytes: u64,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DesktopFileEntry {
    pub name: String,
    pub relative_path: String,
    pub is_directory: bool,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DesktopFileListing {
    pub scope: String,
    pub root: String,
    pub current: String,
    pub entries: Vec<DesktopFileEntry>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DesktopLibraryView {
    pub summary: String,
    pub projects: Vec<String>,
    pub lineage: Vec<String>,
    pub storage: Vec<String>,
    pub inbox: Vec<String>,
    pub plans: Vec<String>,
    pub recovery: Vec<String>,
    pub scan_ready: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DesktopView {
    pub workspace_title: String,
    pub workspaces: Vec<String>,
    pub active_workspace: Option<usize>,
    pub conversations: Vec<String>,
    pub active_conversation: Option<usize>,
    pub conversation_title: String,
    pub transcript: String,
    pub chat_blocks: Vec<DesktopChatBlock>,
    pub library: DesktopLibraryView,
    pub info: String,
    pub activity: String,
    pub jobs: String,
    pub build: String,
    pub git: String,
    pub vault_activity: String,
    pub system_activity: String,
    pub native_model_log_root: String,
    pub notifications: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesktopCertificationReport {
    pub schema_version: u32,
    pub workspace: bool,
    pub conversations: bool,
    pub settings: bool,
    pub permissions: bool,
    pub service_state_readable: bool,
    pub service_executable: bool,
    pub plugins: bool,
    pub artifacts: bool,
    pub ready: bool,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesktopChatCertificationReport {
    pub schema_version: u32,
    pub conversation_id: Option<String>,
    pub stream_events: usize,
    pub user_message_persisted: bool,
    pub assistant_message_persisted: bool,
    pub conversation_reopened: bool,
    pub provider_roundtrip: bool,
    pub structured_tool_roundtrip: bool,
    pub ready: bool,
    pub error: Option<String>,
}

pub struct DesktopController {
    workspace: Workspace,
    registry: WorkspaceRegistry,
    conversations: ConversationStore,
    service: ServiceRegistry,
    model_host: Option<ModelHostRegistry>,
    owns_runtime_lifetime: bool,
    lmstudio_started_by_cortex: bool,
    activity: ActivityStore,
    tasks: TaskStore,
    settings: CortexSettings,
    library_memory: Option<LibraryMemoryDatabase>,
    observability: ProjectObservability,
    project_id: String,
    active_conversation: Option<String>,
    pending_handoff: Option<PendingHandoff>,
    pending_project_creation: Option<PendingProjectCreation>,
    pending_provider_request: Option<PendingProviderRequest>,
    info: String,
    runtime_status: String,
}

impl DesktopController {
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self, String> {
        Self::open_internal(workspace_root.as_ref(), true)
    }

    fn open_internal(workspace_root: &Path, owns_runtime_lifetime: bool) -> Result<Self, String> {
        let bootstrap = Workspace::open(workspace_root).map_err(|error| error.to_string())?;
        let registry = WorkspaceRegistry::open_default()?;
        let _ = registry.ensure_library_root_for_workspace(workspace_root)?;
        let _ = registry.ensure_cortex_self_registered()?;
        let record = registry.attach(&bootstrap)?;
        let workspace = registry.open_workspace(&record)?;
        let state_root = workspace.cortex_state_dir();
        let conversations = ConversationStore::open(&state_root, workspace.root())?;
        archive_stale_runtime_certification_conversations(&conversations)?;
        let service = ServiceRegistry::new(&state_root, workspace.root())?;
        let activity = ActivityStore::open(&state_root)?;
        let tasks = TaskStore::open(&state_root)?;
        let settings = CortexSettings::load_or_default(&state_root)?;
        let library_root = registry.library_root()?;
        let model_host = library_root
            .as_ref()
            .map(|record| ModelHostRegistry::new(&record.root))
            .transpose()?;
        let library_memory = library_root
            .as_ref()
            .map(|record| LibraryMemoryDatabase::open(&record.root))
            .transpose()?;
        let observability = ProjectObservability::open(workspace.root(), &state_root)
            .map_err(|error| format!("failed to open Cortex observability store: {error}"))?;
        let project_id = record.id.clone();
        let active_conversation = conversations
            .list(false)?
            .first()
            .map(|conversation| conversation.id.clone());
        let mut controller = Self {
            workspace,
            registry,
            conversations,
            service,
            model_host,
            owns_runtime_lifetime,
            lmstudio_started_by_cortex: false,
            activity,
            tasks,
            settings,
            library_memory,
            observability,
            project_id,
            active_conversation,
            pending_handoff: None,
            pending_project_creation: None,
            pending_provider_request: None,
            info: String::new(),
            runtime_status: "not probed".into(),
        };
        if owns_runtime_lifetime {
            // Cortex runtime processes are Desktop-owned. Anything surviving before
            // a new Desktop owner exists is treated as orphaned prior-session state.
            let _ = controller.service.stop();
            if let Some(model_host) = &controller.model_host {
                let _ = model_host.stop();
            }
        }
        controller.sync_pending_state()?;
        controller.refresh_overview()?;
        Ok(controller)
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn settings(&self) -> &CortexSettings {
        &self.settings
    }

    pub fn update_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        let runtime_affecting = matches!(
            key,
            "provider"
                | "native_model_host_port"
                | "native_auto_bootstrap"
                | "native_models_max"
                | "lmstudio_auto_start"
                | "lmstudio_url"
                | "chat_model"
                | "tool_model"
                | "vision_model"
                | "embedding_model"
        );
        self.settings.set(key, value)?;
        self.settings.save(&self.workspace.cortex_state_dir())?;
        if runtime_affecting {
            let _ = self.service.stop();
            if let Some(model_host) = &self.model_host {
                let _ = model_host.stop();
            }
        }
        self.activity.append(
            ActivityKind::Workspace,
            "Cortex configuration",
            format!("{key} updated"),
            Some(true),
            json!({"key": key}),
        )?;
        self.refresh_overview()
    }

    pub fn active_conversation_id(&self) -> Option<String> {
        self.active_conversation.clone()
    }

    pub fn fork_for_background(&mut self) -> Result<Self, String> {
        self.sync_pending_state()?;

        // A request worker must not perform a second full Desktop bootstrap. In particular,
        // do not reattach the workspace, reopen the potentially large Vault memory database,
        // refresh overview presentation state, or mutate global registry active-project state
        // merely to obtain an isolated execution controller.
        let workspace = self.workspace.clone();
        let state_root = workspace.cortex_state_dir();
        let conversations = ConversationStore::open(&state_root, workspace.root())?;
        let service = ServiceRegistry::new(&state_root, workspace.root())?;
        let activity = ActivityStore::open(&state_root)?;
        let tasks = TaskStore::open(&state_root)?;

        let fork = Self {
            workspace,
            registry: self.registry.clone(),
            conversations,
            service,
            model_host: self.model_host.clone(),
            owns_runtime_lifetime: false,
            lmstudio_started_by_cortex: self.lmstudio_started_by_cortex,
            activity,
            tasks,
            settings: self.settings.clone(),
            // Vault memory is intentionally lazy for background execution. Operations that
            // actually require the full library catalog load it on demand.
            library_memory: None,
            observability: self.observability.clone(),
            project_id: self.project_id.clone(),
            active_conversation: self.active_conversation.clone(),
            pending_handoff: self.pending_handoff.clone(),
            pending_project_creation: self.pending_project_creation.clone(),
            pending_provider_request: self.pending_provider_request.clone(),
            info: String::new(),
            runtime_status: self.runtime_status.clone(),
        };
        fork.persist_background_state()?;
        Ok(fork)
    }

    pub fn persist_background_state(&self) -> Result<(), String> {
        let path = self.pending_state_path();
        let state = PendingDesktopState {
            handoff: self.pending_handoff.clone(),
            project_creation: self.pending_project_creation.clone(),
            provider_request: self.pending_provider_request.clone(),
        };
        if state.handoff.is_none()
            && state.project_creation.is_none()
            && state.provider_request.is_none()
        {
            if path.exists() {
                fs::remove_file(path).map_err(|error| error.to_string())?;
            }
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(
            path,
            serde_json::to_vec_pretty(&state).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())
    }

    pub fn sync_pending_state(&mut self) -> Result<(), String> {
        let path = self.pending_state_path();
        if !path.is_file() {
            return Ok(());
        }
        let had_provider_wait = self.pending_provider_request.is_some();
        let state: PendingDesktopState =
            serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        self.pending_handoff = state.handoff;
        self.pending_project_creation = state.project_creation;
        self.pending_provider_request = state.provider_request;
        if self.pending_provider_request.is_some() {
            self.runtime_status = "provider / waiting — request preserved".into();
        } else if had_provider_wait {
            self.runtime_status = "online / provider recovered".into();
        }
        Ok(())
    }

    fn pending_state_path(&self) -> PathBuf {
        self.workspace
            .cortex_state_dir()
            .join("desktop")
            .join("pending.json")
    }

    fn clear_pending_state(&mut self) -> Result<(), String> {
        self.pending_handoff = None;
        self.pending_project_creation = None;
        self.pending_provider_request = None;
        self.persist_background_state()
    }

    pub fn library_root(&self) -> Result<Option<PathBuf>, String> {
        Ok(self.registry.library_root()?.map(|record| record.root))
    }

    pub fn storage_authority_record(
        &self,
    ) -> Result<Option<cortex_registry::CortexLibraryRoot>, String> {
        self.registry.library_root()
    }

    pub fn offsite_backup_root(&self) -> Result<Option<PathBuf>, String> {
        self.registry.offsite_backup_root()
    }

    pub fn set_offsite_backup_root(
        &mut self,
        root: impl AsRef<Path>,
    ) -> Result<Option<PathBuf>, String> {
        let record = self.registry.set_offsite_backup_root(root)?;
        self.activity.append(
            ActivityKind::Workspace,
            "Cortex offsite backup",
            record
                .offsite_backup_root
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "disabled".into()),
            Some(true),
            serde_json::to_value(&record).map_err(|error| error.to_string())?,
        )?;
        self.refresh_overview()?;
        Ok(record.offsite_backup_root)
    }

    pub fn registry_clear_offsite_backup_root(&mut self) -> Result<Option<PathBuf>, String> {
        let record = self.registry.clear_offsite_backup_root()?;
        self.activity.append(
            ActivityKind::Workspace,
            "Cortex offsite backup",
            "disabled",
            Some(true),
            serde_json::to_value(&record).map_err(|error| error.to_string())?,
        )?;
        self.refresh_overview()?;
        Ok(record.offsite_backup_root)
    }

    pub fn set_library_root(&mut self, root: impl AsRef<Path>) -> Result<PathBuf, String> {
        let layout = self.registry.provision_library_root(root)?;
        self.library_memory = Some(LibraryMemoryDatabase::open(&layout.root)?);
        let record = self.registry.library_root()?;
        self.activity.append(
            ActivityKind::Workspace,
            "Cortex storage authority",
            layout.root.display().to_string(),
            Some(true),
            json!({
                "root": layout.root,
                "projects": layout.projects,
                "intake": layout.intake,
                "artifacts": layout.artifacts,
                "backups": layout.backups,
                "recovery": layout.recovery,
                "recycle_bin": layout.recycle_bin,
                "volume": record.as_ref().and_then(|record| record.volume.clone()),
                "offsite_backup_root": record.as_ref().and_then(|record| record.offsite_backup_root.clone())
            }),
        )?;
        self.refresh_overview()?;
        Ok(layout.root)
    }

    pub fn library_memory_status(&self) -> serde_json::Value {
        match &self.library_memory {
            Some(memory) => json!({
                "library_root": memory.library_root,
                "catalog_records": memory.catalog.len(),
                "memory_records": memory.memories.len(),
                "organization_pending": memory.pending_organization().len(),
                "scan_cursor": memory.scan_cursor,
                "updated_unix_ms": memory.updated_unix_ms,
            }),
            None => json!({"configured": false}),
        }
    }

    fn allocated_service_port(&self) -> Result<u16, String> {
        let record = self
            .registry
            .resolve_for_root(self.workspace.root())?
            .ok_or_else(|| "active workspace is not registered with Cortex".to_string())?;
        self.registry
            .service_port_for(&record.id, self.settings.service_port)
    }

    fn require_native_provider_ready(&self, client: &CortexClient) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let detail = match client.provider_status() {
                Ok(status) if status.get("ready").and_then(Value::as_bool) == Some(true) => {
                    return Ok(());
                }
                Ok(status) => provider_status_detail(&status),
                Err(error) => error,
            };
            if Instant::now() >= deadline {
                return Err(format!(
                    "Cortex Native Models service/provider handoff did not become ready within 30 seconds: {detail}"
                ));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    pub fn switch_workspace(&mut self, index: usize) -> Result<(), String> {
        let workspaces = self.registry.list()?;
        let selected = workspaces
            .get(index)
            .ok_or_else(|| format!("workspace index is out of range: {index}"))?
            .clone();

        self.registry.set_active(&selected.id)?;
        self.replace_workspace_context(&selected.root)?;
        Ok(())
    }

    pub fn attach_workspace(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        if !path.is_dir() {
            return Err(format!(
                "Cortex workspace attachment requires a directory: {}",
                path.display()
            ));
        }

        let workspace = Workspace::open(path).map_err(|error| error.to_string())?;
        self.registry.attach(&workspace)?;
        self.replace_workspace_context(workspace.root())?;
        Ok(())
    }

    /// Rebind the Desktop to another project without treating an in-process project
    /// switch as a new Desktop launch. The per-project Cortex service is stopped for
    /// the workspace we are leaving, but the Desktop-owned model host / LM Studio
    /// lifetime stays attached to this Desktop process and is reused by the next
    /// project on demand.
    fn replace_workspace_context(&mut self, workspace_root: &Path) -> Result<(), String> {
        let mut replacement = Self::open_internal(workspace_root, false)?;
        let owns_runtime_lifetime = self.owns_runtime_lifetime;
        let lmstudio_started_by_cortex = self.lmstudio_started_by_cortex;

        // One project-scoped service must never remain authoritative after project
        // selection changes. Stop only that service; do NOT tear down the global
        // model host while the same Desktop process is still alive.
        let _ = self.service.stop();

        // Assignment drops the old controller. Disarm its Drop implementation so a
        // normal project switch cannot kill the shared model host or LM Studio.
        self.owns_runtime_lifetime = false;
        self.lmstudio_started_by_cortex = false;
        replacement.owns_runtime_lifetime = owns_runtime_lifetime;
        replacement.lmstudio_started_by_cortex = lmstudio_started_by_cortex;
        replacement.runtime_status = "provider / ready on demand".into();
        replacement.activity.append(
            ActivityKind::Workspace,
            "Project context switched",
            format!(
                "authoritative project root: {}",
                replacement.workspace.root().display()
            ),
            Some(true),
            json!({"workspace_root": replacement.workspace.root()}),
        )?;
        *self = replacement;
        Ok(())
    }

    pub fn view(&mut self) -> DesktopView {
        let registered_workspaces = self.registry.list().unwrap_or_default();
        let active_workspace = registered_workspaces
            .iter()
            .position(|entry| entry.root == self.workspace.root());
        let workspaces = registered_workspaces
            .iter()
            .map(|entry| entry.name.clone())
            .collect();

        let conversations = self.conversations.list(false).unwrap_or_default();
        let active_conversation = self.active_conversation.as_ref().and_then(|active| {
            conversations
                .iter()
                .position(|conversation| &conversation.id == active)
        });
        let conversation_title = active_conversation
            .and_then(|index| conversations.get(index))
            .map(|conversation| conversation.title.clone())
            .unwrap_or_else(|| "No conversation".into());
        let active_conversation_data = self
            .active_conversation
            .as_deref()
            .and_then(|id| self.conversations.load(id).ok());
        let transcript = active_conversation_data
            .clone()
            .map(format_conversation)
            .unwrap_or_else(|| "Start a new conversation or select an existing chat.".into());
        let chat_blocks = active_conversation_data
            .map(conversation_blocks)
            .unwrap_or_default();
        let activity = self.activity_text();

        let jobs = match self.tasks.list(30) {
            Ok(tasks) if !tasks.is_empty() => {
                let mut output = String::from("JOBS\n\n");
                for task in tasks {
                    output.push_str(&format!(
                        "[{:?}] {} — {}{}\n",
                        task.status,
                        task.title,
                        if task.progress.message.is_empty() {
                            task.detail.as_str()
                        } else {
                            task.progress.message.as_str()
                        },
                        if task.cancellation_requested {
                            "  [CANCEL REQUESTED]"
                        } else {
                            ""
                        }
                    ));
                }
                output
            }
            Ok(_) => "JOBS\n\nNo active or recent jobs.".into(),
            Err(error) => format!("JOBS\n\nJob state unavailable: {error}"),
        };

        let operational_events = self.activity.recent(200).unwrap_or_default();
        let build = {
            let mut output = String::from("BUILD\n\n");
            let mut count = 0_usize;
            for event in &operational_events {
                let phase = event
                    .metadata
                    .get("phase")
                    .and_then(Value::as_str)
                    .or_else(|| event.metadata.get("stream_phase").and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let build_related = matches!(event.kind, ActivityKind::Build)
                    || (matches!(event.kind, ActivityKind::Error)
                        && matches!(
                            phase.as_str(),
                            "format"
                                | "formatting"
                                | "check"
                                | "validate"
                                | "validation"
                                | "test"
                                | "testing"
                                | "lint"
                                | "clippy"
                                | "build"
                                | "building"
                                | "launch"
                                | "run"
                                | "verify"
                                | "verification"
                        ));
                if build_related {
                    output.push_str(&format!(
                        "[{:?}] {} — {}\n",
                        event.kind, event.title, event.detail
                    ));
                    count = count.saturating_add(1);
                }
            }
            if count == 0 {
                output.push_str(
                    "No format/check/test/lint/build/launch/verify events are recorded for this project.",
                );
            }
            output
        };

        let mut git = match ReviewSnapshot::collect(self.workspace.root()) {
            Ok(review) => {
                let mut output = String::from("GIT / RECOVERY\n\n");
                output.push_str(&review.summary);
                output.push_str("\n\n");
                if review.git_active {
                    output.push_str("Repository: active\n\nStatus\n------\n");
                    output.push_str(
                        &serde_json::to_string_pretty(&review.git_status)
                            .unwrap_or_else(|_| "status unavailable".into()),
                    );
                    output.push_str("\n\nDiff\n----\n");
                    if review.diff.trim().is_empty() {
                        output.push_str("No uncommitted diff reported.");
                    } else {
                        output.push_str(&review.diff);
                    }
                } else {
                    output.push_str("Repository: Git not initialized for this project.\n");
                }
                if let Some(adapter) = GitAdapter::detect(self.workspace.root()) {
                    output.push_str("\n\nRecent commits\n--------------\n");
                    match adapter.log(24) {
                        Ok(commits) if commits.is_empty() => {
                            output.push_str("No commits are recorded yet.\n");
                        }
                        Ok(commits) => {
                            for commit in commits {
                                let short = commit.hash.chars().take(10).collect::<String>();
                                output.push_str(&format!(
                                    "{short}  {}  —  {}\n",
                                    commit.author, commit.subject
                                ));
                            }
                        }
                        Err(error) => {
                            output.push_str(&format!("History unavailable: {error}\n"));
                        }
                    }
                }
                output.push_str("\n\nCortex transaction\n------------------\n");
                output.push_str(
                    &serde_json::to_string_pretty(&review.transaction)
                        .unwrap_or_else(|_| "transaction state unavailable".into()),
                );
                output
            }
            Err(error) => format!(
                "GIT / RECOVERY\n\nGit review is unavailable for this project.\n\n{error}\n\nCortex transaction history remains authoritative for autonomous mutations."
            ),
        };

        if self.workspace.root().join(".git").exists() {
            if let Ok(output) = Command::new("git")
                .args([
                    "for-each-ref",
                    "--sort=-creatordate",
                    "--count=12",
                    "--format=%(refname:short)  %(objectname:short)  %(subject)",
                    "refs/cortex/checkpoints/",
                ])
                .current_dir(self.workspace.root())
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output()
            {
                if output.status.success() {
                    let safety = String::from_utf8_lossy(&output.stdout);
                    if !safety.trim().is_empty() {
                        git.push_str("\n\nCORTEX SAFETY REFS\n------------------\n");
                        git.push_str(safety.trim());
                        git.push('\n');
                    }
                }
            }
        }

        let vault_git_audit_cache = self
            .workspace
            .cortex_state_dir()
            .join("repositories")
            .join("vault-git-audit.json");
        if let Ok(bytes) = fs::read(&vault_git_audit_cache) {
            if let Ok(snapshot) = serde_json::from_slice::<Value>(&bytes) {
                git.push_str("\n\nVAULT GIT AUDIT\n---------------\n");
                git.push_str(
                    &serde_json::to_string_pretty(&snapshot)
                        .unwrap_or_else(|_| "Vault audit unavailable".into()),
                );
                git.push('\n');
            }
        }

        let provider_cache = self
            .workspace
            .cortex_state_dir()
            .join("repositories")
            .join("forgejo-provider-snapshot.json");
        if let Ok(bytes) = fs::read(&provider_cache) {
            if let Ok(snapshot) = serde_json::from_slice::<Value>(&bytes) {
                git.push_str("\n\nFORGEJO PROVIDER\n----------------\n");
                let owner = snapshot.get("owner").and_then(Value::as_str).unwrap_or("?");
                let repository = snapshot
                    .get("repository_name")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                let refreshed = snapshot
                    .get("refreshed_unix_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                git.push_str(&format!(
                    "Forgejo provider: cached/online\nRepository: {owner}/{repository}\nRefreshed: {refreshed}\n"
                ));
                for (heading, key) in [
                    ("FORGEJO ISSUES", "issues"),
                    ("FORGEJO PULLS", "pulls"),
                    ("FORGEJO ACTIONS", "actions"),
                    ("FORGEJO RELEASES", "releases"),
                ] {
                    git.push_str(&format!("\n{heading}\n{}\n", "-".repeat(heading.len())));
                    let section = snapshot.get(key).cloned().unwrap_or(Value::Null);
                    git.push_str(
                        &serde_json::to_string_pretty(&section)
                            .unwrap_or_else(|_| "unavailable".into()),
                    );
                    git.push('\n');
                }
            }
        }

        let vault_activity = {
            let mut output = String::from("VAULT\n\n");
            let mut count = 0_usize;
            for event in &operational_events {
                if matches!(event.kind, ActivityKind::Vault) {
                    output.push_str(&format!(
                        "[{:?}] {} — {}\n",
                        event.kind, event.title, event.detail
                    ));
                    count = count.saturating_add(1);
                }
            }
            if count == 0 {
                output.push_str("No recent Vault events are recorded for this project.");
            }
            output
        };

        let system_activity = {
            let mut output = String::from("SYSTEM / SERVICES\n\n");
            let mut count = 0_usize;
            for event in &operational_events {
                if matches!(
                    event.kind,
                    ActivityKind::Service | ActivityKind::Approval | ActivityKind::Error
                ) {
                    output.push_str(&format!(
                        "[{:?}] {} — {}\n",
                        event.kind, event.title, event.detail
                    ));
                    count = count.saturating_add(1);
                }
            }
            if count == 0 {
                output.push_str("No recent service/system events are recorded for this project.");
            }
            output
        };

        let native_model_log_root = self
            .registry
            .library_root()
            .ok()
            .flatten()
            .map(|record| {
                record
                    .root
                    .join(".cortex")
                    .join("model-host")
                    .display()
                    .to_string()
            })
            .unwrap_or_default();

        let notifications = {
            let mut output = String::from("NOTIFICATIONS\n\n");
            let mut count = 0_usize;
            for event in &operational_events {
                if matches!(event.kind, ActivityKind::Error | ActivityKind::Approval) {
                    output.push_str(&format!(
                        "[{:?}] {} — {}\n",
                        event.kind, event.title, event.detail
                    ));
                    count = count.saturating_add(1);
                }
            }
            if count == 0 {
                output.push_str("No notifications require attention.");
            }
            output
        };

        let status = self.status_text();
        let library = self.library_view().unwrap_or_default();
        DesktopView {
            workspace_title: self.workspace.profile().name.clone(),
            workspaces,
            active_workspace,
            conversations: conversations
                .iter()
                .map(|conversation| conversation.title.clone())
                .collect(),
            active_conversation,
            conversation_title,
            transcript,
            chat_blocks,
            library,
            info: self.info.clone(),
            activity,
            jobs,
            build,
            git,
            vault_activity,
            system_activity,
            native_model_log_root,
            notifications,
            status,
        }
    }

    pub fn library_view(&self) -> Result<DesktopLibraryView, String> {
        let library_record = self.registry.library_root()?;
        let library_root = library_record
            .as_ref()
            .map(|record| record.root.display().to_string())
            .unwrap_or_else(|| "Not configured".into());
        let volume_summary = library_record
            .as_ref()
            .and_then(|record| record.volume.as_ref())
            .map(|volume| {
                format!(
                    "{}{}{}",
                    volume
                        .mount_root
                        .as_ref()
                        .map(|root| root.display().to_string())
                        .unwrap_or_else(|| "unknown mount".into()),
                    volume
                        .filesystem
                        .as_ref()
                        .map(|filesystem| format!(" · {filesystem}"))
                        .unwrap_or_default(),
                    volume
                        .free_bytes
                        .map(|free| format!(" · {} free", human_bytes(free)))
                        .unwrap_or_default()
                )
            })
            .unwrap_or_else(|| "volume identity not captured yet".into());
        let offsite_summary = library_record
            .as_ref()
            .and_then(|record| record.offsite_backup_root.as_ref())
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "Not configured".into());
        let plans = self.registry.list_migration_plans()?;
        let Some(report) = self.registry.last_drive_catalog()? else {
            return Ok(DesktopLibraryView {
                summary: format!(
                    "Cortex Storage Authority: {library_root}\nVolume: {volume_summary}\nOffsite backup: {offsite_summary}\n\nNo storage catalog has been captured yet. Storage/whole-PC scans are read-only: they catalog and classify files/projects but do not move, delete, deduplicate, register, or initialize repository history."
                ),
                projects: vec!["No project graph yet — run Catalog Preview.".into()],
                lineage: vec!["No lineage analysis yet.".into()],
                storage: vec!["No storage classification yet.".into()],
                inbox: vec!["No unclassified-file summary yet.".into()],
                plans: format_migration_plan_rows(&plans),
                recovery: vec!["No storage migration has been executed. Recovery remains untouched.".into()],
                scan_ready: false,
                truncated: false,
            });
        };
        let mut projects = report.project_nodes.clone();
        projects.sort_by(|left, right| left.root.cmp(&right.root));
        let project_rows = projects
            .iter()
            .map(|node| {
                let depth = project_depth(node, &report);
                let status = if node.already_registered {
                    "registered"
                } else {
                    "cataloged"
                };
                format!(
                    "{}{}  [{:?}]  {}%  {}\n{}",
                    "  ".repeat(depth.min(8)),
                    node.name,
                    node.kind,
                    node.confidence,
                    status,
                    node.root.display()
                )
            })
            .collect::<Vec<_>>();
        let lineage = if report.lineage_candidates.is_empty() {
            vec!["No related-copy/project-lineage candidates detected in this catalog.".into()]
        } else {
            report
                .lineage_candidates
                .iter()
                .map(|candidate| {
                    format!(
                        "{}  — {} related project trees — confidence {}%\n{}",
                        candidate.normalized_name,
                        candidate.project_ids.len(),
                        candidate.confidence,
                        candidate.reason
                    )
                })
                .collect()
        };
        let storage = report
            .class_summary
            .iter()
            .map(|summary| {
                format!(
                    "{:?}: {} files / {}",
                    summary.class,
                    summary.files,
                    human_bytes(summary.bytes)
                )
            })
            .collect::<Vec<_>>();
        let unknown = report
            .class_summary
            .iter()
            .filter(|summary| {
                matches!(
                    summary.class,
                    CatalogFileClass::Unknown
                        | CatalogFileClass::PersonalNonProject
                        | CatalogFileClass::Media
                )
            })
            .map(|summary| {
                format!(
                    "{:?}: {} files / {}",
                    summary.class,
                    summary.files,
                    human_bytes(summary.bytes)
                )
            })
            .collect::<Vec<_>>();
        let duplicate_bytes = report
            .duplicate_groups
            .iter()
            .map(|group| {
                group
                    .bytes_each
                    .saturating_mul(group.paths.len().saturating_sub(1) as u64)
            })
            .sum::<u64>();
        let summary = format!(
            "Cortex Storage Authority: {library_root}\nVolume: {volume_summary}\nOffsite backup: {offsite_summary}\nCatalog roots: {}\nDirectories: {}    Ignored/generated: {}    Files: {}    Cataloged: {}\nProject nodes: {}    Nested edges: {}    Lineage candidates: {}\nDuplicate groups (fast fingerprint): {}    Potential duplicate bytes: {}\nExternal/cycle links skipped: {}{}\n\nREAD-ONLY CATALOG: no file has been moved, deleted, deduplicated, registered, or committed to repository history.",
            if report.scan_roots.is_empty() {
                report.scan_root.display().to_string()
            } else {
                report
                    .scan_roots
                    .iter()
                    .map(|root| root.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            },
            report.scanned_directories,
            report.ignored_directories,
            report.scanned_files,
            human_bytes(report.scanned_bytes),
            report.project_nodes.len(),
            report.project_edges.len(),
            report.lineage_candidates.len(),
            report.duplicate_groups.len(),
            human_bytes(duplicate_bytes),
            report.skipped_cycles_or_external_links,
            if report.cancelled {
                "    [CANCELLED — PARTIAL CATALOG]"
            } else if report.truncated {
                "    [PREVIEW LIMIT REACHED]"
            } else {
                ""
            },
        );
        Ok(DesktopLibraryView {
            summary,
            projects: if project_rows.is_empty() { vec!["No project boundaries detected.".into()] } else { project_rows },
            lineage,
            storage: if storage.is_empty() { vec!["No classified storage rows.".into()] } else { storage },
            inbox: if unknown.is_empty() { vec!["No unclassified/media summary rows in this catalog.".into()] } else { unknown },
            plans: format_migration_plan_rows(&plans),
            recovery: vec![format!(
                "Recovery root: {}\nRecycle Bin root: {}\nProject migration uses copy → byte verification → destination-side promotion → project identity rebind. Originals remain untouched and promoted projects remain pending quality validation before cleanup.",
                self.registry
                    .library_layout()?
                    .map(|layout| layout.recovery.display().to_string())
                    .unwrap_or_else(|| "Not configured".into()),
                self.registry
                    .library_layout()?
                    .map(|layout| layout.recycle_bin.display().to_string())
                    .unwrap_or_else(|| "Not configured".into())
            )],
            scan_ready: true,
            truncated: report.truncated || report.cancelled,
        })
    }

    pub fn scan_storage_catalog_preview(
        &mut self,
        should_cancel: &mut dyn FnMut() -> bool,
    ) -> Result<DriveCatalogReport, String> {
        // The GUI preview intentionally has a bounded first pass so Cortex remains
        // usable while the future resumable/background full-drive crawler lands.
        // The underlying registry scanner itself supports larger/unbounded limits.
        let report =
            self.registry
                .scan_configured_storage_with_cancel(50_000, 500_000, should_cancel)?;
        self.info = format!(
            "Storage catalog preview completed: {} directories, {} ignored/generated directories, {} files, {} project nodes. {}",
            report.scanned_directories,
            report.ignored_directories,
            report.scanned_files,
            report.project_nodes.len(),
            if report.cancelled {
                "Catalog preview cancelled; partial catalog preserved and no files were moved."
            } else if report.truncated {
                "Preview limit reached; no files were moved."
            } else {
                "Catalog reached the end of the configured storage root; no files were moved."
            }
        );
        Ok(report)
    }

    pub fn scan_machine_catalog_preview(
        &mut self,
        include_removable: bool,
        should_cancel: &mut dyn FnMut() -> bool,
    ) -> Result<DriveCatalogReport, String> {
        let report = self.registry.scan_machine_catalog_with_cancel(
            include_removable,
            250_000,
            2_000_000,
            should_cancel,
        )?;
        self.info = format!(
            "Whole-PC catalog preview completed across {} root(s): {} directories, {} ignored/generated directories, {} files, {} project nodes. {}",
            report.scan_roots.len(),
            report.scanned_directories,
            report.ignored_directories,
            report.scanned_files,
            report.project_nodes.len(),
            if report.cancelled {
                "Catalog preview cancelled; partial catalog preserved and no files were moved."
            } else if report.truncated {
                "Preview limit reached; partial catalog preserved and no files were moved."
            } else {
                "Eligible local volumes were cataloged read-only; no files were moved."
            }
        );
        self.activity.append(
            ActivityKind::Workspace,
            "Cortex whole-PC catalog",
            format!(
                "{} project nodes across {} volume root(s)",
                report.project_nodes.len(),
                report.scan_roots.len()
            ),
            Some(true),
            serde_json::to_value(&report).map_err(|error| error.to_string())?,
        )?;
        Ok(report)
    }

    pub fn apply_catalog_migration(
        &mut self,
        plan_id: &str,
    ) -> Result<cortex_registry::MigrationApplyResult, String> {
        let result = self.registry.apply_managed_migration(plan_id)?;
        self.info = format!(
            "Migration {} promoted a byte-verified copy to {} while preserving the original at {}. The promoted project remains pending quality validation.",
            result.plan_id,
            result.target.display(),
            result.source.display()
        );
        self.activity.append(
            ActivityKind::Workspace,
            "Cortex project migration",
            self.info.clone(),
            Some(true),
            serde_json::to_value(&result).map_err(|error| error.to_string())?,
        )?;
        Ok(result)
    }

    pub fn propose_catalog_migration(&mut self, project_id: &str) -> Result<MigrationPlan, String> {
        let plan = self.registry.propose_managed_migration(project_id)?;
        self.info = format!(
            "Dry-run migration plan created: {}. No filesystem mutation has been performed.",
            plan.title
        );
        Ok(plan)
    }

    pub fn record_desktop_error(&mut self, context: &str, error: &str) -> Result<(), String> {
        let id = self.ensure_conversation()?;
        self.conversations.append(
            &id,
            ConversationRole::Assistant,
            format!(
                "[Error]\n\n{context} could not complete safely.\n\n{error}\n\nNo project success is being claimed. You can adjust the request or reply `continue` after the underlying issue is resolved."
            ),
        )?;
        self.activity.append(
            ActivityKind::Error,
            context,
            error,
            Some(false),
            Value::Null,
        )?;
        self.runtime_status = "developer / blocked — retry available".into();
        Ok(())
    }

    pub fn rate_response(&mut self, message_id: &str, score: i8) -> Result<(), String> {
        let conversation_id = self
            .active_conversation
            .clone()
            .ok_or_else(|| "no active Cortex conversation".to_string())?;
        self.conversations
            .set_feedback(&conversation_id, message_id, score)?;
        self.activity.append(
            ActivityKind::Chat,
            "Response feedback",
            format!("{message_id}: {}", score.clamp(-1, 1)),
            Some(true),
            json!({"message_id": message_id, "score": score.clamp(-1, 1)}),
        )?;
        Ok(())
    }

    pub fn redo_response(&mut self, message_id: &str, instructions: &str) -> Result<(), String> {
        let conversation_id = self
            .active_conversation
            .clone()
            .ok_or_else(|| "no active Cortex conversation".to_string())?;
        let conversation = self.conversations.load(&conversation_id)?;
        let target_index = conversation
            .messages
            .iter()
            .position(|message| message.id == message_id)
            .ok_or_else(|| format!("response not found: {message_id}"))?;
        let target = &conversation.messages[target_index];
        if !matches!(target.role, ConversationRole::Assistant) {
            return Err("only Cortex assistant responses can be regenerated".into());
        }
        let original_user = conversation.messages[..target_index]
            .iter()
            .rev()
            .find(|message| matches!(message.role, ConversationRole::User))
            .map(|message| message.content.as_str())
            .unwrap_or("(no earlier user message recorded)");

        let revision_prompt = format!(
            "Regenerate an earlier Cortex response. Preserve the user's original intent but produce a fresh answer that applies the requested changes. Do not mention this instruction wrapper.\\n\\nORIGINAL USER REQUEST:\\n{original_user}\\n\\nPREVIOUS CORTEX RESPONSE:\\n{}\\n\\nREQUESTED CHANGES:\\n{}",
            target.content,
            if instructions.trim().is_empty() {
                "Improve the response while preserving the original intent."
            } else {
                instructions.trim()
            }
        );
        let contextual_prompt = self.project_chat_prompt(&revision_prompt);
        let client = self.ensure_client()?;
        let value = client.chat(&contextual_prompt)?;
        let text = response_text(&value);
        self.conversations
            .append_revision(&conversation_id, message_id, text.clone())?;
        self.activity.append(
            ActivityKind::Agent,
            "Response regenerated",
            text,
            Some(true),
            json!({"parent_message_id": message_id}),
        )?;
        Ok(())
    }

    pub fn new_conversation(&mut self) -> Result<(), String> {
        let conversation = self.conversations.create("New conversation")?;
        self.active_conversation = Some(conversation.id.clone());
        self.clear_pending_state()?;
        self.activity.append(
            ActivityKind::Chat,
            "New conversation",
            conversation.id,
            Some(true),
            Value::Null,
        )?;
        Ok(())
    }

    pub fn select_conversation(&mut self, index: usize) -> Result<(), String> {
        let conversations = self.conversations.list(false)?;
        let conversation = conversations
            .get(index)
            .ok_or_else(|| format!("conversation index is out of range: {index}"))?;
        self.active_conversation = Some(conversation.id.clone());
        self.pending_handoff = None;
        self.pending_project_creation = None;
        self.pending_provider_request = None;
        self.persist_background_state()?;
        Ok(())
    }

    pub fn archive_current_conversation(&mut self) -> Result<(), String> {
        let Some(id) = self.active_conversation.clone() else {
            return Err("no active Cortex conversation to archive".into());
        };
        let archived = self.conversations.archive(&id)?;
        self.activity.append(
            ActivityKind::Chat,
            "Conversation archived",
            archived.title,
            Some(true),
            json!({"conversation_id": id}),
        )?;
        self.active_conversation = self
            .conversations
            .list(false)?
            .first()
            .map(|conversation| conversation.id.clone());
        self.pending_handoff = None;
        self.pending_project_creation = None;
        self.pending_provider_request = None;
        self.persist_background_state()?;
        Ok(())
    }

    pub fn export_conversation(&mut self, index: usize, format: &str) -> Result<PathBuf, String> {
        let conversations = self.conversations.list(false)?;
        let conversation = conversations
            .get(index)
            .ok_or_else(|| format!("conversation index is out of range: {index}"))?;
        let export_root = self
            .workspace
            .root()
            .join("artifacts")
            .join("conversations");
        fs::create_dir_all(&export_root).map_err(|error| error.to_string())?;
        let extension = normalize_conversation_export_format(format)?;
        let path = export_root.join(format!(
            "{}_{}_{}.{}",
            safe_export_stem(&self.workspace.profile().name),
            safe_export_stem(&conversation.title),
            conversation.updated_unix_ms,
            extension
        ));
        fs::write(&path, render_conversation_export(conversation, extension)?)
            .map_err(|error| error.to_string())?;
        self.activity.append(
            ActivityKind::Artifact,
            "Conversation exported",
            path.display().to_string(),
            Some(true),
            json!({"conversation_id": conversation.id.clone(), "format": extension}),
        )?;
        Ok(path)
    }

    pub fn export_project_conversations(&mut self, format: &str) -> Result<PathBuf, String> {
        let conversations = self.conversations.list(false)?;
        let export_root = self
            .workspace
            .root()
            .join("artifacts")
            .join("conversations");
        fs::create_dir_all(&export_root).map_err(|error| error.to_string())?;
        let extension = normalize_conversation_export_format(format)?;
        let stamp = conversations
            .iter()
            .map(|item| item.updated_unix_ms)
            .max()
            .unwrap_or_default();
        let path = export_root.join(format!(
            "{}_conversation_bundle_{}.{}",
            safe_export_stem(&self.workspace.profile().name),
            stamp,
            extension
        ));
        let body = if extension == "json" {
            serde_json::to_string_pretty(&conversations).map_err(|error| error.to_string())?
        } else {
            let separator = if extension == "md" {
                "\n\n---\n\n"
            } else {
                "\n\n==============================\n\n"
            };
            conversations
                .iter()
                .map(|item| render_conversation_export(item, extension))
                .collect::<Result<Vec<_>, _>>()?
                .join(separator)
        };
        fs::write(&path, body).map_err(|error| error.to_string())?;
        self.activity.append(
            ActivityKind::Artifact,
            "Project conversations exported",
            path.display().to_string(),
            Some(true),
            json!({"count": conversations.len(), "format": extension}),
        )?;
        Ok(path)
    }

    pub fn send_chat(&mut self, prompt: &str) -> Result<(), String> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Ok(());
        }

        if self.pending_provider_request.is_some() {
            if is_provider_retry_request(prompt) {
                self.retry_pending_provider_request(None)?;
                return Ok(());
            }
            let _ = self.cancel_pending_provider_request()?;
        }

        if self.handle_pending_handoff_response(prompt)? {
            return Ok(());
        }

        if let Some(operation) = direct_project_operation(prompt) {
            self.run_project_operation_into_conversation(prompt, operation)?;
            return Ok(());
        }

        if is_recovery_snapshot_request(prompt) {
            self.snapshot_internal_git_into_conversation(prompt)?;
            return Ok(());
        }

        if is_library_unregistration_request(prompt) {
            self.unregister_library_project_into_conversation(prompt)?;
            return Ok(());
        }

        if is_library_registration_request(prompt) {
            self.register_library_candidates_into_conversation(prompt)?;
            return Ok(());
        }

        if is_machine_scan_request(prompt) {
            self.scan_machine_into_conversation(prompt)?;
            return Ok(());
        }

        if is_library_scan_request(prompt) {
            self.scan_library_into_conversation(prompt)?;
            return Ok(());
        }

        if is_context_index_request(prompt) {
            self.rebuild_context_index_into_conversation(prompt)?;
            return Ok(());
        }

        if let Some(intent) = self.routed_developer_intent(prompt) {
            self.run_developer_workflow(prompt, intent)?;
            return Ok(());
        }

        let id = self.prepare_user_message(prompt)?;
        let trace = self.chat_trace(&id);
        self.observe(
            ProjectEventKind::ChatRequestStarted,
            EventSeverity::Info,
            "Chat request started",
            trace.clone(),
            json!({"prompt_chars": prompt.chars().count(), "streaming": false}),
        );
        let contextual_prompt = self.project_chat_prompt(prompt);
        let client = match self.ensure_client() {
            Ok(client) => client,
            Err(error) => {
                self.observe(
                    ProjectEventKind::ChatFailed,
                    EventSeverity::Error,
                    &format!("Chat transport unavailable: {error}"),
                    trace,
                    json!({"stage":"ensure_client"}),
                );
                return Err(error);
            }
        };
        self.observe(
            ProjectEventKind::ChatProviderRequest,
            EventSeverity::Info,
            "Provider request dispatched",
            trace.clone(),
            json!({"streaming": false}),
        );
        let mut response = client.chat(&contextual_prompt);
        if let Err(error) = &response {
            if let Some(retry_client) = self.recover_native_transport_reset(error)? {
                response = retry_client.chat(&contextual_prompt);
            }
        }
        match response {
            Ok(value) => {
                self.runtime_status = "online / active request".into();
                let text = response_text(&value);
                self.conversations
                    .append(&id, ConversationRole::Assistant, text.clone())?;
                self.activity.append(
                    ActivityKind::Agent,
                    "Chat response",
                    text.clone(),
                    Some(true),
                    value,
                )?;
                self.observe(
                    ProjectEventKind::ChatCompleted,
                    EventSeverity::Info,
                    "Chat request completed",
                    trace,
                    json!({"assistant_chars": text.chars().count(), "streaming": false}),
                );
                Ok(())
            }
            Err(error) if is_recoverable_provider_error(&error) => {
                self.observe(
                    ProjectEventKind::ChatFailed,
                    EventSeverity::Warning,
                    &format!("Provider unavailable; request preserved: {error}"),
                    trace,
                    json!({"recoverable": true}),
                );
                self.defer_provider_request(&id, prompt, &error)?;
                Ok(())
            }
            Err(error) => {
                self.runtime_status = format!("offline / {error}");
                self.activity.append(
                    ActivityKind::Error,
                    "Chat failed",
                    &error,
                    Some(false),
                    Value::Null,
                )?;
                self.observe(
                    ProjectEventKind::ChatFailed,
                    EventSeverity::Error,
                    &format!("Chat request failed: {error}"),
                    trace,
                    json!({"recoverable": false}),
                );
                Err(error)
            }
        }
    }

    pub fn send_chat_stream(
        &mut self,
        prompt: &str,
        on_event: &mut dyn FnMut(CortexStreamEvent),
    ) -> Result<(), String> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Ok(());
        }

        if self.pending_provider_request.is_some() {
            if is_provider_retry_request(prompt) {
                self.retry_pending_provider_request(Some(on_event))?;
                return Ok(());
            }
            let _ = self.cancel_pending_provider_request()?;
        }

        if self.handle_pending_handoff_response(prompt)? {
            return Ok(());
        }

        if let Some(operation) = direct_project_operation(prompt) {
            self.run_project_operation_into_conversation(prompt, operation)?;
            return Ok(());
        }

        if is_recovery_snapshot_request(prompt) {
            self.snapshot_internal_git_into_conversation(prompt)?;
            return Ok(());
        }

        if is_library_unregistration_request(prompt) {
            self.unregister_library_project_into_conversation(prompt)?;
            return Ok(());
        }

        if is_library_registration_request(prompt) {
            self.register_library_candidates_into_conversation(prompt)?;
            return Ok(());
        }

        if is_machine_scan_request(prompt) {
            self.scan_machine_into_conversation(prompt)?;
            return Ok(());
        }

        if is_library_scan_request(prompt) {
            self.scan_library_into_conversation(prompt)?;
            return Ok(());
        }

        if is_context_index_request(prompt) {
            self.rebuild_context_index_into_conversation(prompt)?;
            return Ok(());
        }

        if let Some(intent) = self.routed_developer_intent(prompt) {
            self.run_developer_workflow(prompt, intent)?;
            return Ok(());
        }

        let id = self.prepare_user_message(prompt)?;
        let trace = self.chat_trace(&id);
        self.observe(
            ProjectEventKind::ChatRequestStarted,
            EventSeverity::Info,
            "Streaming chat request started",
            trace.clone(),
            json!({"prompt_chars": prompt.chars().count(), "streaming": true}),
        );
        let contextual_prompt = self.project_chat_prompt(prompt);
        let client = match self.ensure_client() {
            Ok(client) => client,
            Err(error) => {
                self.observe(
                    ProjectEventKind::ChatFailed,
                    EventSeverity::Error,
                    &format!("Streaming chat transport unavailable: {error}"),
                    trace,
                    json!({"stage":"ensure_client"}),
                );
                return Err(error);
            }
        };
        self.observe(
            ProjectEventKind::ChatProviderRequest,
            EventSeverity::Info,
            "Streaming provider request dispatched",
            trace.clone(),
            json!({"streaming": true}),
        );

        let observability = self.observability.clone();
        let stream_trace = trace.clone();
        let mut stream_events = 0usize;
        let visible_stream_chars = Cell::new(0usize);
        let mut stream_started = false;
        let mut wrapped = |event: CortexStreamEvent| {
            stream_events = stream_events.saturating_add(1);
            if matches!(event.kind, CortexStreamKind::TextDelta) {
                visible_stream_chars.set(
                    visible_stream_chars
                        .get()
                        .saturating_add(event.text_delta.chars().count()),
                );
            }
            if !stream_started {
                let mut started = ProjectEvent::new(
                    new_event_id("chat-stream"),
                    ProjectEventKind::ChatStreamStarted,
                    "cortex_desktop",
                    "Provider stream started",
                    stream_trace.clone(),
                );
                started
                    .attributes
                    .insert("streaming".into(), Value::Bool(true));
                let _ = observability.record(&started);
                stream_started = true;
            }
            on_event(event);
        };

        let mut response = client.chat_stream(&contextual_prompt, &mut wrapped);
        if let Err(error) = &response {
            // A Windows loopback reset can occur when llama-server/model worker exits.
            // Retry exactly once only when no user-visible text was emitted; this
            // avoids duplicating a partially streamed assistant answer.
            if visible_stream_chars.get() == 0 {
                if let Some(retry_client) = self.recover_native_transport_reset(error)? {
                    response = retry_client.chat_stream(&contextual_prompt, &mut wrapped);
                }
            }
        }
        match response {
            Ok(value) => {
                self.runtime_status = "online / active request".into();
                let text = response_text(&value);
                self.conversations
                    .append(&id, ConversationRole::Assistant, text.clone())?;
                self.activity.append(
                    ActivityKind::Agent,
                    "Chat response",
                    text.clone(),
                    Some(true),
                    value,
                )?;
                self.observe(
                    ProjectEventKind::ChatCompleted,
                    EventSeverity::Info,
                    "Streaming chat request completed",
                    trace,
                    json!({"assistant_chars": text.chars().count(), "stream_events": stream_events, "streaming": true}),
                );
                Ok(())
            }
            Err(error) if is_recoverable_provider_error(&error) => {
                self.observe(
                    ProjectEventKind::ChatFailed,
                    EventSeverity::Warning,
                    &format!("Provider unavailable; streaming request preserved: {error}"),
                    trace,
                    json!({"recoverable": true, "stream_events": stream_events}),
                );
                self.defer_provider_request(&id, prompt, &error)?;
                Ok(())
            }
            Err(error) => {
                self.runtime_status = format!("offline / {error}");
                self.activity.append(
                    ActivityKind::Error,
                    "Chat stream failed",
                    &error,
                    Some(false),
                    Value::Null,
                )?;
                self.observe(
                    ProjectEventKind::ChatFailed,
                    EventSeverity::Error,
                    &format!("Streaming chat request failed: {error}"),
                    trace,
                    json!({"recoverable": false, "stream_events": stream_events}),
                );
                Err(error)
            }
        }
    }

    /// Interrupt the active local provider request when Cortex owns the provider process.
    /// Native inference is Desktop-owned, so stopping the model host closes the active
    /// loopback stream immediately. Compatibility providers such as LM Studio are
    /// external processes and are never killed by Cortex; their returned result is
    /// still discarded by the Desktop cancellation guard.
    pub fn cancel_active_request(&mut self) -> Result<bool, String> {
        // O2D-R051N9STOR1H_TERMINAL_CANCELLATION
        // Cancel durable execution ownership first so transactions/provider leases
        // cannot remain live even when the provider itself is external.
        let terminal_cancelled = cancel_active_executions_for_owner(
            std::process::id(),
            "Desktop user requested cancellation",
        )?;
        let mut interrupted = false;
        if self.settings.provider.eq_ignore_ascii_case("native") {
            if let Some(model_host) = &self.model_host {
                interrupted = model_host.stop()?;
            }
        }
        let pending_cancelled = self.cancel_pending_provider_request()?;
        if interrupted {
            self.runtime_status = "native provider / interrupted by user".into();
            self.activity.append(
                ActivityKind::Service,
                "Active Cortex request interrupted",
                "Desktop stopped the Cortex-owned native model host; the next request will restart it on demand.",
                Some(true),
                json!({"provider":"native","hard_interrupt":true}),
            )?;
        } else if self.settings.provider.eq_ignore_ascii_case("lmstudio") {
            self.activity.append(
                ActivityKind::Service,
                "Active Cortex request cancellation requested",
                "LM Studio is an external compatibility provider, so Cortex will discard the active result but will not terminate LM Studio itself.",
                Some(true),
                json!({"provider":"lmstudio","hard_interrupt":false}),
            )?;
        }
        Ok(interrupted || pending_cancelled || terminal_cancelled > 0)
    }

    pub fn provider_recovery_pending(&self) -> bool {
        self.pending_provider_request.is_some()
    }

    pub fn cancel_pending_provider_request(&mut self) -> Result<bool, String> {
        self.sync_pending_state()?;
        let Some(pending) = self.pending_provider_request.take() else {
            return Ok(false);
        };
        let _ = self.conversations.update_message_content(
            &pending.conversation_id,
            &pending.status_message_id,
            provider_recovery_cancelled_card(&pending.last_error),
        );
        self.persist_background_state()?;
        self.runtime_status = "online / provider retry stopped".into();
        self.activity.append(
            ActivityKind::Service,
            "Provider retry stopped",
            "Preserved provider request discarded by user; automatic replay is disabled.",
            Some(true),
            json!({"request_preserved": false, "cancelled": true}),
        )?;
        Ok(true)
    }

    pub fn resume_pending_provider_request(&mut self) -> Result<bool, String> {
        if self.pending_provider_request.is_none() {
            return Ok(false);
        }
        self.retry_pending_provider_request(None)?;
        Ok(self.pending_provider_request.is_none())
    }

    fn defer_provider_request(
        &mut self,
        conversation_id: &str,
        original_prompt: &str,
        error: &str,
    ) -> Result<(), String> {
        let conversation = self.conversations.append(
            conversation_id,
            ConversationRole::Assistant,
            provider_recovery_card(error, 1, false),
        )?;
        let status_message_id = conversation
            .messages
            .last()
            .map(|message| message.id.clone())
            .ok_or_else(|| {
                "Cortex could not create provider recovery status message".to_string()
            })?;

        self.pending_provider_request = Some(PendingProviderRequest {
            conversation_id: conversation_id.to_string(),
            original_prompt: original_prompt.to_string(),
            status_message_id,
            attempts: 1,
            last_error: error.to_string(),
        });
        self.persist_background_state()?;
        self.runtime_status = "provider / waiting — request preserved".into();
        self.activity.append(
            ActivityKind::Service,
            "Provider recovery wait",
            error,
            Some(false),
            json!({"request_preserved": true}),
        )?;
        Ok(())
    }

    fn retry_pending_provider_request(
        &mut self,
        mut on_event: Option<&mut dyn FnMut(CortexStreamEvent)>,
    ) -> Result<(), String> {
        let Some(mut pending) = self.pending_provider_request.clone() else {
            return Ok(());
        };
        pending.attempts = pending.attempts.saturating_add(1);

        let contextual_prompt = self.project_chat_prompt(&pending.original_prompt);
        let client = match self.ensure_client() {
            Ok(client) => client,
            Err(error) if is_recoverable_provider_error(&error) => {
                self.update_provider_wait(&mut pending, &error)?;
                return Ok(());
            }
            Err(error) => return Err(error),
        };

        match client.provider_status() {
            Ok(status) if status.get("ready").and_then(Value::as_bool) == Some(true) => {}
            Ok(status) => {
                let detail = provider_status_detail(&status);
                self.update_provider_wait(&mut pending, &detail)?;
                return Ok(());
            }
            Err(error) if is_recoverable_provider_error(&error) => {
                self.update_provider_wait(&mut pending, &error)?;
                return Ok(());
            }
            Err(error) => return Err(error),
        }

        let response = match on_event.as_mut() {
            Some(callback) => client.chat_stream(&contextual_prompt, *callback),
            None => client.chat(&contextual_prompt),
        };

        match response {
            Ok(value) => {
                self.conversations.update_message_content(
                    &pending.conversation_id,
                    &pending.status_message_id,
                    provider_recovery_card(
                        "The configured Cortex provider is ready.",
                        pending.attempts,
                        true,
                    ),
                )?;
                let text = response_text(&value);
                self.conversations.append(
                    &pending.conversation_id,
                    ConversationRole::Assistant,
                    text.clone(),
                )?;
                self.activity.append(
                    ActivityKind::Agent,
                    "Preserved request resumed",
                    text,
                    Some(true),
                    value,
                )?;
                self.pending_provider_request = None;
                self.persist_background_state()?;
                self.runtime_status = "online / resumed preserved request".into();
                Ok(())
            }
            Err(error) if is_recoverable_provider_error(&error) => {
                self.update_provider_wait(&mut pending, &error)
            }
            Err(error) => Err(error),
        }
    }

    fn update_provider_wait(
        &mut self,
        pending: &mut PendingProviderRequest,
        error: &str,
    ) -> Result<(), String> {
        self.conversations.update_message_content(
            &pending.conversation_id,
            &pending.status_message_id,
            provider_recovery_card(error, pending.attempts, false),
        )?;
        pending.last_error = error.to_string();
        self.pending_provider_request = Some(pending.clone());
        self.persist_background_state()?;
        self.runtime_status = "provider / waiting — request preserved".into();
        Ok(())
    }

    fn rebuild_context_index_into_conversation(&mut self, prompt: &str) -> Result<(), String> {
        let id = self.prepare_user_message(prompt)?;
        let result = self.rebuild_context_index(100_000, 512 * 1024)?;
        let index = result.get("index").cloned().unwrap_or(Value::Null);
        let file_count = index
            .get("files")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        let languages = index
            .get("language_counts")
            .cloned()
            .unwrap_or_else(|| json!({}));
        self.conversations.append(
            &id,
            ConversationRole::Assistant,
            format!(
                "[Index]\n\nProject context index rebuilt successfully.\n\nFiles indexed: {file_count}\nLanguages: {}\n\nThe index is now available to Cortex for project-aware conversation and downstream tools.",
                serde_json::to_string(&languages).unwrap_or_else(|_| "{}".into())
            ),
        )?;
        self.runtime_status = "online / project indexed".into();
        Ok(())
    }

    pub fn run_project_operation(&mut self, operation: ProjectOperation) -> Result<Value, String> {
        let service = ProcessService::default();
        let result = service.run_project_operation(self.workspace.root(), operation)?;
        let value = serde_json::to_value(&result).map_err(|error| error.to_string())?;
        self.info = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
        self.activity.append(
            ActivityKind::Build,
            format!("Project {:?}", operation),
            format!(
                "{} {:?} — exit {:?} in {} ms",
                result.program, result.args, result.exit_code, result.duration_ms
            ),
            Some(result.success),
            value.clone(),
        )?;
        if result.success {
            Ok(value)
        } else {
            Err(format!(
                "project {:?} failed with exit {:?}\n{}",
                operation, result.exit_code, result.stderr
            ))
        }
    }

    pub fn snapshot_internal_git(&mut self, label: &str) -> Result<Value, String> {
        let record = self
            .registry
            .resolve_for_root(self.workspace.root())?
            .ok_or_else(|| "active workspace is not registered with Cortex".to_string())?;
        let git_dir = self.registry.internal_git_dir_for(&record.id)?;
        let snapshot = ShadowGitStore::snapshot(self.workspace.root(), &git_dir, label)?;
        let value = serde_json::to_value(&snapshot).map_err(|error| error.to_string())?;
        self.activity.append(
            ActivityKind::Workspace,
            "Internal Git recovery snapshot",
            format!("{} — {}", snapshot.commit, label),
            Some(true),
            value.clone(),
        )?;
        self.info = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
        Ok(value)
    }

    fn run_project_operation_into_conversation(
        &mut self,
        prompt: &str,
        operation: ProjectOperation,
    ) -> Result<(), String> {
        let id = self.prepare_user_message(prompt)?;
        match self.run_project_operation(operation) {
            Ok(result) => {
                self.conversations.append(
                    &id,
                    ConversationRole::Assistant,
                    format!(
                        "[Project Operation]\n\n`{:?}` completed successfully.\n\n```json\n{}\n```",
                        operation,
                        serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
                    ),
                )?;
                if operation == ProjectOperation::Build && prompt_requests_launch(prompt) {
                    self.launch_verified_project_into_conversation(&id)?;
                }
                Ok(())
            }
            Err(error)
                if matches!(
                    operation,
                    ProjectOperation::Build | ProjectOperation::Validate | ProjectOperation::Test
                ) =>
            {
                // H56: a natural-language developer request to build/test/validate is not
                // a dead-end command runner.  If the operation exposes a source failure,
                // escalate directly into the bounded Repair agent using the same selected
                // project and active transaction.  This is the missing execution bridge that
                // lets Cortex repair its own malformed edit instead of asking the user to fix it.
                self.activity.append(
                    ActivityKind::Agent,
                    "Project operation repair escalation",
                    format!("{:?} failed; invoking bounded project repair", operation),
                    Some(false),
                    json!({"operation": format!("{:?}", operation), "error": error}),
                )?;
                let repair_prompt = format!(
                    "The user explicitly requested project operation `{operation:?}` and it failed. \
Inspect the real selected-project files and diagnostics, reuse the active Cortex transaction when one exists, repair only the concrete source/configuration errors required to make the project quality gate green, and verify the result yourself. \
Do not ask the user to edit files and do not stop at suggested code.\n\nORIGINAL USER REQUEST:\n{prompt}\n\nFAILED OPERATION:\n{error}"
                );
                self.run_agent_internal("repair", &repair_prompt, false, None)?;
                if prompt_requests_launch(prompt) {
                    self.launch_verified_project_into_conversation(&id)?;
                }
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn launch_verified_project_into_conversation(
        &mut self,
        conversation_id: &str,
    ) -> Result<Value, String> {
        let client = self.ensure_client()?;
        let launch = client.tool("runtime.launch_project", json!({"args": []}))?;
        let pid = launch.get("pid").and_then(Value::as_u64);
        self.activity.append(
            ActivityKind::Service,
            "Verified project launch",
            pid.map(|value| {
                format!("launched verified project under Cortex ownership (PID {value})")
            })
            .unwrap_or_else(|| "launched verified project under Cortex ownership".into()),
            Some(true),
            launch.clone(),
        )?;
        self.conversations.append(
            conversation_id,
            ConversationRole::Assistant,
            format!(
                "[Runtime]\n\nThe verified project build was launched under Cortex ownership.{}",
                pid.map(|value| format!(" PID: `{value}`."))
                    .unwrap_or_default()
            ),
        )?;
        Ok(launch)
    }

    fn snapshot_internal_git_into_conversation(&mut self, prompt: &str) -> Result<(), String> {
        let id = self.prepare_user_message(prompt)?;
        let label = format!(
            "Cortex recovery checkpoint — {}",
            conversation_title(prompt)
        );
        let snapshot = self.snapshot_internal_git(&label)?;
        self.conversations.append(
            &id,
            ConversationRole::Assistant,
            format!(
                "[Recovery Snapshot]\n\nCreated a Cortex-internal shadow Git checkpoint without adding `.git` to the project folder.\n\n```json\n{}\n```",
                serde_json::to_string_pretty(&snapshot).map_err(|error| error.to_string())?
            ),
        )?;
        Ok(())
    }

    pub fn scan_library(&mut self) -> Result<LibraryScanReport, String> {
        if self.library_memory.is_none() {
            self.library_memory = self
                .registry
                .library_root()?
                .map(|record| LibraryMemoryDatabase::open(&record.root))
                .transpose()?;
        }

        let report = self.registry.scan_library(100_000)?;
        if let Some(memory) = self.library_memory.as_mut() {
            for candidate in &report.candidates {
                let kind = if candidate.already_registered {
                    LibraryItemKind::RegisteredProject
                } else {
                    LibraryItemKind::ProjectCandidate
                };
                let _ = memory.record_catalog_path(&candidate.root, kind, None);
                if !candidate.already_registered
                    && !memory.organization.iter().any(|suggestion| {
                        suggestion.status == "pending"
                            && suggestion.path == candidate.root
                            && suggestion.category == "project_candidate"
                    })
                {
                    memory.propose_organization(
                        candidate.root.clone(),
                        "project_candidate",
                        &format!(
                            "{} markers detected with confidence {}",
                            candidate.kind, candidate.confidence
                        ),
                        candidate.confidence,
                        "Review and register this folder as a Cortex project",
                    );
                }
            }
            memory.scan_cursor.fallback_scan_unix_ms = report.generated_unix_ms;
            let _ = memory.save();
        }
        let intake = self.registry.scan_intake(2_000, 10_000).ok();
        self.runtime_status = match intake.as_ref() {
            Some(intake) => format!(
                "Vault indexed / {} candidates / {} stable intake item(s)",
                report.candidates.len(),
                intake.stable_items
            ),
            None => format!("Vault indexed / {} candidates", report.candidates.len()),
        };
        self.info = library_scan_text(&report);
        if let Some(intake) = intake.as_ref() {
            self.info.push_str(&format!(
                "\n\nCORTEX INTAKE\nStable items: {}\nUnassigned items: {}\nIntake root: {}\n",
                intake.stable_items,
                intake.unassigned_items,
                intake.root.display()
            ));
        }
        self.activity.append(
            ActivityKind::Workspace,
            "Cortex Vault scan",
            format!(
                "{} candidate projects across {} directories",
                report.candidates.len(),
                report.scanned_directories
            ),
            Some(true),
            serde_json::to_value(&report).map_err(|error| error.to_string())?,
        )?;
        Ok(report)
    }

    pub fn ingest_paths(&mut self, paths: &[String]) -> Result<(), String> {
        let id = self.ensure_conversation()?;
        if paths.is_empty() {
            return Ok(());
        }
        let mut findings = Vec::new();
        for raw in paths.iter().take(32) {
            findings.push(self.ingest_path(Path::new(raw))?);
        }
        self.conversations.append(
            &id,
            ConversationRole::User,
            format!("[Attached {} item(s)]\n{}", paths.len(), paths.join("\n")),
        )?;
        self.conversations.append(
            &id,
            ConversationRole::Assistant,
            format!("[Attachment intake]\n\n{}", findings.join("\n\n")),
        )?;
        self.runtime_status = format!("attachment intake / {} item(s)", paths.len());
        Ok(())
    }

    fn ingest_path(&mut self, path: &Path) -> Result<String, String> {
        let canonical = fs::canonicalize(path).map_err(|error| {
            format!("failed to resolve dropped path {}: {error}", path.display())
        })?;
        if canonical.is_dir() {
            return match self.registry.inspect_candidate(&canonical)? {
                Some(candidate) => Ok(format_library_candidate(&candidate)),
                None => Ok(format!(
                    "Folder `{}` was scanned. It does not currently look like a standalone project boundary; Cortex left it unregistered.",
                    canonical.display()
                )),
            };
        }
        if !canonical.is_file() {
            return Ok(format!(
                "`{}` is not a regular file and was not ingested.",
                canonical.display()
            ));
        }

        let metadata = fs::metadata(&canonical).map_err(|error| error.to_string())?;
        let name = canonical
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("attachment")
            .to_string();
        let extension = canonical
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let attachments = self.workspace.cortex_state_dir().join("attachments");
        fs::create_dir_all(&attachments).map_err(|error| error.to_string())?;
        let managed_name = managed_attachment_name(&name, &canonical);
        let destination = attachments.join(&managed_name);
        fs::copy(&canonical, &destination).map_err(|error| error.to_string())?;

        if matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp") {
            let prompt = "Identify this attached image and explain what it contains, what kind of development asset/evidence it appears to be, and which current-project workflow it is likely useful for. Be specific and do not invent unreadable text.";
            let vision = self.ensure_client().and_then(|client| {
                client.tool(
                    "vision.inspect",
                    json!({
                        "path": format!("attachment://{managed_name}"),
                        "prompt": prompt
                    }),
                )
            });
            return match vision {
                Ok(value) => Ok(format!(
                    "Image `{name}` ({} bytes) was copied into managed Cortex attachments and inspected with vision:\n{}",
                    metadata.len(), response_text(&value)
                )),
                Err(error) => Ok(format!(
                    "Image `{name}` ({} bytes) was copied into managed Cortex attachments. Vision was appropriate but is currently unavailable: {error}",
                    metadata.len()
                )),
            };
        }

        if is_text_attachment(&extension) && metadata.len() <= 8 * 1024 * 1024 {
            let text = fs::read_to_string(&destination).map_err(|error| error.to_string())?;
            let excerpt = text.chars().take(12_000).collect::<String>();
            return Ok(format!(
                "Text/source attachment `{name}` ({} bytes, .{extension}) was staged safely. Cortex read a bounded excerpt ({} characters).\n```\n{}\n```",
                metadata.len(), excerpt.chars().count(), excerpt
            ));
        }

        let kind = if matches!(extension.as_str(), "zip" | "7z" | "rar" | "tar" | "gz") {
            "archive"
        } else if matches!(extension.as_str(), "pdf" | "doc" | "docx" | "odt") {
            "document"
        } else {
            "binary/unknown"
        };
        Ok(format!(
            "Attachment `{name}` ({} bytes) was staged as {kind}. Cortex identified it without executing or blindly extracting it; a format-specific reader can be invoked when needed.",
            metadata.len()
        ))
    }

    fn scan_machine_into_conversation(&mut self, prompt: &str) -> Result<(), String> {
        let id = self.prepare_user_message(prompt)?;
        let report = self
            .registry
            .scan_machine_catalog(false, 1_000_000, 8_000_000)?;
        let view = self.library_view()?;
        let project_rows = report
            .project_nodes
            .iter()
            .take(60)
            .map(|node| {
                format!(
                    "- {} — {:?} — confidence {}%{}\n  id: `{}`\n  `{}`",
                    node.name,
                    node.kind,
                    node.confidence,
                    if node.already_registered {
                        " — already registered"
                    } else {
                        ""
                    },
                    node.id,
                    node.root.display()
                )
            })
            .collect::<Vec<_>>();
        let text = format!(
            "[Whole-PC Project Catalog]\n\n{}\n\nPROJECT CANDIDATES\n{}\n\nCatalog discovery is read-only. No candidate was registered, moved, deleted, deduplicated, or migrated. Project-family/lineage review is required before bulk registration or migration.",
            view.summary,
            if project_rows.is_empty() {
                "No project boundaries detected.".to_string()
            } else {
                project_rows.join("\n")
            }
        );
        self.conversations
            .append(&id, ConversationRole::Assistant, text)?;
        self.runtime_status = format!(
            "whole-PC catalog / {} project node(s) / {} root(s)",
            report.project_nodes.len(),
            report.scan_roots.len()
        );
        self.activity.append(
            ActivityKind::Workspace,
            "Cortex whole-PC catalog",
            format!(
                "{} project nodes across {} local volume root(s)",
                report.project_nodes.len(),
                report.scan_roots.len()
            ),
            Some(true),
            serde_json::to_value(&report).map_err(|error| error.to_string())?,
        )?;
        Ok(())
    }

    fn scan_library_into_conversation(&mut self, prompt: &str) -> Result<(), String> {
        let id = self.prepare_user_message(prompt)?;
        let report = self.registry.scan_configured_storage(100_000, 1_000_000)?;
        let view = self.library_view()?;
        let project_rows = report
            .project_nodes
            .iter()
            .take(40)
            .map(|node| {
                format!(
                    "- {} — {:?} — confidence {}%{}\n  id: `{}`\n  `{}`",
                    node.name,
                    node.kind,
                    node.confidence,
                    if node.already_registered {
                        " — already registered"
                    } else {
                        ""
                    },
                    node.id,
                    node.root.display()
                )
            })
            .collect::<Vec<_>>();
        let lineage_rows = report
            .lineage_candidates
            .iter()
            .take(20)
            .map(|candidate| {
                format!(
                    "- {} — {} related project tree(s) — confidence {}% — {}",
                    candidate.normalized_name,
                    candidate.project_ids.len(),
                    candidate.confidence,
                    candidate.reason
                )
            })
            .collect::<Vec<_>>();
        let mut text = format!(
            "[Vault Catalog]\n\n{}\n\nPROJECT CANDIDATES\n{}",
            view.summary,
            if project_rows.is_empty() {
                "No project boundaries detected.".to_string()
            } else {
                project_rows.join("\n")
            }
        );
        if report.project_nodes.len() > project_rows.len() {
            text.push_str(&format!(
                "\n\n… {} additional project nodes are retained in the persistent catalog.",
                report.project_nodes.len() - project_rows.len()
            ));
        }
        text.push_str("\n\nPROJECT-FAMILY / LINEAGE REVIEW\n");
        if lineage_rows.is_empty() {
            text.push_str("No related-copy lineage candidates detected.");
        } else {
            text.push_str(&lineage_rows.join("\n"));
        }
        text.push_str(
            "\n\nCataloging is read-only. Confidence never authorizes registration, Git initialization, file movement, deduplication, or deletion. Registration is explicit-target only: use `register <exact unique name>`, `register <full path>`, or `register <catalog id>`. Ambiguous names require a full path or catalog id. Bulk registration is review-gated."
        );
        self.conversations
            .append(&id, ConversationRole::Assistant, text)?;
        self.activity.append(
            ActivityKind::Vault,
            "Cortex storage catalog",
            format!(
                "{} project nodes / {} lineage candidates / {} files; read-only catalog",
                report.project_nodes.len(),
                report.lineage_candidates.len(),
                report.scanned_files
            ),
            Some(true),
            json!({
                "scan_root": report.scan_root,
                "project_nodes": report.project_nodes.len(),
                "lineage_candidates": report.lineage_candidates.len(),
                "read_only": true
            }),
        )?;
        self.runtime_status = format!(
            "Vault catalog / {} project nodes / read-only",
            report.project_nodes.len()
        );
        Ok(())
    }

    fn register_library_candidates_into_conversation(
        &mut self,
        prompt: &str,
    ) -> Result<(), String> {
        let id = self.prepare_user_message(prompt)?;
        let selector = explicit_library_registration_target(prompt).ok_or_else(|| {
            "Project registration requires an explicit command such as `register <exact project name>`, `register <full path>`, or `register <catalog id>`.".to_string()
        })?;
        let report = self.registry.last_drive_catalog()?.ok_or_else(|| {
            "No Cortex storage catalog is available yet. Run `scan vault` first.".to_string()
        })?;

        if is_bulk_library_registration_selector(&selector) {
            let lineage = if report.lineage_candidates.is_empty() {
                "No related-copy lineage candidates are currently recorded.".to_string()
            } else {
                report
                    .lineage_candidates
                    .iter()
                    .take(30)
                    .map(|candidate| {
                        format!(
                            "- {} — {} related project tree(s) — {}",
                            candidate.normalized_name,
                            candidate.project_ids.len(),
                            candidate.reason
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            self.conversations.append(
                &id,
                ConversationRole::Assistant,
                format!(
                    "[Vault Registration Review]\n\nBulk registration is review-gated. No projects were registered. Marker/confidence detection proves only that a folder resembles a project; it does not establish canonical authority. Review project-family, duplicate, backup, snapshot, nested-component and lineage relationships first.\n\n{lineage}\n\nRegister approved projects individually by exact name, full path, or catalog id."
                ),
            )?;
            self.activity.append(
                ActivityKind::Approval,
                "Bulk project registration blocked for review",
                "No projects were registered; project-family/lineage review is required.",
                Some(true),
                json!({"selector": selector, "registered": 0}),
            )?;
            return Ok(());
        }

        let mut selected = report
            .project_nodes
            .iter()
            .filter(|candidate| !candidate.already_registered)
            .filter(|candidate| catalog_project_matches_selector(candidate, &selector))
            .collect::<Vec<_>>();
        selected.sort_by(|left, right| left.root.cmp(&right.root));
        if selected.is_empty() {
            self.conversations.append(
                &id,
                ConversationRole::Assistant,
                format!(
                    "[Vault Registration]\n\nNo unregistered catalog project exactly matched `{selector}`. No project was registered. Use the exact unique name, full path, or catalog id shown by `scan vault`."
                ),
            )?;
            return Ok(());
        }
        if selected.len() > 1 {
            let matches = selected
                .iter()
                .map(|candidate| {
                    format!(
                        "- {} → `{}` (id `{}`)",
                        candidate.name,
                        candidate.root.display(),
                        candidate.id
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            self.conversations.append(
                &id,
                ConversationRole::Assistant,
                format!(
                    "[Vault Registration]\n\n`{selector}` is ambiguous and matches {} catalog projects. No project was registered. Reissue the command with one exact full path or catalog id:\n\n{matches}",
                    selected.len()
                ),
            )?;
            return Ok(());
        }

        let candidate = selected[0];
        let record = self.registry.attach_candidate(&candidate.root)?;
        self.conversations.append(
            &id,
            ConversationRole::Assistant,
            format!(
                "[Vault Registration]\n\nRegistered exactly one Cortex project after an explicit exact-target command:\n\n{} → `{}`\nWorkspace id: `{}`\n\nNo source files were moved or deleted by registration.",
                record.name,
                record.root.display(),
                record.id
            ),
        )?;
        self.activity.append(
            ActivityKind::Workspace,
            "Cortex project registered",
            format!("{} → {}", record.name, record.root.display()),
            Some(true),
            json!({"workspace_id": record.id, "root": record.root, "explicit": true}),
        )?;
        self.refresh_overview()?;
        Ok(())
    }

    fn unregister_library_project_into_conversation(&mut self, prompt: &str) -> Result<(), String> {
        let id = self.prepare_user_message(prompt)?;
        let selector = explicit_library_unregistration_target(prompt).ok_or_else(|| {
            "Project unregistration requires an explicit command such as `unregister <exact project name>`, `unregister <full path>`, or `unregister <workspace id>`.".to_string()
        })?;
        let registered = self.registry.list()?;
        let mut matches = registered
            .iter()
            .filter(|workspace| registered_workspace_matches_selector(workspace, &selector))
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| left.root.cmp(&right.root));
        if matches.is_empty() {
            self.conversations.append(
                &id,
                ConversationRole::Assistant,
                format!(
                    "[Vault Unregister]\n\nNo registered Cortex project exactly matched `{selector}`. Nothing changed."
                ),
            )?;
            return Ok(());
        }
        if matches.len() > 1 {
            let rows = matches
                .iter()
                .map(|workspace| {
                    format!(
                        "- {} → `{}` (id `{}`)",
                        workspace.name,
                        workspace.root.display(),
                        workspace.id
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            self.conversations.append(
                &id,
                ConversationRole::Assistant,
                format!(
                    "[Vault Unregister]\n\n`{selector}` is ambiguous. Nothing changed. Use one exact full path or workspace id:\n\n{rows}"
                ),
            )?;
            return Ok(());
        }
        let workspace = matches[0];
        if workspace.root == self.workspace.root() {
            self.conversations.append(
                &id,
                ConversationRole::Assistant,
                format!(
                    "[Vault Unregister]\n\nRefused to unregister the active workspace `{}`. Switch Cortex to another project first. No registry, filesystem, Git, Vault, conversation, or state data was changed.",
                    workspace.root.display()
                ),
            )?;
            return Ok(());
        }
        let workspace_id = workspace.id.clone();
        let workspace_name = workspace.name.clone();
        let workspace_root = workspace.root.clone();
        let state_root = workspace.state_root.clone();
        if !self.registry.detach(&workspace_id)? {
            return Err(format!(
                "registered Cortex workspace disappeared before detach: {workspace_id}"
            ));
        }
        self.conversations.append(
            &id,
            ConversationRole::Assistant,
            format!(
                "[Vault Unregister]\n\nUnregistered `{workspace_name}` from the Cortex project registry only.\n\nProject: `{}`\nPreserved project state: `{}`\n\nCortex did not delete, move, rename, initialize Git in, alter remotes in, or modify project files. Existing per-project Cortex state and Vault/Git data remain on disk for safe reattachment/recovery.",
                workspace_root.display(),
                state_root.display()
            ),
        )?;
        self.activity.append(
            ActivityKind::Workspace,
            "Cortex project unregistered",
            format!("{} → {}", workspace_name, workspace_root.display()),
            Some(true),
            json!({
                "workspace_id": workspace_id,
                "root": workspace_root,
                "state_root_preserved": state_root,
                "filesystem_mutated": false
            }),
        )?;
        Ok(())
    }

    pub fn run_agent(&mut self, mode: &str, prompt: &str) -> Result<Value, String> {
        self.clear_pending_state()?;
        self.run_agent_internal(mode, prompt, true, None)
    }

    // O2D-R051N9M9A5_SAFE_AGENT_STREAM_TELEMETRY
    pub fn run_agent_stream(
        &mut self,
        mode: &str,
        prompt: &str,
        on_event: &mut dyn FnMut(CortexStreamEvent),
    ) -> Result<Value, String> {
        self.clear_pending_state()?;
        self.run_agent_internal(mode, prompt, true, Some(on_event))
    }
    // O2D-R051N9M10_EXECUTION_SPINE
    fn run_agent_internal(
        &mut self,
        mode: &str,
        prompt: &str,
        append_user_prompt: bool,
        on_event: Option<&mut dyn FnMut(CortexStreamEvent)>,
    ) -> Result<Value, String> {
        let conversation_id = self.ensure_conversation()?;
        let workspace_root = self.workspace.root().to_path_buf();
        let execution_id = begin_execution(&workspace_root, &conversation_id, mode, 8)?;
        transition_for_workspace(
            &workspace_root,
            &execution_id,
            ExecutionPhase::Inspecting,
            "Inspecting authoritative project state",
            "provider/tool activity",
        )?;

        let library_root = self
            .library_root()?
            .unwrap_or_else(|| workspace_root.clone());
        let _provider_lease = ProviderLeaseGuard::acquire(&library_root, &execution_id)?;
        bind_provider_scope_for_workspace(&workspace_root, &execution_id, &library_root)?;

        // M10D: every mutating execution owns a fresh durable transaction.
        // An active transaction left by an older execution is abandoned rather
        // than silently resurrected by H54 continuity.
        if matches!(mode, "apply" | "repair")
            && cortex_execution::spine::execution_scope_depth_for_workspace(
                &workspace_root,
                &execution_id,
            )? == 1
        {
            let client = self.ensure_client()?;
            let active_transaction = client
                .tool("source.transaction_status", json!({}))
                .ok()
                .and_then(|status| {
                    status
                        .pointer("/transaction/id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            if active_transaction.is_some() {
                client.tool("source.rollback", json!({}))?;
            }

            let started = client.tool(
                "source.begin_transaction",
                json!({"label": format!("m10-{}-{}", mode, execution_id)}),
            )?;
            let transaction_id = started
                .get("transaction_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    "M10 execution-owned source.begin_transaction returned no transaction_id"
                        .to_string()
                })?;
            claim_transaction_for_workspace(&workspace_root, transaction_id)?;
        }

        let result = self.run_agent_internal_m10_body(mode, prompt, append_user_prompt, on_event);
        match &result {
            Ok(_) => {
                let _ = complete_execution_for_workspace(&workspace_root, &execution_id);
            }
            Err(error) => {
                let _ = fail_execution_for_workspace(&workspace_root, &execution_id, error);
                if matches!(mode, "apply" | "repair") {
                    if let Ok(client) = self.ensure_client() {
                        let _ = client.tool("source.rollback", json!({}));
                    }
                }
            }
        }
        result
    }
    fn run_agent_internal_m10_body(
        &mut self,
        mode: &str,
        prompt: &str,
        append_user_prompt: bool,
        mut on_event: Option<&mut dyn FnMut(CortexStreamEvent)>,
    ) -> Result<Value, String> {
        let id = self.ensure_conversation()?;
        let current = self.conversations.load(&id)?;
        if current.title == "New conversation" {
            self.conversations
                .rename(&id, &conversation_title(prompt))?;
        }

        let mode_label =
            handoff_label(mode).ok_or_else(|| format!("unsupported Cortex agent mode: {mode}"))?;

        if append_user_prompt {
            self.conversations
                .append(&id, ConversationRole::User, prompt.to_string())?;
        }
        self.activity.append(
            ActivityKind::Agent,
            format!("{mode} request"),
            prompt,
            None,
            Value::Null,
        )?;

        let client = self.ensure_client()?;
        // H54: Desktop owns natural-language transaction continuity. A selected-project
        // conversation must never require the user to know or type --reuse-transaction.
        // If Cortex already has an active transaction for this project's workspace,
        // continue it intentionally; otherwise the RPC begins a new durable transaction.
        let reuse_active_transaction = if matches!(mode, "apply" | "repair") {
            client
                .tool("source.transaction_status", json!({}))
                .ok()
                .and_then(|status| {
                    status
                        .pointer("/transaction/id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
        } else {
            None
        };
        if let Some(transaction_id) = reuse_active_transaction.as_deref() {
            self.activity.append(
                ActivityKind::Agent,
                "Developer transaction recovery",
                format!(
                    "continuing active Cortex transaction {transaction_id} for the selected project"
                ),
                Some(true),
                json!({"transaction_id": transaction_id, "mode": mode}),
            )?;
        }

        // M11U2A: Repair starts from compiler truth, not an unconstrained model turn.
        // Run the deterministic project gate first, ground exact required packages,
        // and hand the first Repair turn the same compact evidence capsule used by
        // bounded continuation. Healthy projects keep the original prompt unchanged.
        let initial_repair_prompt = if mode == "repair" {
            Some(self.m11u2_prepare_initial_repair_prompt(&client, prompt)?)
        } else {
            None
        };
        let dispatch_prompt = initial_repair_prompt.as_deref().unwrap_or(prompt);

        let mut initial_result = match on_event.as_deref_mut() {
            Some(on_event) => match mode {
                "inspect" => client.inspect_stream(dispatch_prompt, on_event),
                "plan" => client.plan_stream(dispatch_prompt, on_event),
                // H57/M9A3: preserve durable transaction continuity on the streamed
                // Desktop path exactly as the existing non-streamed path does.
                "apply" => client.apply_reuse_transaction_stream(dispatch_prompt, on_event),
                "repair" => client.repair_reuse_transaction_stream(dispatch_prompt, on_event),
                _ => unreachable!(),
            },
            None => match mode {
                "inspect" => client.inspect(dispatch_prompt),
                "plan" => client.plan(dispatch_prompt),
                "apply" => client.apply_reuse_transaction(dispatch_prompt),
                "repair" => client.repair_reuse_transaction(dispatch_prompt),
                _ => unreachable!(),
            },
        };
        if let Err(error) = &initial_result {
            if let Some(retry_client) = self.recover_native_transport_reset(error)? {
                // A connection reset may happen after the RPC server already began the
                // transaction. Always use the reuse-capable mutation request on retry;
                // ensure_active_transaction will still create a transaction when none exists.
                initial_result =
                    match on_event {
                        Some(on_event) => match mode {
                            "inspect" => retry_client.inspect_stream(dispatch_prompt, on_event),
                            "plan" => retry_client.plan_stream(dispatch_prompt, on_event),
                            "apply" => retry_client
                                .apply_reuse_transaction_stream(dispatch_prompt, on_event),
                            "repair" => retry_client
                                .repair_reuse_transaction_stream(dispatch_prompt, on_event),
                            _ => unreachable!(),
                        },
                        None => match mode {
                            "inspect" => retry_client.inspect(dispatch_prompt),
                            "plan" => retry_client.plan(dispatch_prompt),
                            "apply" => retry_client.apply_reuse_transaction(dispatch_prompt),
                            "repair" => retry_client.repair_reuse_transaction(dispatch_prompt),
                            _ => unreachable!(),
                        },
                    };
            }
        }

        let result = match initial_result {
            Ok(value) if mode == "apply" => self.verify_apply_before_success(prompt, value),
            Ok(value) if mode == "repair" => self.verify_repair_before_success(prompt, value),
            other => other,
        };

        match &result {
            Ok(value) => {
                self.runtime_status = "online / active request".into();
                let text = if matches!(mode, "apply" | "repair")
                    && value
                        .get("quality_verified")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                {
                    verified_mutation_response_text(value)
                } else {
                    response_text(value)
                };
                let transcript_text = format!("[{mode_label}]\n\n{text}");
                self.conversations.append(
                    &id,
                    ConversationRole::Assistant,
                    transcript_text.clone(),
                )?;
                if matches!(mode, "apply" | "repair") {
                    let diff_appended = self.append_result_diff_bubbles(&id, value)?;
                    if !diff_appended {
                        self.append_result_file_bubbles(&id, value)?;
                    }
                }
                self.info = format!("CORTEX {}\n\n{}", mode.to_ascii_uppercase(), text);
                self.activity.append(
                    ActivityKind::Agent,
                    format!("{mode} agent"),
                    text,
                    Some(true),
                    value.clone(),
                )?;
            }
            Err(error) => {
                self.activity.append(
                    ActivityKind::Error,
                    format!("{mode} agent failed"),
                    error,
                    Some(false),
                    Value::Null,
                )?;
            }
        }
        result
    }

    fn append_result_diff_bubbles(
        &mut self,
        conversation_id: &str,
        result: &Value,
    ) -> Result<bool, String> {
        let verified = result
            .get("compile_verified")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let status = if verified { "Build Verified" } else { "Saved" };
        let mut appended = false;

        if let Ok(review) = ReviewSnapshot::collect(self.workspace.root()) {
            if !review.diff.trim().is_empty() {
                for (path, diff) in split_unified_diff_by_file(&review.diff).into_iter().take(8) {
                    self.append_diff_bubble(conversation_id, &path, status, &diff)?;
                    appended = true;
                }
                return Ok(appended);
            }
        }

        if let Some(files) = result.get("transaction_files") {
            if let Some(created) = files.get("created").and_then(Value::as_array) {
                for item in created.iter().take(8) {
                    let Some(path) = item.as_str() else { continue };
                    let resolved = self
                        .workspace
                        .resolve(path)
                        .map_err(|error| error.to_string())?;
                    if !resolved.is_file() {
                        continue;
                    }
                    let text = fs::read_to_string(&resolved).unwrap_or_default();
                    let line_count = text.lines().count().max(1);
                    let mut diff = format!(
                        "--- /dev/null\n+++ b/{}\n@@ -0,0 +1,{} @@\n",
                        path.replace('\\', "/"),
                        line_count
                    );
                    for line in text.lines().take(160) {
                        diff.push('+');
                        diff.push_str(line);
                        diff.push('\n');
                    }
                    self.append_diff_bubble(conversation_id, path, status, &diff)?;
                    appended = true;
                }
            }
        }
        Ok(appended)
    }

    fn append_diff_bubble(
        &mut self,
        conversation_id: &str,
        relative_path: &str,
        status: &str,
        diff: &str,
    ) -> Result<(), String> {
        let payload = json!({
            "path": relative_path.replace('\\', "/"),
            "status": status,
            "diff": diff,
        });
        self.conversations.append(
            conversation_id,
            ConversationRole::Tool,
            format!(
                "[DiffBubble]\n\n{}",
                serde_json::to_string(&payload).map_err(|error| error.to_string())?
            ),
        )?;
        Ok(())
    }

    fn append_result_file_bubbles(
        &mut self,
        conversation_id: &str,
        result: &Value,
    ) -> Result<(), String> {
        let Some(files) = result.get("transaction_files") else {
            return Ok(());
        };

        let verified = result
            .get("compile_verified")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let status = if verified { "Build Verified" } else { "Saved" };

        let mut seen = std::collections::BTreeSet::new();
        let mut ordered = Vec::new();

        for key in ["created", "touched"] {
            if let Some(items) = files.get(key).and_then(Value::as_array) {
                for item in items {
                    if let Some(path) = item.as_str() {
                        if seen.insert(path.to_string()) {
                            ordered.push(path.to_string());
                        }
                    }
                }
            }
        }

        for path in ordered.into_iter().take(24) {
            self.append_file_bubble(conversation_id, &path, status)?;
        }

        Ok(())
    }

    fn append_file_bubble(
        &mut self,
        conversation_id: &str,
        relative_path: &str,
        status: &str,
    ) -> Result<(), String> {
        let resolved = self
            .workspace
            .resolve(relative_path)
            .map_err(|error| error.to_string())?;
        if !resolved.is_file() {
            return Ok(());
        }

        let bytes = fs::read(&resolved).map_err(|error| {
            format!(
                "failed to read file bubble source {}: {error}",
                resolved.display()
            )
        })?;
        let max_bytes = 24 * 1024usize;
        let truncated = bytes.len() > max_bytes;
        let preview_bytes = &bytes[..bytes.len().min(max_bytes)];
        let preview = String::from_utf8_lossy(preview_bytes).to_string();

        let payload = json!({
            "path": relative_path.replace('\\', "/"),
            "language": file_language(relative_path),
            "status": status,
            "preview": preview,
            "truncated": truncated,
            "bytes": bytes.len(),
        });

        self.conversations.append(
            conversation_id,
            ConversationRole::Tool,
            format!(
                "[FileBubble]\n\n{}",
                serde_json::to_string(&payload).map_err(|error| error.to_string())?
            ),
        )?;
        Ok(())
    }

    fn m11u2_prepare_initial_repair_prompt(
        &mut self,
        client: &CortexClient,
        prompt: &str,
    ) -> Result<String, String> {
        let profile = client.tool("project.profile", json!({}))?;
        let gate = run_project_quality_gate(client, &profile)?;
        let success = gate
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        self.activity.append(
            ActivityKind::Build,
            "Initial Repair baseline",
            if success {
                "deterministic project quality gate is already green; preserving the original Repair request"
            } else {
                "deterministic project quality gate failed; first Repair turn will receive compiler evidence and exact dependency grounding"
            },
            Some(success),
            gate.clone(),
        )?;

        if success {
            return Ok(prompt.to_string());
        }

        let repair_signal = repair_signal_from_gate(&gate);
        let repair_strategy = choose_repair_strategy(&repair_signal);
        ensure_active_repair_transaction(client)?;
        let grounding_evidence = if repair_signal.missing_dependency_grounding.is_empty() {
            Value::Null
        } else {
            ground_repair_dependencies(client, &gate, &repair_signal.missing_dependency_grounding)?
        };
        let repair_targets = repair_targets_from_gate(&gate, self.workspace.root());
        let repair_target_evidence = controller_repair_target_evidence(client, &repair_targets);
        let strategy_instruction = match repair_strategy {
            RepairStrategy::Reconstruction => "RECONSTRUCTION is controller-selected. Rebuild the affected compact module coherently from the exact dependency/compiler evidence instead of patching individual API errors. Preserve the resolved dependency versions unless the compiler proves a declaration is wrong.",
            RepairStrategy::DependencyAdjustment => "DEPENDENCY ADJUSTMENT is controller-selected. Resolve exact declarations/source evidence first and do not guess API-sensitive source.",
            RepairStrategy::MultiFile => "MULTI-FILE repair is controller-selected. Keep the edit set bounded to compiler-implicated files and let the controller validate each mutation cycle.",
            RepairStrategy::Surgical => "SURGICAL repair is controller-selected. Make the smallest evidence-backed change that addresses the current diagnostics.",
        };

        Ok(format!(
            "M11U2 COMPILER-FIRST REPAIR. The controller already ran the deterministic baseline quality gate before this model turn. Treat the supplied compiler/dependency evidence as authoritative. The user already authorized the repair: execute tools now, do not ask whether to proceed, and do not claim success early. If the failure is formatter-only, apply the formatter-required source changes exactly rather than stopping. M11U2 ENFORCES ONE SOURCE MUTATION PER COMPILER CYCLE. After one coherent source mutation, the controller automatically formats/validates it and returns fresh diagnostics. M11U4 binds compiler diagnostics to controller-selected repair targets and pre-reads the primary target when possible. Do not perform repeated status/list/read/search loops or rediscover a target already supplied by the controller.

CONTROLLER REPAIR STRATEGY: {:?}
{}

GROUNDING SIGNAL:
{}

CONTROLLER-GROUNDED DEPENDENCY EVIDENCE:
{}

M11U4 CONTROLLER REPAIR TARGETS:
{}

M11U4 PRIMARY TARGET SOURCE:
{}

ORIGINAL USER REQUEST:
{}

PROJECT PROFILE:
{}

COMPILER REPAIR CAPSULE:
{}",
            repair_strategy,
            strategy_instruction,
            compact_json(
                &serde_json::to_value(&repair_signal).unwrap_or(Value::Null),
                4_000,
            ),
            compact_json(&grounding_evidence, 14_000),
            compact_json(&serde_json::to_value(&repair_targets).unwrap_or(Value::Null), 4_000),
            compact_json(&repair_target_evidence, 18_000),
            prompt,
            compact_json(&profile, 8_000),
            compact_json(&Self::m11u2_repair_gate_capsule(&gate), 10_000),
        ))
    }

    fn m11u2_repair_gate_capsule(gate: &Value) -> Value {
        let stages = gate
            .get("stages")
            .and_then(Value::as_array)
            .map(|stages| {
                stages
                    .iter()
                    .map(|stage| {
                        let result = stage.get("result").cloned().unwrap_or(Value::Null);
                        let diagnostics = result
                            .get("diagnostics")
                            .and_then(Value::as_array)
                            .map(|items| items.iter().take(12).cloned().collect::<Vec<_>>())
                            .unwrap_or_default();
                        json!({
                            "capability": stage.get("capability").cloned().unwrap_or(Value::Null),
                            "tool": stage.get("tool").cloned().unwrap_or(Value::Null),
                            "success": stage.get("success").cloned().unwrap_or(Value::Bool(false)),
                            "diagnostics": diagnostics,
                            "cortex_quality": result.get("cortex_quality").cloned().unwrap_or(Value::Null),
                            "cortex_candidate": result.get("cortex_candidate").cloned().unwrap_or(Value::Null),
                            "stderr": result
                                .get("stderr")
                                .and_then(Value::as_str)
                                .map(|text| {
                                    text.chars()
                                        .rev()
                                        .take(4_000)
                                        .collect::<String>()
                                        .chars()
                                        .rev()
                                        .collect::<String>()
                                })
                                .unwrap_or_default()
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        json!({
            "success": gate.get("success").cloned().unwrap_or(Value::Bool(false)),
            "stages": stages
        })
    }

    fn m11u2_quality_fingerprint(gate: &Value) -> Option<String> {
        let stages = gate.get("stages").and_then(Value::as_array)?;
        stages.iter().find_map(|stage| {
            stage
                .pointer("/result/cortex_candidate/quality_fingerprint")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    stage
                        .pointer("/result/cortex_quality/current/diagnostic_fingerprints")
                        .map(|value| compact_json(value, 2_000))
                })
        })
    }

    fn m11u2_prompt_requests_runtime(prompt: &str) -> bool {
        let lower = prompt.to_ascii_lowercase();
        [
            "launch",
            "run the actual",
            "open a working",
            "native window",
            "executable",
            "runtime verification",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    }

    fn m11u2_prompt_requests_window(prompt: &str) -> bool {
        let lower = prompt.to_ascii_lowercase();
        [
            "window",
            "windowed",
            "desktop app",
            "desktop application",
            "gui",
            "native app",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    }

    fn m11u2_prompt_requests_title_change(prompt: &str) -> bool {
        let lower = prompt.to_ascii_lowercase();
        lower.contains("title")
            && (lower.contains("update")
                || lower.contains("updating")
                || lower.contains("once per second"))
    }

    fn m11u2_runtime_acceptance(client: &CortexClient, prompt: &str) -> Result<Value, String> {
        let build = client.tool("build.project_build", json!({}))?;
        let build_success = build
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !build_success {
            return Err(format!(
                "M11U2 runtime acceptance could not start because the project build failed:\n{}",
                compact_json(&build, 8_000)
            ));
        }

        let require_window = Self::m11u2_prompt_requests_window(prompt);
        let require_title_change = Self::m11u2_prompt_requests_title_change(prompt);
        let runtime = client.tool(
            "runtime.verify_project",
            json!({
                "minimum_alive_ms": 2_000,
                "keep_running": true,
                "require_window": require_window,
                "require_title_change": require_title_change,
                "title_sample_interval_ms": 1_200
            }),
        )?;
        if !runtime
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(format!(
                "M11U2 runtime acceptance failed; no completion claim is allowed:\n{}",
                compact_json(&runtime, 8_000)
            ));
        }
        Ok(json!({"build": build, "runtime": runtime}))
    }

    fn verify_repair_before_success(
        &mut self,
        prompt: &str,
        agent_result: Value,
    ) -> Result<Value, String> {
        let client = self.ensure_client()?;
        let initial_transaction_files = client.tool("source.transaction_files", json!({}))?;
        let touched = initial_transaction_files
            .get("touched")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        let created = initial_transaction_files
            .get("created")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);

        if touched + created == 0 {
            return Err(
                "Cortex Repair finished without changing any project files. Prose or code snippets in chat are not accepted as a repair."
                    .into(),
            );
        }

        let profile = client.tool("project.profile", json!({}))?;
        let maximum_repairs = 2usize;
        let mut repairs = Vec::new();
        let mut final_gate = Value::Null;
        let mut previous_failure_fingerprint: Option<String> = None;

        for attempt in 0..=maximum_repairs {
            let gate = run_project_quality_gate(&client, &profile)?;
            let success = gate
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            final_gate = gate.clone();

            if success {
                self.activity.append(
                    ActivityKind::Build,
                    "Repair verification",
                    if attempt == 0 {
                        "project quality gate passed after source repair"
                    } else {
                        "project quality gate passed after bounded Repair continuation"
                    },
                    Some(true),
                    gate.clone(),
                )?;

                // Re-read transaction evidence because bounded repair continuation may
                // have touched additional project files after the first repair pass.
                let transaction_files = client.tool("source.transaction_files", json!({}))?;
                let intent_acceptance = project_intent_acceptance(self.workspace.root())?;
                let intent_success = intent_acceptance
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                self.activity.append(
                    if intent_success {
                        ActivityKind::Build
                    } else {
                        ActivityKind::Error
                    },
                    "Project intent acceptance",
                    if intent_success {
                        "implemented project matches the saved technology/application intent contract"
                    } else {
                        "quality gate passed, but the implemented project does not match the saved intent contract"
                    },
                    Some(intent_success),
                    intent_acceptance.clone(),
                )?;
                if !intent_success {
                    return Err(format!(
                        "Cortex produced a buildable project that does not satisfy the requested project intent. Completion is denied.\n\n{}",
                        compact_json(&intent_acceptance, 10_000)
                    ));
                }
                let runtime_acceptance = if Self::m11u2_prompt_requests_runtime(prompt) {
                    Some(Self::m11u2_runtime_acceptance(&client, prompt)?)
                } else {
                    None
                };
                let runtime_verified = runtime_acceptance.is_some();
                let transaction_commit = client.tool("source.commit", json!({}))?;
                return Ok(json!({
                    "agent": agent_result,
                    "repairs": repairs,
                    "transaction_files": transaction_files,
                    "transaction_commit": transaction_commit,
                    "project_profile": profile,
                    "verification": gate,
                    "intent_acceptance": intent_acceptance,
                    "runtime_acceptance": runtime_acceptance,
                    "compile_verified": true,
                    "quality_verified": true,
                    "runtime_verified": runtime_verified,
                    "repair_attempts": attempt
                }));
            }

            let failure_fingerprint = Self::m11u2_quality_fingerprint(&gate);
            if attempt > 0
                && failure_fingerprint.is_some()
                && failure_fingerprint == previous_failure_fingerprint
            {
                self.activity.append(
                    ActivityKind::Error,
                    "Repair diagnostic convergence guard",
                    "compiler diagnostics did not change after the previous bounded repair; refusing another equivalent model continuation",
                    Some(false),
                    gate.clone(),
                )?;
                break;
            }
            previous_failure_fingerprint = failure_fingerprint;

            if attempt == maximum_repairs {
                break;
            }

            self.activity.append(
                ActivityKind::Build,
                "Repair verification",
                format!(
                    "project quality gate failed; continuing bounded Repair attempt {} of {}",
                    attempt + 1,
                    maximum_repairs
                ),
                Some(false),
                gate.clone(),
            )?;

            let _ = client.tool("development.run_advance", json!({}));
            let repair_signal = repair_signal_from_gate(&gate);
            let repair_strategy = choose_repair_strategy(&repair_signal);
            ensure_active_repair_transaction(&client)?;
            let grounding_evidence = if repair_signal.missing_dependency_grounding.is_empty() {
                Value::Null
            } else {
                ground_repair_dependencies(
                    &client,
                    &gate,
                    &repair_signal.missing_dependency_grounding,
                )?
            };
            let repair_targets = repair_targets_from_gate(&gate, self.workspace.root());
            let repair_target_evidence =
                controller_repair_target_evidence(&client, &repair_targets);
            let strategy_instruction = match repair_strategy {
                RepairStrategy::Reconstruction => "RECONSTRUCTION is controller-selected. Re-read the affected compact module and rebuild it coherently from the exact dependency/compiler evidence instead of patching individual errors. Preserve the currently resolved dependency versions unless the compiler proves a dependency declaration itself is wrong. Do not reuse APIs that are absent from the grounded evidence.",
                RepairStrategy::DependencyAdjustment => "DEPENDENCY ADJUSTMENT is controller-selected. Resolve exact dependency declarations/source evidence first and do not mutate API-sensitive source until grounding is complete.",
                RepairStrategy::MultiFile => "MULTI-FILE repair is controller-selected. Keep the edit set bounded to the files implicated by the diagnostics and verify immediately afterward.",
                RepairStrategy::Surgical => "SURGICAL repair is controller-selected. Make the smallest evidence-backed change that resolves the current diagnostics and verify immediately afterward.",
            };
            let repair_prompt = format!(
                "The selected-project repair still fails its deterministic quality gate. Continue under the controller-selected strategy below and fix ONLY the concrete failures in the repair capsule. The user already authorized this repair: do not ask whether to proceed and do not stop on a future-tense promise. Execute project tools now. Do not broaden scope or claim success early. If the failure is formatter-only, apply the formatter-required source changes exactly rather than stopping. M11U2 ENFORCES ONE SOURCE MUTATION PER COMPILER CYCLE: make one coherent evidence-backed mutation, then stop editing and consume the controller's automatic format/validation result before any further source change. M11U4 binds compiler diagnostics to controller-selected repair targets. The controller has already read the primary target when possible; do not spend the bounded Repair budget rediscovering it with broad source.search/status loops.\n\nCONTROLLER REPAIR STRATEGY: {:?}\n{}\n\nGROUNDING SIGNAL:\n{}\n\nCONTROLLER-GROUNDED DEPENDENCY EVIDENCE:\n{}\n\nM11U4 CONTROLLER REPAIR TARGETS:\n{}\n\nM11U4 PRIMARY TARGET SOURCE:\n{}\n\nORIGINAL USER REQUEST:\n{prompt}\n\nPROJECT PROFILE:\n{}\n\nCOMPILER REPAIR CAPSULE:\n{}",
                repair_strategy,
                strategy_instruction,
                compact_json(&serde_json::to_value(&repair_signal).unwrap_or(Value::Null), 4_000),
                compact_json(&grounding_evidence, 14_000),
                compact_json(&serde_json::to_value(&repair_targets).unwrap_or(Value::Null), 4_000),
                compact_json(&repair_target_evidence, 18_000),
                compact_json(&profile, 8_000),
                compact_json(&Self::m11u2_repair_gate_capsule(&gate), 10_000)
            );
            let repair = client.repair_reuse_transaction(&repair_prompt)?;
            repairs.push(repair);
            let _ = client.tool("development.run_advance", json!({}));
        }

        self.activity.append(
            ActivityKind::Error,
            "Repair verification failed",
            format!(
                "bounded Repair continuation budget exhausted after {maximum_repairs} additional attempt(s)"
            ),
            Some(false),
            final_gate.clone(),
        )?;

        Err(format!(
            "Cortex changed project files during Repair, but the detected project quality gate is still failing after {maximum_repairs} bounded continuation attempts. The repair is NOT verified.\n\n{}",
            compact_json(&final_gate, 14_000)
        ))
    }

    fn verify_apply_before_success(
        &mut self,
        prompt: &str,
        agent_result: Value,
    ) -> Result<Value, String> {
        let client = self.ensure_client()?;
        let transaction_files = client.tool("source.transaction_files", json!({}))?;
        let touched = transaction_files
            .get("touched")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        let created = transaction_files
            .get("created")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);

        if touched + created == 0 {
            return Err(
                "Cortex Code finished without changing any project files. No implementation will be reported as successful."
                    .into(),
            );
        }

        let profile = client.tool("project.profile", json!({}))?;
        let mut repairs = Vec::new();
        let maximum_repairs = 3usize;
        let mut final_gate = Value::Null;

        for attempt in 0..=maximum_repairs {
            let gate = run_project_quality_gate(&client, &profile)?;
            let success = gate
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            final_gate = gate.clone();
            if success {
                self.activity.append(
                    ActivityKind::Build,
                    "Code verification",
                    if attempt == 0 {
                        "project quality gate passed"
                    } else {
                        "project quality gate passed after bounded automatic repair"
                    },
                    Some(true),
                    gate.clone(),
                )?;
                // H54: commit the durable source transaction only after the
                // complete detected quality gate succeeds. A later project request
                // therefore starts cleanly, while failed work remains resumable.
                let intent_acceptance = project_intent_acceptance(self.workspace.root())?;
                let intent_success = intent_acceptance
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                self.activity.append(
                    if intent_success {
                        ActivityKind::Build
                    } else {
                        ActivityKind::Error
                    },
                    "Project intent acceptance",
                    if intent_success {
                        "implemented project matches the saved technology/application intent contract"
                    } else {
                        "quality gate passed, but the implemented project does not match the saved intent contract"
                    },
                    Some(intent_success),
                    intent_acceptance.clone(),
                )?;
                if !intent_success {
                    return Err(format!(
                        "Cortex produced a buildable project that does not satisfy the requested project intent. Completion is denied.\n\n{}",
                        compact_json(&intent_acceptance, 10_000)
                    ));
                }
                let runtime_acceptance = if Self::m11u2_prompt_requests_runtime(prompt) {
                    Some(Self::m11u2_runtime_acceptance(&client, prompt)?)
                } else {
                    None
                };
                let runtime_verified = runtime_acceptance.is_some();
                let transaction_commit = client.tool("source.commit", json!({}))?;
                return Ok(json!({
                    "agent": agent_result,
                    "repairs": repairs,
                    "transaction_files": transaction_files,
                    "transaction_commit": transaction_commit,
                    "project_profile": profile,
                    "verification": gate,
                    "intent_acceptance": intent_acceptance,
                    "runtime_acceptance": runtime_acceptance,
                    "compile_verified": true,
                    "quality_verified": true,
                    "runtime_verified": runtime_verified,
                    "repair_attempts": attempt
                }));
            }

            if attempt == maximum_repairs {
                break;
            }

            self.activity.append(
                ActivityKind::Build,
                "Code verification",
                format!(
                    "project quality gate failed; invoking bounded Repair attempt {} of {}",
                    attempt + 1,
                    maximum_repairs
                ),
                Some(false),
                gate.clone(),
            )?;

            let _ = client.tool("development.run_advance", json!({}));
            let repair_prompt = format!(
                "The approved implementation below failed its detected project quality gate. Reuse the active source transaction and repair ONLY errors required to satisfy the requested milestone. Do not broaden scope. Re-run the applicable project quality checks before finishing and do not claim success unless they are green.\n\nORIGINAL REQUEST:\n{prompt}\n\nPROJECT PROFILE:\n{}\n\nQUALITY GATE RESULT:\n{}",
                compact_json(&profile, 8_000),
                compact_json(&gate, 18_000)
            );
            let repair = client.repair_reuse_transaction(&repair_prompt)?;
            repairs.push(repair);
            let _ = client.tool("development.run_advance", json!({}));
        }

        self.activity.append(
            ActivityKind::Error,
            "Code verification failed",
            "bounded automatic Repair budget was exhausted while the project quality gate was still failing",
            Some(false),
            final_gate.clone(),
        )?;
        Err(format!(
            "Cortex changed the source but the detected project quality gate is still failing after {maximum_repairs} bounded repair attempts. The implementation is NOT verified.\n\n{}",
            compact_json(&final_gate, 14_000)
        ))
    }

    fn prepare_user_message(&mut self, prompt: &str) -> Result<String, String> {
        let id = self.ensure_conversation()?;
        let current = self.conversations.load(&id)?;
        if current.title == "New conversation" {
            self.conversations
                .rename(&id, &conversation_title(prompt))?;
        }
        self.conversations
            .append(&id, ConversationRole::User, prompt.to_string())?;
        self.activity.append(
            ActivityKind::Chat,
            "User message",
            prompt,
            None,
            Value::Null,
        )?;
        Ok(id)
    }

    fn chat_trace(&self, conversation_id: &str) -> TraceContext {
        let mut trace = TraceContext::new(new_trace_id("desktop-chat"));
        trace.project_id = Some(self.project_id.clone());
        trace.session_id = Some(format!("desktop-pid-{}", std::process::id()));
        trace.conversation_id = Some(conversation_id.to_string());
        trace
    }

    fn observe(
        &self,
        kind: ProjectEventKind,
        severity: EventSeverity,
        summary: &str,
        trace: TraceContext,
        attributes: Value,
    ) {
        let mut event = ProjectEvent::new(
            new_event_id("desktop"),
            kind,
            "cortex_desktop",
            summary,
            trace,
        );
        event.severity = severity;
        if let Value::Object(values) = attributes {
            event.attributes.extend(values);
        }
        let _ = self.observability.record(&event);
    }

    fn offer_standalone_project_creation(
        &mut self,
        conversation_id: &str,
        prompt: &str,
    ) -> Result<(), String> {
        let project_name = standalone_project_name(prompt);
        let intent = infer_project_intent(prompt);
        let target = if let Ok(Some(layout)) = self.registry.library_layout() {
            available_project_target(&layout.projects, &project_name)?
        } else {
            available_sibling_project_target(self.workspace.root(), &project_name)?
        };

        let message = format!(
            "I can create this as a completely separate project without modifying `{}`.\n\n\
Project: `{}`\n\
Location: `{}`\n\
Detected language: {}\n\
Detected build system: {}\n\
Target platform: {}\n\
Application type: {}\n\
Scaffold: {}\n\
Runtime proof required: {}\n\n\
If you reply **Yes**, Cortex will create and register that project, switch this conversation to it, \
hand the original request to the generic development workflow, require actual source changes, \
run the detected project quality gate, enforce the saved project-intent contract, and only report \
success when both technical verification and requested-intent acceptance are green.\n\n\
Reply **No** to leave the filesystem unchanged.",
            self.workspace.profile().name,
            project_name,
            target.display(),
            intent_value(&intent.language),
            intent_value(&intent.build_system),
            intent_value(&intent.platform),
            intent_value(&intent.application_kind),
            project_scaffold_label(&intent),
            if intent.runtime_required { "yes" } else { "no" },
        );

        self.conversations
            .append(conversation_id, ConversationRole::Assistant, message)?;
        self.pending_project_creation = Some(PendingProjectCreation {
            conversation_id: conversation_id.to_string(),
            original_prompt: prompt.to_string(),
            project_name,
            target,
            intent,
        });
        self.persist_background_state()?;
        self.runtime_status = "awaiting approval / create standalone project".into();
        Ok(())
    }

    fn handle_pending_project_creation_response(&mut self, response: &str) -> Result<bool, String> {
        self.sync_pending_state()?;
        let Some(pending) = self.pending_project_creation.clone() else {
            return Ok(false);
        };

        let active_matches = self
            .active_conversation
            .as_deref()
            .is_some_and(|id| id == pending.conversation_id);
        if !active_matches {
            self.pending_project_creation = None;
            self.persist_background_state()?;
            return Ok(false);
        }

        if is_handoff_no(response) {
            self.conversations.append(
                &pending.conversation_id,
                ConversationRole::User,
                response.to_string(),
            )?;
            self.conversations.append(
                &pending.conversation_id,
                ConversationRole::Assistant,
                "Okay — the separate project was not created.".to_string(),
            )?;
            self.pending_project_creation = None;
            self.persist_background_state()?;
            self.runtime_status = "online / developer".into();
            return Ok(true);
        }

        if !is_handoff_yes(response) {
            self.pending_project_creation = None;
            self.persist_background_state()?;
            return Ok(false);
        }

        self.conversations.append(
            &pending.conversation_id,
            ConversationRole::User,
            response.to_string(),
        )?;

        self.pending_project_creation = None;
        self.persist_background_state()?;

        let intent = normalized_project_intent(&pending.intent, &pending.original_prompt);
        create_standalone_project(&pending.target, &pending.project_name, &intent)?;

        let created_workspace =
            Workspace::open(&pending.target).map_err(|error| error.to_string())?;
        self.registry.attach(&created_workspace)?;

        let mut replacement = Self::open(&pending.target)?;
        let target_title = conversation_title(&pending.original_prompt);
        let transferred = replacement.conversations.create(&target_title)?;
        replacement.conversations.append(
            &transferred.id,
            ConversationRole::User,
            pending.original_prompt.clone(),
        )?;

        replacement.conversations.append(
            &transferred.id,
            ConversationRole::Assistant,
            format!(
                "[Developer]\n\nCreated and registered standalone project `{}` at `{}` using the `{}` scaffold. \
Cortex is now attached to the NEW project; the previous workspace remains untouched. \
The saved project-intent contract is authoritative for language/build/runtime acceptance, so a green compiler alone cannot satisfy the request if the resulting project is the wrong technology or application type.",
                pending.project_name,
                pending.target.display(),
                project_scaffold_label(&intent),
            ),
        )?;
        replacement.active_conversation = Some(transferred.id);
        replacement.runtime_status = "developer / implementing new standalone project".into();

        *self = replacement;

        let discovery = discover_roadmap(self.workspace.root())?;
        let roadmap = discovery
            .roadmap
            .unwrap_or_else(|| proposed_roadmap(&pending.project_name));
        let development_store = DevelopmentStore::open(&self.workspace.cortex_state_dir())?;
        let development_run = development_store.begin(self.workspace.root(), &roadmap)?;
        self.activity.append(
            ActivityKind::Job,
            "Milestone development started",
            format!(
                "{} — {} / stage {:?}",
                development_run.milestone_id,
                development_run.milestone_title,
                development_run.stage
            ),
            Some(true),
            json!({"development_run_id": development_run.id}),
        )?;

        let intent_json =
            serde_json::to_string_pretty(&intent).unwrap_or_else(|_| "{}".to_string());
        let implementation_prompt = format!(
            "You are now attached to a newly-created standalone project at `{}`. \
Implement the user's ORIGINAL request in THIS new project only. \
Do not modify or reference the previous workspace. \
The PROJECT INTENT CONTRACT below is authoritative. Do not silently change the requested language, build system, platform, or application type just because another scaffold is easier. \
Use the smallest practical dependency set and current compileable APIs. \
You MUST materially edit project source and any required project manifest/build files. \
Do not merely paste sample code into chat. A prose/code-snippet-only answer is a failed implementation. \
Keep the final chat response concise: changed files, quality/runtime evidence, and blockers; do not dump large source listings. \
Treat the active Cortex development run as durable milestone state, not as a special-case demo. \
Finish only after the implementation exists in project files, the applicable quality gate is green, the project-intent contract passes, and requested runtime evidence passes.\n\n\
PROJECT INTENT CONTRACT:\n{}\n\n\
ORIGINAL USER REQUEST:\n{}",
            self.workspace.root().display(),
            intent_json,
            pending.original_prompt
        );

        let result = match self.run_agent_internal("apply", &implementation_prompt, false, None) {
            Ok(result) => result,
            Err(error) => {
                let id = self.ensure_conversation()?;
                self.conversations.append(
                    &id,
                    ConversationRole::Assistant,
                    format!(
                        "[Developer]\n\nThe standalone project was created and remains attached, but the first Code pass could not complete. Nothing was rolled back or hidden.\n\nError\n-----\n{error}\n\nYou can correct provider/configuration issues and reply `continue` to retry from this project."
                    ),
                )?;
                self.runtime_status = "developer / blocked — retry available".into();
                return Ok(true);
            }
        };

        if result
            .get("quality_verified")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let runtime_verified = result
                .get("runtime_verified")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            self.runtime_status = if runtime_verified {
                "online / intent verified / runtime verified".into()
            } else {
                "online / intent verified / quality verified".into()
            };
        }

        Ok(true)
    }

    fn project_chat_prompt(&self, prompt: &str) -> String {
        let recent = self.recent_developer_context(4_000);
        let context_summary = ContextStore::new(self.workspace.clone())
            .load_index()
            .ok()
            .flatten()
            .map(|index| {
                format!(
                    "{} indexed files; languages={}",
                    index.files.len(),
                    serde_json::to_string(&index.language_counts).unwrap_or_else(|_| "{}".into())
                )
            })
            .unwrap_or_else(|| "persistent project index not built yet".into());
        format!(
            "You are Cortex attached to the current project.\n\
PROJECT NAME: {}\n\
PROJECT ROOT: {}\n\
PROJECT INDEX: {}\n\n\
The user is speaking to you as the developer/collaborator assigned to this project. \
Keep the project identity in mind even during conversational turns. \
Broad brainstorming, feature discussion, and product-direction questions are conversational: answer directly from the supplied project/index context and recent conversation rather than forcing an agent/tool loop. \
Do not claim to have inspected or changed source during this chat-only turn unless that evidence appears below. \
Cortex Desktop has real project read/write tools in its developer execution path. Never tell the user that Cortex cannot edit/save project files and never instruct the user to make project edits manually as a substitute for Cortex execution. \
Do not tell the user to pick Inspect/Plan/Code/Repair modes; the Desktop routes explicit project work automatically.\n\n\
RECENT CONVERSATION CONTEXT:\n{}\n\n\
DURABLE DEVELOPMENT STATE:\n{}\n\n\
USER MESSAGE:\n{}",
            self.workspace.profile().name,
            self.workspace.root().display(),
            context_summary,
            recent,
            self.development_context_summary(),
            prompt
        )
    }

    fn recent_developer_context(&self, max_chars: usize) -> String {
        let Some(id) = self.active_conversation.as_deref() else {
            return String::new();
        };
        let Ok(conversation) = self.conversations.load(id) else {
            return String::new();
        };
        tail_chars(&format_conversation(conversation), max_chars)
    }

    fn active_development_run(&self) -> Option<cortex_development::DevelopmentRun> {
        DevelopmentStore::open(&self.workspace.cortex_state_dir())
            .ok()
            .and_then(|store| store.active().ok().flatten())
    }

    fn development_context_summary(&self) -> String {
        let Ok(store) = DevelopmentStore::open(&self.workspace.cortex_state_dir()) else {
            return "No durable development run is active.".into();
        };
        let run = store.active().ok().flatten();
        let roadmap = store.roadmap().ok().flatten();
        match (run, roadmap) {
            (Some(run), Some(roadmap)) => {
                let completed = roadmap
                    .milestones
                    .iter()
                    .filter(|milestone| {
                        milestone.status == cortex_development::MilestoneStatus::Complete
                    })
                    .count();
                format!(
                    "Run {} | milestone {} — {} | stage {:?} | repair {}/{} | completed milestones {}/{} | next: {} | last error: {}",
                    run.id,
                    run.milestone_id,
                    run.milestone_title,
                    run.stage,
                    run.repair_budget.used,
                    run.repair_budget.maximum,
                    completed,
                    roadmap.milestones.len(),
                    run.next_action.as_deref().unwrap_or("—"),
                    run.last_error.as_deref().unwrap_or("—")
                )
            }
            (Some(run), None) => format!(
                "Run {} | milestone {} — {} | stage {:?} | repair {}/{} | next: {}",
                run.id,
                run.milestone_id,
                run.milestone_title,
                run.stage,
                run.repair_budget.used,
                run.repair_budget.maximum,
                run.next_action.as_deref().unwrap_or("—")
            ),
            (None, Some(roadmap)) => format!(
                "Roadmap loaded with {} milestone(s); no durable run is active.",
                roadmap.milestones.len()
            ),
            (None, None) => "No durable development run is active.".into(),
        }
    }

    fn routed_developer_intent(&self, prompt: &str) -> Option<DeveloperIntent> {
        // Every Cortex conversation is attached to one authoritative workspace. Natural
        // implementation/repair follow-ups therefore inherit project context even when
        // the immediately preceding turn was ordinary chat rather than an explicit
        // developer-mode turn. Brainstorming and explicit `chat:` requests are still
        // filtered by `developer_followup_intent`.
        route_developer_intent(prompt, true)
    }

    fn run_developer_workflow(
        &mut self,
        prompt: &str,
        intent: DeveloperIntent,
    ) -> Result<(), String> {
        if intent == DeveloperIntent::NewProject {
            let id = self.prepare_user_message(prompt)?;
            self.offer_standalone_project_creation(&id, prompt)?;
            return Ok(());
        }

        let id = self.prepare_user_message(prompt)?;
        let recent = self.recent_developer_context(2_400);

        if intent == DeveloperIntent::ReadOnly {
            self.runtime_status = "developer / inspecting project".into();
            let developer_prompt = format!(
                "Act as the developer responsible for the selected project at `{}`. \
The user is the designer/product owner. Inspect only the evidence needed for this request from current source, project context, latest build/checkpoint state, and recent conversation. \
Use project tools when they materially improve accuracy, but do not mutate files on this read-only turn. \
Do not claim that Cortex lacks file access; this desktop is attached to the project and read-only source tools are available. \
Answer naturally and directly without exposing internal Inspect/Plan/Code/Repair mode names unless the user explicitly asks about them.\n\n\
USER REQUEST:\n{prompt}\n\n\
RECENT PROJECT CONTEXT:\n{recent}",
                self.workspace.root().display()
            );
            let client = self.ensure_client()?;
            let inspected = client.inspect(&developer_prompt)?;
            let text = response_text(&inspected);
            if text.trim().is_empty() {
                return Err(
                    "Cortex project inspection completed without user-visible text.".into(),
                );
            }
            self.conversations.append(
                &id,
                ConversationRole::Assistant,
                format!("[Developer]\n\n{text}"),
            )?;
            self.activity.append(
                ActivityKind::Agent,
                "Developer project inspection",
                &text,
                Some(true),
                inspected,
            )?;
            self.runtime_status = "online / developer".into();
            return Ok(());
        }

        // Explicit implementation/repair language in an attached project chat is the
        // authorization for a bounded project transaction. Do not insert a second
        // approval turn or a tool-less advisory response in front of execution.
        self.clear_pending_state()?;
        let (mode, task_contract) = match intent {
            DeveloperIntent::Repair => (
                "repair",
                "Diagnose the reported failure from authoritative source/build evidence, mutate only the files needed to repair it, and verify the project quality gate before finishing.",
            ),
            DeveloperIntent::ContinueAndCode | DeveloperIntent::Code => (
                "apply",
                "Inspect enough of the selected project to identify the bounded implementation, make the requested project-file changes, and verify the project quality gate before finishing.",
            ),
            DeveloperIntent::ReadOnly | DeveloperIntent::NewProject => unreachable!(),
        };
        self.runtime_status = format!("developer / {mode} executing");

        let execution_prompt = format!(
            "You are Cortex operating inside the authoritative selected project root `{}`. \
You HAVE project source inspection, project-file mutation, transaction, build, test, and diagnostic tools in this mode. \
Never tell the user to make edits manually and never state that you cannot edit/save project files. \
{} \
Use a durable Cortex source transaction for mutation. Keep scope bounded to the user's request. \
The user's explicit implement/fix/build/finish request is already authorization for this bounded project mutation: DO NOT ask whether to proceed, do not request a second confirmation, and do not promise future tool use instead of executing it now. \
A prose answer, code snippet, suggested patch, future-tense promise, or approval question without successful project-file mutation is a failed execution. \
Do not paste large replacement files into chat; finish with concise changed-file and verification evidence.\n\n\
USER REQUEST:\n{prompt}\n\n\
RECENT PROJECT CONTEXT:\n{recent}",
            self.workspace.root().display(),
            task_contract,
        );

        let first = self.run_agent_internal(mode, &execution_prompt, false, None);
        let result = match first {
            Err(error) if mutation_execution_retry_required(&error) => {
                self.activity.append(
                    ActivityKind::Agent,
                    "Developer mutation retry",
                    "The first project-agent pass returned without a successful file mutation; retrying once with a mandatory tool-first contract.",
                    Some(false),
                    json!({"mode": mode, "error": error}),
                )?;
                let retry_prompt = format!(
                    "MANDATORY TOOL-FIRST RETRY for the selected project `{}`. \
The previous attempt did not mutate project files. Do not answer with limitations, instructions for the user, pseudo-code, or a proposed patch. \
Begin by using authoritative project/source tools, then perform the bounded requested mutation with Cortex source tools in the active transaction and run the detected quality gate. \
You have the required project-file tools in this mode.\n\nUSER REQUEST:\n{prompt}",
                    self.workspace.root().display(),
                );
                self.run_agent_internal(mode, &retry_prompt, false, None)
            }
            other => other,
        };

        match result {
            Ok(_) => {
                if prompt_requests_launch(prompt) {
                    self.launch_verified_project_into_conversation(&id)?;
                }
                self.runtime_status = "online / developer".into();
                Ok(())
            }
            Err(error) => {
                self.conversations.append(
                    &id,
                    ConversationRole::Assistant,
                    format!(
                        "[Developer]\n\nCortex could not complete the requested project mutation. No success is being claimed.\n\nError\n-----\n{error}\n\nThe selected project and conversation remain intact; retrying after resolving the reported provider/tool/build blocker is safe."
                    ),
                )?;
                self.runtime_status = "developer / blocked — retry available".into();
                Ok(())
            }
        }
    }

    fn handle_pending_handoff_response(&mut self, response: &str) -> Result<bool, String> {
        self.sync_pending_state()?;
        if self.handle_pending_project_creation_response(response)? {
            return Ok(true);
        }

        let Some(pending) = self.pending_handoff.clone() else {
            return Ok(false);
        };

        let active_matches = self
            .active_conversation
            .as_deref()
            .is_some_and(|id| id == pending.conversation_id);
        if !active_matches {
            self.clear_pending_state()?;
            return Ok(false);
        }

        if is_handoff_yes(response) {
            self.conversations.append(
                &pending.conversation_id,
                ConversationRole::User,
                response.to_string(),
            )?;
            self.clear_pending_state()?;
            let execution_mode = match pending.mode {
                HandoffMode::Code => "apply",
                HandoffMode::Repair => "repair",
            };
            if let Err(error) =
                self.run_agent_internal(execution_mode, &pending.execution_prompt, false, None)
            {
                self.conversations.append(
                    &pending.conversation_id,
                    ConversationRole::Assistant,
                    format!(
                        "[Developer]\n\nThe approved {execution_mode} step could not complete. The conversation and project remain intact.\n\nError\n-----\n{error}\n\nFix the provider/configuration issue if needed, then reply `continue` to retry."
                    ),
                )?;
                self.runtime_status = "developer / blocked — retry available".into();
            }
            return Ok(true);
        }

        if is_handoff_no(response) {
            self.conversations.append(
                &pending.conversation_id,
                ConversationRole::User,
                response.to_string(),
            )?;
            self.conversations.append(
                &pending.conversation_id,
                ConversationRole::Assistant,
                "Okay — stopping before the source-changing step. The inspection and recommendation remain in this conversation."
                    .to_string(),
            )?;
            self.clear_pending_state()?;
            self.runtime_status = "online / chat".into();
            return Ok(true);
        }

        // A new non-confirmation message replaces the pending decision.
        self.clear_pending_state()?;
        Ok(false)
    }

    pub fn browse_files(
        &self,
        library_scope: bool,
        relative_directory: &str,
    ) -> Result<DesktopFileListing, String> {
        let root = if library_scope {
            self.library_root()?
                .ok_or_else(|| "Cortex Vault root is not configured".to_string())?
        } else {
            self.workspace.root().to_path_buf()
        };
        let root = fs::canonicalize(&root).map_err(|error| error.to_string())?;
        let requested = if relative_directory.trim().is_empty() {
            root.clone()
        } else {
            root.join(relative_directory)
        };
        let current = fs::canonicalize(&requested)
            .map_err(|error| format!("could not browse {}: {error}", requested.display()))?;
        if !current.starts_with(&root) || !current.is_dir() {
            return Err("file browser path escaped its selected Cortex scope".into());
        }

        let mut entries = fs::read_dir(&current)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if library_scope && name.eq_ignore_ascii_case(".cortex") {
                    return None;
                }
                let metadata = entry.metadata().ok()?;
                let relative_path = path
                    .strip_prefix(&root)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/");
                Some(DesktopFileEntry {
                    name,
                    relative_path,
                    is_directory: metadata.is_dir(),
                    bytes: if metadata.is_file() {
                        metadata.len()
                    } else {
                        0
                    },
                })
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right.is_directory.cmp(&left.is_directory).then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
        });
        entries.truncate(500);

        Ok(DesktopFileListing {
            scope: if library_scope { "Vault" } else { "Project" }.into(),
            root: root.display().to_string(),
            current: current
                .strip_prefix(&root)
                .unwrap_or(Path::new(""))
                .to_string_lossy()
                .replace('\\', "/"),
            entries,
        })
    }

    pub fn open_workbench_file(
        &mut self,
        relative_path: &str,
    ) -> Result<DesktopWorkbenchDocument, String> {
        let resolved = self
            .workspace
            .resolve(relative_path)
            .map_err(|error| error.to_string())?;
        if !resolved.is_file() {
            return Err(format!("Workbench path is not a file: {relative_path}"));
        }

        let bytes = fs::read(&resolved)
            .map_err(|error| format!("failed to read {}: {error}", resolved.display()))?;
        let max_bytes = 2 * 1024 * 1024usize;
        let truncated = bytes.len() > max_bytes;
        let content = String::from_utf8_lossy(&bytes[..bytes.len().min(max_bytes)]).to_string();

        Ok(DesktopWorkbenchDocument {
            path: relative_path.replace('\\', "/"),
            language: file_language(relative_path).to_string(),
            content,
            bytes: bytes.len() as u64,
            truncated,
        })
    }

    pub fn show_files(&mut self, query: &str) -> Result<(), String> {
        let query = query.trim();
        if !query.is_empty() {
            if let Ok(path) = self.workspace.resolve(query) {
                if path.is_file() {
                    let id = self.ensure_conversation()?;
                    self.append_file_bubble(&id, query, "Opened")?;
                    self.info =
                        format!("FILES\n\nOpened `{query}` as an inline conversation file bubble.");
                    return Ok(());
                }
            }
        }

        let files = self
            .workspace
            .list_files("", 250)
            .map_err(|error| error.to_string())?;
        let filter = query.to_ascii_lowercase();
        let filtered = files
            .into_iter()
            .filter(|path| {
                query.is_empty()
                    || path
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .contains(&filter)
            })
            .collect::<Vec<_>>();

        let mut output = format!(
            "FILES\n\nWorkspace: {}\nFilter: {}\nShowing: {} item(s) (bounded to 250)\n\n",
            self.workspace.profile().name,
            if query.is_empty() { "none" } else { query },
            filtered.len()
        );
        if filtered.is_empty() {
            output.push_str("No workspace files matched this filter.\n");
        } else {
            for path in filtered {
                output.push_str(&format!("• {}\n", path.display()));
            }
        }
        self.info = output;
        Ok(())
    }

    pub fn show_settings(&mut self) -> Result<(), String> {
        let allocated_port = self.allocated_service_port().ok();
        let library_root = self
            .library_root()?
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not configured".into());
        self.info = format!(
            "CORTEX SETTINGS\n\n{}\n\nVault root: {}\nAllocated service port: {}\nProvider runtime: {}\n\nUse the Provider action for a live provider/network probe.",
            serde_json::to_string_pretty(&self.settings).map_err(|error| error.to_string())?,
            library_root,
            allocated_port
                .map(|port| port.to_string())
                .unwrap_or_else(|| "unavailable".into()),
            self.runtime_status
        );
        Ok(())
    }

    pub fn show_changes(&mut self) -> Result<(), String> {
        let review = match ReviewSnapshot::collect(self.workspace.root()) {
            Ok(review) => review,
            Err(error) => {
                self.info = format!(
                    "CHANGES\n\nGit review is unavailable for this workspace.\n\
This is a normal nonfatal state when the project is not using Git.\n\n\
Detail:\n{error}\n\n\
Cortex transactions and Activity remain available for source-change history."
                );
                let _ = self.activity.append(
                    ActivityKind::Review,
                    "Review unavailable",
                    &error,
                    Some(false),
                    json!({"workspace": self.workspace.root()}),
                );
                return Ok(());
            }
        };

        let transaction =
            serde_json::to_string_pretty(&review.transaction).map_err(|error| error.to_string())?;
        let mut output = String::from("CHANGES\n\n");
        output.push_str(&format!("{}\n\n", review.summary));

        if !review.git_active {
            output.push_str(
                "Git repository: not active\n\
Cortex will show transaction information without treating missing Git as an error.\n\n",
            );
        } else if review.diff.is_empty() {
            output.push_str("Git diff: no uncommitted diff reported.\n\n");
        } else {
            output.push_str("Git diff\n--------\n");
            output.push_str(&review.diff);
            output.push_str("\n\n");
        }

        output.push_str("Cortex transaction\n------------------\n");
        output.push_str(&transaction);
        self.info = output;

        let review_metadata = serde_json::to_value(&review).map_err(|error| error.to_string())?;
        self.activity.append(
            ActivityKind::Review,
            "Review refreshed",
            review.summary.clone(),
            Some(true),
            review_metadata,
        )?;
        Ok(())
    }

    pub fn search_vault(&mut self, query: &str) -> Result<(), String> {
        let query = query.trim();
        if query.is_empty() {
            self.info = "VAULT\n\nSearch Cortex's persisted project knowledge.\n\
Type a term into the field above and press Vault.\n\n\
Examples:\n• roadmap\n• renderer\n• project browser\n• last build failure"
                .into();
            return Ok(());
        }

        let client = self.ensure_client()?;
        let value = client.tool("vault.search", json!({"query": query, "limit": 12}))?;
        self.info = format!(
            "VAULT\n\nQuery: {query}\n\n{}",
            serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
        );
        self.activity.append(
            ActivityKind::Vault,
            "Vault search",
            query,
            Some(true),
            value,
        )?;
        Ok(())
    }

    pub fn show_tasks(&mut self) -> Result<(), String> {
        let tasks = self.tasks.list(24)?;
        let development = DevelopmentStore::open(&self.workspace.cortex_state_dir())?;
        let active_run = development.active()?;
        let roadmap = development.roadmap()?;

        let mut output = String::from("CORTEX DEVELOPMENT / TASKS\n\n");
        if let Some(run) = &active_run {
            output.push_str(&format!(
                "ACTIVE MILESTONE\n  {} — {}\n  Stage: {:?}\n  Repair budget: {} / {}\n  Next: {}\n  Last error: {}\n  Evidence: {} item(s)\n\n",
                run.milestone_id,
                run.milestone_title,
                run.stage,
                run.repair_budget.used,
                run.repair_budget.maximum,
                run.next_action.as_deref().unwrap_or("—"),
                run.last_error.as_deref().unwrap_or("—"),
                run.evidence.len(),
            ));
        } else {
            output.push_str("ACTIVE MILESTONE\n  No durable development run is active.\n\n");
        }

        if let Some(roadmap) = &roadmap {
            output.push_str("ROADMAP\n");
            for milestone in &roadmap.milestones {
                output.push_str(&format!(
                    "  [{:?}] {} — {}\n",
                    milestone.status, milestone.id, milestone.title
                ));
            }
            output.push('\n');
        }

        if tasks.is_empty() {
            output.push_str("RECENT TASKS\n  No recorded task jobs for this workspace.\n");
            self.info = output;
            return Ok(());
        }

        output.push_str("RECENT TASKS — MOST RECENT 24\n\n");
        for task in tasks {
            let progress = match task.progress.total {
                Some(total) if total > 0 => format!(
                    "{} / {} — {}",
                    task.progress.current,
                    total,
                    task.progress.message.as_str()
                ),
                _ if !task.progress.message.is_empty() => task.progress.message.clone(),
                _ => "no live progress recorded".into(),
            };
            output.push_str(&format!(
                "{:?}  {}\n  {}\n  Progress: {}\n  Detail: {}\n  ID: {}{}\n\n",
                task.status,
                task.title,
                task.kind,
                progress,
                if task.detail.is_empty() {
                    "—"
                } else {
                    &task.detail
                },
                task.id,
                if task.cancellation_requested {
                    "  [CANCEL REQUESTED]"
                } else {
                    ""
                }
            ));
        }
        self.info = output;
        Ok(())
    }

    pub fn show_artifacts(&mut self) -> Result<(), String> {
        let artifacts = self.artifacts(100)?;
        let mut text = String::from("ARTIFACTS\n\n");
        if artifacts.is_empty() {
            text.push_str("No Cortex artifacts or visual evidence are currently recorded.\n");
        } else {
            text.push_str(&format!("Most recent {} item(s)\n\n", artifacts.len()));
            for artifact in artifacts {
                let display = artifact
                    .path
                    .strip_prefix(self.workspace.root())
                    .unwrap_or(&artifact.path);
                text.push_str(&format!(
                    "• {:?} — {} B\n  {}\n\n",
                    artifact.kind,
                    artifact.bytes,
                    display.display()
                ));
            }
        }
        self.info = text;
        Ok(())
    }

    pub fn inspect_image(&mut self, path: &Path, prompt: &str) -> Result<Value, String> {
        let client = self.ensure_client()?;
        let relative = path
            .strip_prefix(self.workspace.root())
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        let value = client.tool(
            "vision.inspect",
            json!({"path": relative, "prompt": prompt}),
        )?;
        self.info = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
        self.activity.append(
            ActivityKind::Artifact,
            "Vision inspection",
            relative,
            Some(true),
            value.clone(),
        )?;
        Ok(value)
    }

    pub fn run_build_check(&mut self) -> Result<(), String> {
        if !self.workspace.root().join("Cargo.toml").is_file() {
            let mut task = self.tasks.create("Project validate")?;
            self.tasks.update(
                &mut task,
                TaskStatus::Running,
                "detected project validation profile",
                Value::Null,
            )?;
            match self.run_project_operation(ProjectOperation::Validate) {
                Ok(value) => {
                    self.tasks.update(
                        &mut task,
                        TaskStatus::Succeeded,
                        "project validation passed",
                        value,
                    )?;
                    return Ok(());
                }
                Err(error) => {
                    self.tasks
                        .update(&mut task, TaskStatus::Failed, &error, Value::Null)?;
                    return Err(error);
                }
            }
        }

        let mut task = self.tasks.create("Cargo check")?;
        self.tasks.update(
            &mut task,
            TaskStatus::Running,
            "build.cargo_check",
            Value::Null,
        )?;
        self.activity.append(
            ActivityKind::Build,
            "Cargo check",
            "running",
            None,
            json!({"task": task.id.clone()}),
        )?;

        let client = self.ensure_client()?;
        let result = client.tool("build.cargo_check", json!({}));
        match result {
            Ok(value) => {
                self.tasks.update(
                    &mut task,
                    TaskStatus::Succeeded,
                    "cargo check passed",
                    value.clone(),
                )?;
                self.activity.append(
                    ActivityKind::Build,
                    "Cargo check",
                    "passed",
                    Some(true),
                    value,
                )?;
                Ok(())
            }
            Err(error) => {
                self.tasks
                    .update(&mut task, TaskStatus::Failed, &error, Value::Null)?;
                self.activity.append(
                    ActivityKind::Error,
                    "Cargo check",
                    &error,
                    Some(false),
                    Value::Null,
                )?;
                Err(error)
            }
        }
    }

    pub fn refresh_overview(&mut self) -> Result<(), String> {
        let bootstrap = DesktopBootstrap::load(self.workspace.root())?;
        let context_store = ContextStore::new(self.workspace.clone());
        let context = context_store
            .load_inventory()?
            .map(|inventory| {
                format!(
                    "{} files / {} textual / {} bytes{}",
                    inventory.file_count,
                    inventory.textual_files,
                    inventory.total_bytes,
                    if inventory.truncated {
                        " [TRUNCATED]"
                    } else {
                        ""
                    }
                )
            })
            .unwrap_or_else(|| "not scanned".into());
        let index = context_store
            .load_index()?
            .map(|index| {
                format!(
                    "{} files / {} languages / scan={}{}",
                    index.files.len(),
                    index.language_counts.len(),
                    index.scanned_unix_ms,
                    if index.truncated { " [TRUNCATED]" } else { "" }
                )
            })
            .unwrap_or_else(|| "not indexed".into());
        let registered_workspaces = self.registry.list()?.len();
        let library_root = self
            .library_root()?
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not configured".into());
        let allocated_port = self
            .allocated_service_port()
            .map(|port| port.to_string())
            .unwrap_or_else(|_| "unavailable".into());
        self.info = format!(
            "CORTEX WORKSPACE\n\nName: {}\nKinds: {:?}\nBuild systems: {:?}\nAdapters: {:?}\nState: {}\nContext: {}\nIndex: {}\nKnown workspaces: {}\nVault root: {}\nAllocated service port: {}\nCortex home: {}\nConversations: {}\nService: {}",
            bootstrap.workspace.name,
            bootstrap.workspace.kinds,
            bootstrap.workspace.build_systems,
            bootstrap.workspace.adapters,
            bootstrap.state_root.display(),
            context,
            index,
            registered_workspaces,
            library_root,
            allocated_port,
            self.registry.home().display(),
            bootstrap.conversations.len(),
            service_label(bootstrap.service.as_ref())
        );
        Ok(())
    }

    pub fn show_context_index(&mut self) -> Result<(), String> {
        let context = ContextStore::new(self.workspace.clone());
        self.info = match context.load_index()? {
            Some(index) => format!(
                "CORTEX CONTEXT INDEX\n\nFiles: {}\nLanguages: {}\nScanned: {}\nHash limit: {} bytes\nTruncated: {}\nPath: {}",
                index.files.len(),
                index.language_counts.len(),
                index.scanned_unix_ms,
                index.max_hash_bytes,
                index.truncated,
                context.index_path().display()
            ),
            None => format!(
                "CORTEX CONTEXT INDEX\n\nNo persistent index exists yet.\nUse Index to build it.\nPath: {}",
                context.index_path().display()
            ),
        };
        Ok(())
    }

    pub fn rebuild_context_index(
        &mut self,
        max_files: usize,
        max_hash_bytes: usize,
    ) -> Result<Value, String> {
        let mut task = self.tasks.create("Context index")?;
        self.tasks.update(
            &mut task,
            TaskStatus::Running,
            "workspace context indexing",
            Value::Null,
        )?;
        self.activity.append(
            ActivityKind::Context,
            "Context index",
            "running",
            None,
            json!({"task": task.id.clone()}),
        )?;

        let context = ContextStore::new(self.workspace.clone());
        match context.rebuild_index(max_files, max_hash_bytes) {
            Ok((index, delta)) => {
                let result = json!({
                    "index": index,
                    "delta": delta,
                    "path": context.index_path()
                });
                self.tasks.update(
                    &mut task,
                    TaskStatus::Succeeded,
                    "context index rebuilt",
                    result.clone(),
                )?;
                self.activity.append(
                    ActivityKind::Context,
                    "Context index",
                    "completed",
                    Some(true),
                    json!({
                        "task": task.id.clone(),
                        "new_files": delta.new_files.len(),
                        "modified_files": delta.modified_files.len(),
                        "deleted_files": delta.deleted_files.len(),
                        "unchanged_files": delta.unchanged_files,
                        "hashed_files": delta.hashed_files,
                        "reused_hashes": delta.reused_hashes
                    }),
                )?;
                self.info = format!(
                    "CORTEX CONTEXT INDEX\n\nFiles: {}\nNew: {}\nModified: {}\nDeleted: {}\nUnchanged: {}\nHashed: {}\nHashes reused: {}\nPath: {}",
                    index.files.len(),
                    delta.new_files.len(),
                    delta.modified_files.len(),
                    delta.deleted_files.len(),
                    delta.unchanged_files,
                    delta.hashed_files,
                    delta.reused_hashes,
                    context.index_path().display()
                );
                Ok(result)
            }
            Err(error) => {
                self.tasks
                    .update(&mut task, TaskStatus::Failed, &error, Value::Null)?;
                self.activity.append(
                    ActivityKind::Error,
                    "Context index failed",
                    &error,
                    Some(false),
                    json!({"task": task.id.clone()}),
                )?;
                Err(error)
            }
        }
    }

    pub fn set_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        self.settings.set(key, value)?;
        self.settings.save(&self.workspace.cortex_state_dir())
    }

    pub fn initialize_runtime(&mut self) -> Result<Value, String> {
        match self
            .ensure_client()
            .and_then(|client| client.provider_status())
        {
            Ok(status) => {
                let model_count = status
                    .get("models")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                let selected = status
                    .get("selected_model")
                    .and_then(Value::as_str)
                    .unwrap_or("none");
                let state = status
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                self.runtime_status =
                    format!("{state} / {model_count} models / selected={selected}");
                self.activity.append(
                    ActivityKind::Service,
                    "Cortex provider",
                    self.runtime_status.clone(),
                    Some(true),
                    status.clone(),
                )?;
                Ok(status)
            }
            Err(error) => {
                self.runtime_status = format!("offline / {error}");
                let _ = self.activity.append(
                    ActivityKind::Error,
                    "Cortex provider offline",
                    &error,
                    Some(false),
                    json!({"lmstudio_url": self.settings.lmstudio_url}),
                );
                Err(error)
            }
        }
    }

    pub fn probe_provider_quiet(&mut self) -> bool {
        match self
            .ensure_client()
            .and_then(|client| client.provider_status())
        {
            Ok(status) => {
                let model_count = status
                    .get("models")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                let selected = status
                    .get("selected_model")
                    .and_then(Value::as_str)
                    .unwrap_or("none");
                let state = status
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                self.runtime_status =
                    format!("{state} / {model_count} models / selected={selected}");
                status
                    .get("ready")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            }
            Err(error) => {
                self.runtime_status = format!("offline / {error}");
                false
            }
        }
    }

    pub fn refresh_repository_vault_audit(&mut self) -> Result<(), String> {
        let audit = self.ensure_client()?.tool("git.vault.audit", json!({}))?;
        self.activity.append(
            ActivityKind::Workspace,
            "Repository Vault audit",
            "Git-visible large-file candidates reviewed against Cortex Vault policy",
            Some(true),
            audit.clone(),
        )?;
        self.info = format!(
            "REPOSITORIES\n\nVault/Git audit refreshed.\n\n{}",
            compact_json(&audit, 8_000)
        );
        Ok(())
    }

    pub fn create_repository_safety_checkpoint(&mut self) -> Result<(), String> {
        let checkpoint = self.ensure_client()?.tool(
            "git.safety.create",
            json!({"label":"Cortex GUI safety checkpoint"}),
        )?;
        self.activity.append(
            ActivityKind::Workspace,
            "Repository safety checkpoint",
            "created hidden Cortex recovery ref without changing the active branch/index",
            Some(true),
            checkpoint.clone(),
        )?;
        self.info = format!(
            "REPOSITORIES\n\nSafety checkpoint created.\n\n{}",
            compact_json(&checkpoint, 6_000)
        );
        Ok(())
    }

    pub fn push_repository_local(&mut self) -> Result<(), String> {
        let push = self.ensure_client()?.tool("git.push.local", json!({}))?;
        self.activity.append(
            ActivityKind::Workspace,
            "Repository local push",
            "current branch pushed to cortex-local",
            Some(true),
            push.clone(),
        )?;
        self.info = format!(
            "REPOSITORIES\n\nLocal Forgejo push completed.\n\n{}",
            compact_json(&push, 6_000)
        );
        Ok(())
    }

    pub fn refresh_repository_provider(&mut self) -> Result<(), String> {
        let snapshot = self
            .ensure_client()?
            .tool("git.provider.refresh", json!({}))?;
        self.activity.append(
            ActivityKind::Workspace,
            "Repository provider refresh",
            "machine-local Forgejo repository metadata refreshed",
            Some(true),
            snapshot.clone(),
        )?;
        self.info = format!(
            "REPOSITORIES\n\nMachine-local Forgejo metadata refreshed.\n\n{}",
            compact_json(&snapshot, 8_000)
        );
        Ok(())
    }

    pub fn provider_status(&mut self) -> Result<Value, String> {
        self.initialize_runtime()
    }

    pub fn show_provider_status(&mut self) -> Result<(), String> {
        match self.initialize_runtime() {
            Ok(status) => {
                self.info = format!(
                    "CORTEX PROVIDER\n\n{}",
                    serde_json::to_string_pretty(&status).map_err(|error| error.to_string())?
                );
            }
            Err(error) => {
                self.info = format!(
                    "CORTEX PROVIDER\n\nStatus: offline\nConfigured endpoint: {}\n\n{}\n\nCortex Native Models are preferred when available. Check the selected provider/model status, then retry explicitly after the provider is ready.",
                    self.settings.lmstudio_url,
                    error
                );
            }
        }
        Ok(())
    }

    pub fn models(&mut self) -> Result<Value, String> {
        self.ensure_client()?.models()
    }

    pub fn artifacts(&self, limit: usize) -> Result<Vec<ArtifactEntry>, String> {
        discover_artifacts(
            self.workspace.root(),
            &self.workspace.cortex_state_dir(),
            &self.workspace.sessions_dir(),
            limit,
        )
    }

    pub fn certify_chat_roundtrip(&mut self) -> DesktopChatCertificationReport {
        let mut quiet = |_stage: &str| {};
        self.certify_chat_roundtrip_with_progress(&mut quiet)
    }

    pub fn certify_chat_roundtrip_with_progress(
        &mut self,
        on_progress: &mut dyn FnMut(&str),
    ) -> DesktopChatCertificationReport {
        let mut report = DesktopChatCertificationReport {
            schema_version: 1,
            conversation_id: None,
            stream_events: 0,
            user_message_persisted: false,
            assistant_message_persisted: false,
            conversation_reopened: false,
            provider_roundtrip: false,
            structured_tool_roundtrip: false,
            ready: false,
            error: None,
        };

        let previous_active_conversation = self.active_conversation.clone();
        let mut certification_conversation_id: Option<String> = None;

        let result = (|| -> Result<(), String> {
            on_progress("creating internal certification conversation");
            let conversation = self.conversations.create(RUNTIME_CERTIFICATION_TITLE)?;
            let conversation_id = conversation.id;
            self.active_conversation = Some(conversation_id.clone());
            certification_conversation_id = Some(conversation_id.clone());
            report.conversation_id = Some(conversation_id.clone());

            on_progress("dispatching live provider streaming probe");
            let mut stream_events = 0usize;
            let send_result = self.send_chat_stream(RUNTIME_CERTIFICATION_PROMPT, &mut |_| {
                stream_events = stream_events.saturating_add(1)
            });
            report.stream_events = stream_events;
            send_result?;
            if let Some(pending) = &self.pending_provider_request {
                let detail = if pending.last_error.trim().is_empty() {
                    "provider error was not recorded".to_string()
                } else {
                    pending.last_error.clone()
                };
                return Err(format!(
                    "chat certification entered provider-recovery state instead of completing a provider round-trip: {detail}"
                ));
            }

            on_progress("provider round-trip completed; verifying persisted messages");
            let persisted = self.conversations.load(&conversation_id)?;
            report.user_message_persisted = persisted
                .messages
                .iter()
                .any(|message| matches!(message.role, ConversationRole::User));
            report.assistant_message_persisted = persisted.messages.iter().any(|message| {
                matches!(message.role, ConversationRole::Assistant)
                    && !message.content.trim().is_empty()
            });
            report.provider_roundtrip =
                report.assistant_message_persisted && report.stream_events > 0;

            on_progress("dispatching live structured-tool transport probe");
            let tool_probe = self.ensure_client()?.provider_tool_smoke()?;
            report.structured_tool_roundtrip =
                Self::structured_tool_probe_proves_continuity(&tool_probe);
            if !report.structured_tool_roundtrip {
                return Err(format!(
                    "live provider round-trip succeeded for ordinary chat but failed to prove structured project-tool transport: {}",
                    compact_json(&tool_probe, 4_000)
                ));
            }

            on_progress("reopening conversation store for recovery verification");
            let reopened =
                ConversationStore::open(self.workspace.cortex_state_dir(), self.workspace.root())?
                    .load(&conversation_id)?;
            report.conversation_reopened = reopened.messages.len() == persisted.messages.len();
            report.ready = report.user_message_persisted
                && report.assistant_message_persisted
                && report.conversation_reopened
                && report.provider_roundtrip
                && report.structured_tool_roundtrip;
            on_progress("chat certification verification complete");
            Ok(())
        })();

        if let Some(conversation_id) = certification_conversation_id.as_deref() {
            let _ = self.conversations.archive(conversation_id);
        }
        self.active_conversation = previous_active_conversation
            .filter(|id| {
                self.conversations
                    .load(id)
                    .is_ok_and(|conversation| !conversation.archived)
            })
            .or_else(|| {
                self.conversations
                    .list(false)
                    .ok()
                    .and_then(|conversations| {
                        conversations
                            .into_iter()
                            .find(|conversation| {
                                !is_internal_runtime_certification_conversation(conversation)
                            })
                            .map(|conversation| conversation.id)
                    })
            });
        let _ = self.persist_background_state();

        if let Err(error) = result {
            report.error = Some(error);
        }
        report
    }

    fn structured_tool_probe_proves_continuity(tool_probe: &Value) -> bool {
        let calls_include_workspace_status = |key: &str| {
            tool_probe
                .get(key)
                .and_then(Value::as_array)
                .is_some_and(|calls| {
                    calls.iter().any(|call| {
                        call.get("name").and_then(Value::as_str) == Some("workspace.status")
                    })
                })
        };

        // H63+ proof: a real first structured call, authoritative stateless replay,
        // and a real second structured call all succeeded.
        if tool_probe
            .get("multi_turn_tool_continuity")
            .and_then(Value::as_bool)
            == Some(true)
        {
            return calls_include_workspace_status("first_tool_calls")
                && calls_include_workspace_status("second_tool_calls");
        }

        // Backward compatibility for older single-turn provider smoke reports.
        calls_include_workspace_status("tool_calls")
    }

    pub fn certify(&self) -> DesktopCertificationReport {
        let state_root = self.workspace.cortex_state_dir();
        let conversations = ConversationStore::open(&state_root, self.workspace.root()).is_ok();
        let settings = CortexSettings::load_or_default(&state_root).is_ok();
        let permissions =
            PermissionPolicy::load_or_default(&state_root.join("permissions.json")).is_ok();
        let service_state_readable = self.service.read().is_ok();
        let service_executable = sibling_cortex_executable().is_ok();
        let plugins = PluginRegistry::discover(self.workspace.root(), &state_root).is_ok();
        let artifacts = self.artifacts(20).is_ok();
        let workspace = self.workspace.root().is_dir();
        let ready = workspace
            && conversations
            && settings
            && permissions
            && service_state_readable
            && service_executable
            && plugins
            && artifacts;
        let mut notes = Vec::new();
        if self
            .service
            .read()
            .ok()
            .flatten()
            .map(|state| state.status != ServiceStatus::Running)
            .unwrap_or(true)
        {
            notes.push(
                "Desktop foundation is valid, but Cortex service is not currently running."
                    .to_string(),
            );
        }
        DesktopCertificationReport {
            schema_version: 1,
            workspace,
            conversations,
            settings,
            permissions,
            service_state_readable,
            service_executable,
            plugins,
            artifacts,
            ready,
            notes,
        }
    }

    fn ensure_conversation(&mut self) -> Result<String, String> {
        if let Some(id) = self.active_conversation.clone() {
            return Ok(id);
        }
        let conversation = self.conversations.create("New conversation")?;
        self.active_conversation = Some(conversation.id.clone());
        Ok(conversation.id)
    }

    fn configured_provider(&self) -> String {
        std::env::var("CORTEX_PROVIDER")
            .unwrap_or_else(|_| self.settings.provider.clone())
            .trim()
            .to_ascii_lowercase()
    }

    fn recover_native_transport_reset(
        &mut self,
        error: &str,
    ) -> Result<Option<CortexClient>, String> {
        if self.configured_provider() == "lmstudio" || !is_provider_transport_reset(error) {
            return Ok(None);
        }
        self.runtime_status = "provider / recovering local transport".into();
        self.activity.append(
            ActivityKind::Service,
            "Native provider transport recovery",
            format!(
                "loopback provider connection reset; restarting Desktop-owned runtime once: {error}"
            ),
            Some(false),
            json!({"automatic_retry": true, "error": error}),
        )?;
        let _ = self.service.stop();
        if let Some(model_host) = &self.model_host {
            let _ = model_host.stop();
        }
        std::thread::sleep(Duration::from_millis(150));
        self.ensure_client().map(Some)
    }

    fn ensure_client(&mut self) -> Result<CortexClient, String> {
        let executable = sibling_cortex_executable()?;
        let provider = self.configured_provider();
        if let Ok(client) = CortexClient::discover(self.workspace.root()) {
            let executable_matches = self
                .service
                .read()?
                .map(|state| same_executable(&state.executable, &executable))
                .unwrap_or(false);
            match client.require_desktop_api() {
                Ok(_) if executable_matches => {
                    if provider == "lmstudio" {
                        self.ensure_lmstudio_compatibility()?;
                    } else {
                        self.ensure_native_model_host()?;
                        self.require_native_provider_ready(&client)?;
                    }
                    return Ok(client);
                }
                result => {
                    let reason = match result {
                        Ok(_) => format!(
                            "running Cortex service executable does not match this desktop build: expected {}",
                            executable.display()
                        ),
                        Err(error) => error,
                    };
                    let _ = self.activity.append(
                        ActivityKind::Service,
                        "Cortex service compatibility restart",
                        &reason,
                        Some(false),
                        json!({"expected_executable": executable}),
                    );
                    match self.service.read()? {
                        Some(state) if state.status == ServiceStatus::Running => {
                            self.service.stop().map_err(|error| {
                                format!(
                                    "{reason}; failed to stop incompatible Cortex service: {error}"
                                )
                            })?;
                        }
                        Some(_) => {
                            let _ = self.service.clear_stale();
                        }
                        None => {}
                    }
                }
            }
        }
        if !self.settings.auto_start_service {
            return Err("Cortex service is offline and automatic start is disabled.".into());
        }

        let allocated_port = self.allocated_service_port()?;
        let provider_url = if provider == "lmstudio" {
            self.ensure_lmstudio_compatibility()?;
            self.settings.lmstudio_url.clone()
        } else {
            self.ensure_native_model_host()?.endpoint
        };

        let mut environment = vec![
            ("CORTEX_PROVIDER".to_string(), provider.clone()),
            ("CORTEX_NATIVE_MODEL_URL".to_string(), provider_url.clone()),
            (
                "CORTEX_LMSTUDIO_URL".to_string(),
                self.settings.lmstudio_url.clone(),
            ),
            (
                "CORTEX_COMFYUI_URL".to_string(),
                self.settings.comfyui_url.clone(),
            ),
            (
                "CORTEX_DESKTOP_OWNER_PID".to_string(),
                std::process::id().to_string(),
            ),
        ];
        for (generic_key, legacy_key, value) in [
            (
                "CORTEX_MODEL_CHAT",
                "CORTEX_LMSTUDIO_MODEL_CHAT",
                self.settings.chat_model.as_ref(),
            ),
            (
                "CORTEX_MODEL_TOOL",
                "CORTEX_LMSTUDIO_MODEL_TOOL",
                self.settings.tool_model.as_ref(),
            ),
            (
                "CORTEX_MODEL_VISION",
                "CORTEX_LMSTUDIO_MODEL_VISION",
                self.settings.vision_model.as_ref(),
            ),
            (
                "CORTEX_MODEL_EMBEDDING",
                "CORTEX_LMSTUDIO_MODEL_EMBEDDING",
                self.settings.embedding_model.as_ref(),
            ),
        ] {
            if let Some(value) = value {
                environment.push((generic_key.to_string(), value.clone()));
                environment.push((legacy_key.to_string(), value.clone()));
            }
        }
        self.service
            .spawn_executable_with_env(executable, allocated_port, &environment)?;
        let client = CortexClient::discover(self.workspace.root())?;
        client.require_desktop_api()?;
        if provider != "lmstudio" {
            self.require_native_provider_ready(&client)?;
        }
        self.activity.append(
            ActivityKind::Service,
            "Cortex service",
            format!("started by desktop on port {allocated_port}"),
            Some(true),
            json!({"port": allocated_port}),
        )?;
        Ok(client)
    }

    fn ensure_native_model_host(&mut self) -> Result<ModelHostState, String> {
        let library = self
            .registry
            .library_root()?
            .ok_or_else(|| "Cortex Vault root is not configured".to_string())?;
        let models_root = library.root.join("Models");
        let catalog = discover_models(&models_root)?;
        if catalog.models.is_empty() {
            let detail = native_models_unavailable_detail(&models_root);
            self.runtime_status = "native models / not configured".into();
            self.activity.append(
                ActivityKind::Service,
                "Cortex Native Models",
                &detail,
                Some(false),
                json!({
                    "provider":"native",
                    "state":"not_configured",
                    "models_root":models_root,
                    "models":0
                }),
            )?;
            return Err(detail);
        }
        if self.model_host.is_none() {
            self.model_host = Some(ModelHostRegistry::new(&library.root)?);
        }
        let executable = sibling_model_host_executable()?;
        let state = self
            .model_host
            .as_ref()
            .ok_or_else(|| "Cortex model-host registry is unavailable".to_string())?
            .start(
                executable,
                &library.root,
                std::process::id(),
                self.settings.native_model_host_port,
                self.settings.native_auto_bootstrap,
                self.settings.native_models_max,
            )?;
        self.activity.append(
            ActivityKind::Service,
            "Cortex Native Models",
            format!(
                "model host ready on port {} with {} discovered GGUF model(s)",
                state.port, state.model_count
            ),
            Some(true),
            json!({
                "provider":"native",
                "port":state.port,
                "models":state.model_count,
                "llama_server":state.llama_executable
            }),
        )?;
        Ok(state)
    }

    fn ensure_lmstudio_compatibility(&mut self) -> Result<(), String> {
        let provider = LmStudioProvider::new(&self.settings.lmstudio_url, None);
        if provider.health().is_ok() {
            return Ok(());
        }
        if !self.settings.lmstudio_auto_start {
            return Err(format!(
                "LM Studio compatibility provider is offline at {} and automatic startup is disabled.",
                self.settings.lmstudio_url
            ));
        }
        let lms = locate_lms_executable().ok_or_else(|| {
            "LM Studio compatibility provider is selected, but lms.exe was not found. Switch provider to native or install/bootstrap the LM Studio CLI.".to_string()
        })?;
        let port = loopback_port(&self.settings.lmstudio_url).unwrap_or(1234);
        let mut command = lms_command(&lms);
        command
            .arg("server")
            .arg("start")
            .arg("--port")
            .arg(port.to_string())
            .arg("--bind")
            .arg("127.0.0.1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = command.status().map_err(|error| {
            format!("failed to auto-start LM Studio compatibility server: {error}")
        })?;
        if !status.success() {
            return Err(format!(
                "LM Studio compatibility auto-start exited with {status}"
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if provider.health().is_ok() {
                self.lmstudio_started_by_cortex = true;
                self.activity.append(
                    ActivityKind::Service,
                    "LM Studio compatibility",
                    format!("auto-started headless provider on port {port}"),
                    Some(true),
                    json!({"provider":"lmstudio","port":port,"command":lms}),
                )?;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(format!(
            "LM Studio compatibility server did not become ready at {} after automatic startup",
            self.settings.lmstudio_url
        ))
    }

    fn activity_text(&self) -> String {
        let mut output = String::from("ACTIVITY / TASKS\n\n");
        if let Ok(tasks) = self.tasks.list(12) {
            for task in tasks {
                output.push_str(&format!(
                    "[TASK {:?}] {} — {}\n",
                    task.status, task.title, task.detail
                ));
            }
        }
        if let Ok(events) = self.activity.recent(30) {
            for event in events {
                output.push_str(&format!(
                    "[{:?}] {} — {}\n",
                    event.kind, event.title, event.detail
                ));
            }
        }
        output
    }

    fn status_text(&self) -> String {
        let service = self.service.read().ok().flatten();
        let conversation = self
            .active_conversation
            .as_deref()
            .and_then(|id| self.conversations.load(id).ok())
            .map(|conversation| conversation.title)
            .unwrap_or_else(|| "No conversation".into());
        let provider = if self.pending_provider_request.is_some() {
            "waiting / request preserved"
        } else {
            self.runtime_status.as_str()
        };
        let milestone = self
            .active_development_run()
            .map(|run| format!("{} / {:?}", run.milestone_id, run.stage))
            .unwrap_or_else(|| "none".into());
        format!(
            "{}  /  {}    |    Service: {}    |    Provider: {}    |    Milestone: {}",
            self.workspace.profile().name,
            conversation,
            service_label(service.as_ref()),
            provider,
            milestone
        )
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
enum HandoffMode {
    Code,
    Repair,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct ProjectIntentContract {
    #[serde(default)]
    language: String,
    #[serde(default)]
    build_system: String,
    #[serde(default)]
    platform: String,
    #[serde(default)]
    application_kind: String,
    #[serde(default)]
    scaffold: String,
    #[serde(default)]
    expected_window: bool,
    #[serde(default)]
    runtime_required: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PendingProjectCreation {
    conversation_id: String,
    original_prompt: String,
    project_name: String,
    target: PathBuf,
    #[serde(default)]
    intent: ProjectIntentContract,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PendingHandoff {
    conversation_id: String,
    execution_prompt: String,
    mode: HandoffMode,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PendingProviderRequest {
    conversation_id: String,
    original_prompt: String,
    status_message_id: String,
    attempts: u32,
    #[serde(default)]
    last_error: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PendingDesktopState {
    handoff: Option<PendingHandoff>,
    project_creation: Option<PendingProjectCreation>,
    #[serde(default)]
    provider_request: Option<PendingProviderRequest>,
}

fn handoff_label(mode: &str) -> Option<&'static str> {
    match mode {
        "inspect" => Some("Inspect"),
        "plan" => Some("Plan"),
        "apply" => Some("Code"),
        "repair" => Some("Repair"),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeveloperIntent {
    ReadOnly,
    ContinueAndCode,
    Code,
    Repair,
    NewProject,
}

fn developer_intent(prompt: &str) -> Option<DeveloperIntent> {
    let lower = prompt.trim().to_ascii_lowercase();

    if lower.starts_with("chat:")
        || lower.contains("stay in chat")
        || lower.contains("general chat")
        || lower.contains("don't use tools")
        || lower.contains("do not use tools")
    {
        return None;
    }

    let new_project_request = [
        "separate project",
        "seperate project",
        "new project",
        "another project",
        "create a project",
        "make a project",
        "start a project",
    ]
    .iter()
    .any(|term| lower.contains(term))
        && [
            "create", "make", "start", "build", "want", "can we", "could we",
        ]
        .iter()
        .any(|term| lower.contains(term));

    if new_project_request {
        return Some(DeveloperIntent::NewProject);
    }

    let continuation = matches!(
        lower.as_str(),
        "continue"
            | "continue."
            | "keep going"
            | "keep going."
            | "next"
            | "next step"
            | "next steps"
            | "carry on"
            | "carry on."
            | "continue from there"
            | "continue from here"
            | "next task"
            | "next task please"
            | "what next"
            | "what's next"
            | "whats next"
    ) || [
        "continue next",
        "continue the project",
        "continue on this project",
        "continue where we left off",
        "pick up where we left off",
        "resume the project",
        "next step in the roadmap",
        "next steps in the roadmap",
    ]
    .iter()
    .any(|term| lower.contains(term));

    if continuation {
        return Some(DeveloperIntent::ContinueAndCode);
    }

    if is_brainstorming_chat_request(&lower) {
        return None;
    }

    if [
        "what are you capable of",
        "what can you do right now",
        "what can you do now",
        "what can we do right now",
        "what can we do now",
        "what are we working on",
        "what have we done",
        "where are we at",
        "where are we",
        "what is left",
        "what's left",
        "whats left",
        "remaining work",
        "can we index",
        "index this project",
        "index the project",
        "separate project",
        "new project",
        "create another project",
        "make another project",
        "start another project",
        "you are cortex",
        "you're cortex",
        "youre cortex",
        "you are the developer",
        "you're the developer",
        "youre the developer",
        "building open2d",
        "building open 2d",
        "building this project",
        "help me build this",
        "work on this project",
        "look over this",
        "look this over",
        "what would you change",
        "what would you recommend",
        "recommend changing",
        "recommend changes",
        "recommend improvements",
        "suggest changes",
        "suggest improvements",
    ]
    .iter()
    .any(|term| lower.contains(term))
    {
        return Some(DeveloperIntent::ReadOnly);
    }

    let project_context = [
        "project",
        "repo",
        "repository",
        "workspace",
        "source",
        "code",
        "crate",
        "rust",
        "cargo",
        "build",
        "compile",
        "foundry",
        "open2d",
        "open 2d",
        "cortex",
        "editor",
        "runtime",
        "file",
        "implementation",
        "roadmap",
        "feature",
        "plugin",
        "asset",
        "level",
        "scene",
        "ui",
        "ux",
        "game maker",
        "game engine",
        "authoring",
    ]
    .iter()
    .any(|term| lower.contains(term));

    if !project_context {
        return None;
    }

    let contains_any = |terms: &[&str]| terms.iter().any(|term| lower.contains(term));

    if contains_any(&[
        "build failed",
        "compile failed",
        "compiler error",
        "error log",
        "crash",
        "broken",
        "regression",
        "repair",
        "debug",
        "failing test",
        "test failed",
        "won't build",
        "doesn't build",
        "does not build",
    ]) {
        return Some(DeveloperIntent::Repair);
    }

    if contains_any(&[
        "implement",
        "add ",
        "create ",
        "change ",
        "modify",
        "update ",
        "wire ",
        "refactor",
        "remove ",
        "rename ",
        "patch ",
        "edit ",
        "make this",
        "fix ",
        "code ",
        "build this",
        "build it",
        "build the",
        "finish this",
        "finish up",
        "finish the",
        "complete this",
        "complete it",
        "wrap this up",
        "get this working",
        "get it working",
    ]) {
        return Some(DeveloperIntent::Code);
    }

    if contains_any(&[
        "inspect",
        "audit",
        "review",
        "analyze",
        "analyse",
        "check the project",
        "check project",
        "current project",
        "project status",
        "find where",
        "locate ",
        "where is",
        "how is this implemented",
        "plan ",
        "roadmap",
        "map out",
        "design ",
        "architecture",
        "next steps",
        "what should we do",
        "strategy",
        "spec ",
        "specification",
        "what can we add",
        "what should we add",
        "what should change",
        "how should we improve",
        "how can we improve",
        "what do you recommend",
        "what would you recommend",
        "what should we work on",
        "what can we work on",
        "what is the next useful step",
    ]) {
        return Some(DeveloperIntent::ReadOnly);
    }

    None
}

fn developer_followup_intent(prompt: &str) -> Option<DeveloperIntent> {
    let lower = prompt.trim().to_ascii_lowercase();

    if lower.starts_with("chat:")
        || lower.contains("stay in chat")
        || lower.contains("general chat")
        || lower.contains("don't use tools")
        || lower.contains("do not use tools")
        || is_brainstorming_chat_request(&lower)
    {
        return None;
    }

    let contains_any = |terms: &[&str]| terms.iter().any(|term| lower.contains(term));

    if matches!(
        lower.as_str(),
        "yes proceed"
            | "yes, proceed"
            | "yes please proceed"
            | "please proceed"
            | "proceed"
            | "proceed."
            | "yes go ahead"
            | "yes, go ahead"
            | "okay proceed"
            | "ok proceed"
            | "yep proceed"
            | "yeah proceed"
            | "continue"
            | "continue."
            | "resume"
            | "resume."
            | "keep going"
            | "keep going."
            | "keep working"
            | "keep working."
            | "finish it"
            | "finish this"
            | "finish up"
            | "complete it"
            | "complete this"
            | "wrap this up"
            | "do the rest"
            | "next"
            | "next step"
            | "next steps"
            | "carry on"
            | "carry on."
            | "continue from there"
            | "continue from here"
            | "next task"
            | "next task please"
    ) || contains_any(&[
        "proceed with it",
        "proceed with the fix",
        "proceed with the repair",
        "proceed with the build",
        "continue coding",
        "continue working",
        "continue the work",
        "continue the project",
        "continue where we left off",
        "pick up where we left off",
        "resume the project",
        "resume work",
        "finish the current task",
        "finish the milestone",
        "finish up the current task",
        "complete the current task",
        "get this working",
        "get it working",
        "work through the roadmap",
        "do the next task",
        "do the next tasks",
        "get us to the next build point",
        "get this to the next build point",
        "get us to something i can test",
    ]) {
        return Some(DeveloperIntent::ContinueAndCode);
    }

    if matches!(lower.as_str(), "retry" | "retry." | "again" | "again.")
        || contains_any(&[
            "fix this",
            "fix it",
            "please fix",
            "can you fix",
            "could you fix",
            "why can you not fix",
            "why can't you fix",
            "why cant you fix",
            "repair this",
            "repair it",
            "resolve this",
            "resolve it",
            "correct this",
            "make this work",
            "make it work",
            "that didn't work",
            "that did not work",
            "still broken",
            "still doesn't work",
            "still does not work",
            "try again",
            "failed again",
            "it failed",
            "that failed",
            "why did that fail",
            "fix whatever failed",
            "repair whatever is broken",
            "it crashed",
            "crashed again",
            "build is failing",
            "test is failing",
        ])
    {
        return Some(DeveloperIntent::Repair);
    }

    if contains_any(&[
        "do it",
        "go ahead",
        "apply it",
        "apply that",
        "make the change",
        "make that change",
        "implement it",
        "implement that",
        "add it",
        "add that",
        "change it",
        "change that",
        "wire it in",
        "wire this in",
        "hook it up",
        "hook this up",
        "clean this up",
        "refactor it",
        "refactor this",
        "rename it",
        "rename this",
        "remove it",
        "remove this",
        "use this instead",
        "use that instead",
        "move this",
        "move it",
        "make it faster",
        "make this faster",
        "polish this",
        "polish it",
        "same thing here",
        "same thing there",
        "same as before",
        "do the same thing",
        "do that too",
        "do this too",
        "go with that",
        "use the previous approach",
        "apply the previous change here",
        "make it like that",
        "make this like that",
        "make a new one",
        "add another one",
    ]) {
        return Some(DeveloperIntent::Code);
    }

    if contains_any(&[
        "why is this happening",
        "why did this happen",
        "what is happening",
        "what's happening",
        "whats happening",
        "what is wrong",
        "what's wrong",
        "whats wrong",
        "explain this",
        "explain that",
        "show me what is wrong",
        "what changed",
        "is it done",
        "is this done",
        "where were we",
        "what were we doing",
        "what remains",
        "what is left",
        "what's left",
        "whats left",
    ]) {
        return Some(DeveloperIntent::ReadOnly);
    }

    None
}

fn mutation_execution_retry_required(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "without a successful project-file mutation",
        "without changing any project files",
        "no implementation will be reported as successful",
        "prose, code snippets, or suggested edits were rejected",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn route_developer_intent(
    prompt: &str,
    has_active_developer_context: bool,
) -> Option<DeveloperIntent> {
    developer_intent(prompt).or_else(|| {
        has_active_developer_context
            .then(|| developer_followup_intent(prompt))
            .flatten()
    })
}

fn is_provider_retry_request(prompt: &str) -> bool {
    matches!(
        prompt.trim().to_ascii_lowercase().as_str(),
        "retry provider"
            | "resume provider"
            | "retry last provider request"
            | "resume last provider request"
    )
}

fn is_provider_transport_reset(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "os error 10054",
        "forcibly closed by the remote host",
        "connection reset by peer",
        "connection reset",
        "broken pipe",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn is_recoverable_provider_error(error: &str) -> bool {
    if is_provider_transport_reset(error) {
        return true;
    }
    let lower = error.to_ascii_lowercase();
    [
        "connection refused",
        "actively refused",
        "os error 10061",
        "connect failed",
        "provider is unavailable",
        "lm studio provider is unavailable",
        "lm studio has no available text model",
        "no available text model",
        "no model is currently loaded",
        "not currently loaded/available",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn provider_status_detail(status: &Value) -> String {
    let state = status
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let selected = status
        .get("selected_model")
        .and_then(Value::as_str)
        .unwrap_or("none");
    let detail = status
        .get("detail")
        .and_then(Value::as_str)
        .unwrap_or("The configured Cortex provider is not ready yet.");
    format!("Provider state: {state}\nSelected text model: {selected}\n{detail}")
}

fn provider_recovery_card(detail: &str, attempts: u32, ready: bool) -> String {
    if ready {
        format!(
            "[Provider]\\n\\n✓ The configured Cortex provider is ready.\\n\\nThe preserved request resumed successfully after {attempts} explicit attempt(s)."
        )
    } else {
        format!(
            "[Provider]\\n\\nThe configured Cortex provider is not ready yet.\\n\\n{detail}\\n\\nAttempts: {attempts}\\nRequest: preserved, but NOT automatically replayed.\\n\\nUse `retry provider` or `resume provider` to retry this exact request. Use normal `continue` to continue the active project/milestone instead. Use Stop to discard the preserved retry."
        )
    }
}

fn provider_recovery_cancelled_card(detail: &str) -> String {
    format!(
        "[Provider]\\n\\nStopped. The preserved provider retry was discarded and will not replay automatically.\\n\\nLast provider detail: {detail}"
    )
}

fn is_context_index_request(prompt: &str) -> bool {
    let lower = prompt.trim().to_ascii_lowercase();
    [
        "can we index this project",
        "can we index the project",
        "index this project",
        "index the project",
        "reindex this project",
        "rebuild the project index",
        "rebuild context index",
    ]
    .iter()
    .any(|term| lower.contains(term))
}

fn is_brainstorming_chat_request(lower: &str) -> bool {
    [
        "can we add new features",
        "can we add features",
        "what features can we add",
        "what else can we add",
        "new feature ideas",
        "feature ideas",
        "useful features",
        "brainstorm features",
        "brainstorm new features",
        "brainstorm this",
        "what could this become",
        "what should this become",
        "ideas for this project",
    ]
    .iter()
    .any(|term| lower.contains(term))
}

fn is_handoff_yes(input: &str) -> bool {
    matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
            | "yeah"
            | "yep"
            | "sure"
            | "ok"
            | "okay"
            | "go"
            | "go ahead"
            | "proceed"
            | "do it"
            | "fix it"
            | "continue"
            | "sounds good"
            | "apply it"
            | "apply that"
            | "ship it"
            | "that's fine"
            | "thats fine"
            | "let's do it"
            | "lets do it"
    )
}

fn is_handoff_no(input: &str) -> bool {
    matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "n" | "no"
            | "nope"
            | "cancel"
            | "cancel that"
            | "stop"
            | "never mind"
            | "nevermind"
            | "leave it"
            | "don't change that"
            | "dont change that"
            | "stay"
            | "stay in chat"
    )
}

fn readable_message(content: &str) -> String {
    let mut text = content
        .replace("<br />", "\n")
        .replace("<br/>", "\n")
        .replace("<br>", "\n")
        .replace("\r\n", "\n")
        .replace("**", "")
        .replace("__", "");

    let mut output = Vec::new();
    let mut in_table = false;

    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();

        if trimmed.is_empty() {
            if output.last().is_some_and(|last: &String| !last.is_empty()) {
                output.push(String::new());
            }
            in_table = false;
            continue;
        }

        if is_markdown_table_separator(trimmed) {
            in_table = true;
            continue;
        }

        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let cells = trimmed
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim())
                .filter(|cell| !cell.is_empty())
                .collect::<Vec<_>>();
            if cells.len() == 2 {
                if !in_table
                    && cells[0].eq_ignore_ascii_case("category")
                    && cells[1].eq_ignore_ascii_case("detail")
                {
                    in_table = true;
                    continue;
                }
                output.push(format!("{}: {}", cells[0], cells[1]));
            } else if !cells.is_empty() {
                output.push(cells.join("  •  "));
            }
            in_table = true;
            continue;
        }

        let heading = trimmed.trim_start_matches('#').trim();
        if trimmed.starts_with('#') && !heading.is_empty() {
            if output.last().is_some_and(|last: &String| !last.is_empty()) {
                output.push(String::new());
            }
            output.push(heading.to_string());
            output.push("─".repeat(heading.chars().count().clamp(4, 56)));
            continue;
        }

        output.push(line.to_string());
        in_table = false;
    }

    while output.last().is_some_and(|line| line.is_empty()) {
        output.pop();
    }

    text = output.join("\r\n");
    text
}

fn is_markdown_table_separator(line: &str) -> bool {
    let compact = line
        .chars()
        .filter(|character| !matches!(character, '|' | '-' | ':' | ' '))
        .collect::<String>();
    compact.is_empty() && line.contains('-') && line.contains('|')
}

fn indent_block(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{prefix}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

#[derive(Clone, Debug, Default, Deserialize)]
struct DiffBubblePayload {
    path: String,
    #[serde(default)]
    status: String,
    diff: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileBubblePayload {
    path: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    preview: String,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    bytes: usize,
}

fn normalize_conversation_export_format(format: &str) -> Result<&'static str, String> {
    match format.trim().to_ascii_lowercase().as_str() {
        "txt" | "text" => Ok("txt"),
        "md" | "markdown" => Ok("md"),
        "json" => Ok("json"),
        other => Err(format!("unsupported conversation export format: {other}")),
    }
}

fn safe_export_stem(value: &str) -> String {
    let mut stem = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    while stem.contains("__") {
        stem = stem.replace("__", "_");
    }
    let stem = stem.trim_matches('_').to_string();
    if stem.is_empty() {
        "conversation".into()
    } else {
        stem
    }
}

fn render_conversation_export(conversation: &Conversation, format: &str) -> Result<String, String> {
    if format == "json" {
        return serde_json::to_string_pretty(conversation).map_err(|error| error.to_string());
    }
    let mut out = String::new();
    if format == "md" {
        out.push_str(&format!(
            "# {}\n\nProject: `{}`\n\n",
            conversation.title,
            conversation.workspace_root.display()
        ));
    } else {
        out.push_str(&format!(
            "CORTEX CONVERSATION EXPORT\nTitle: {}\nProject: {}\nConversation: {}\n\n",
            conversation.title,
            conversation.workspace_root.display(),
            conversation.id
        ));
    }
    for message in &conversation.messages {
        let role = match message.role {
            ConversationRole::User => "USER",
            ConversationRole::Assistant => "CORTEX",
            ConversationRole::System => "SYSTEM",
            ConversationRole::Tool => "TOOL",
        };
        if format == "md" {
            out.push_str(&format!("## {role}\n\n{}\n\n", message.content));
        } else {
            out.push_str(&format!("[{role}]\n{}\n\n", message.content));
        }
    }
    Ok(out)
}

fn default_chat_revision() -> u32 {
    1
}

fn split_unified_diff_by_file(diff: &str) -> Vec<(String, String)> {
    let normalized = diff.replace("\r\n", "\n");
    let lines = normalized.lines().collect::<Vec<_>>();
    let mut output = Vec::new();
    let mut index = 0usize;

    while index < lines.len() {
        if !lines[index].starts_with("diff --git ") {
            index += 1;
            continue;
        }
        let start = index;
        let mut path = String::new();
        index += 1;
        while index < lines.len() && !lines[index].starts_with("diff --git ") {
            if let Some(value) = lines[index].strip_prefix("+++ b/") {
                path = value.to_string();
            }
            index += 1;
        }
        if path.is_empty() {
            let header = lines[start].split_whitespace().collect::<Vec<_>>();
            if let Some(value) = header.get(3) {
                path = value.trim_start_matches("b/").to_string();
            }
        }
        let mut chunk = lines[start..index].join("\n");
        chunk.push('\n');
        output.push((path, chunk));
    }

    if output.is_empty() && !normalized.trim().is_empty() {
        let path = normalized
            .lines()
            .find_map(|line| line.strip_prefix("+++ b/"))
            .unwrap_or("changes")
            .to_string();
        output.push((path, normalized));
    }
    output
}

fn file_language(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => "Rust",
        "toml" => "TOML",
        "json" => "JSON",
        "wgsl" => "WGSL",
        "glsl" | "vert" | "frag" => "GLSL",
        "hlsl" => "HLSL",
        "cpp" | "cc" | "cxx" => "C++",
        "c" => "C",
        "h" | "hpp" | "hxx" => "Header",
        "cs" => "C#",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "js" | "mjs" | "cjs" => "JavaScript",
        "ts" | "tsx" => "TypeScript",
        "py" => "Python",
        "ps1" => "PowerShell",
        "sh" | "bash" => "Shell",
        "md" => "Markdown",
        "yaml" | "yml" => "YAML",
        "xml" => "XML",
        "html" => "HTML",
        "css" => "CSS",
        _ => "Text",
    }
}

fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    const TIB: f64 = GIB * 1024.0;
    let value = bytes as f64;
    if value >= TIB {
        format!("{:.2} TiB", value / TIB)
    } else if value >= GIB {
        format!("{:.2} GiB", value / GIB)
    } else if value >= MIB {
        format!("{:.2} MiB", value / MIB)
    } else if value >= KIB {
        format!("{:.2} KiB", value / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn project_depth(node: &cortex_registry::ProjectGraphNode, report: &DriveCatalogReport) -> usize {
    let mut depth = 0_usize;
    let mut current = node.parent_id.as_deref();
    while let Some(parent_id) = current {
        depth += 1;
        if depth >= 16 {
            break;
        }
        current = report
            .project_nodes
            .iter()
            .find(|candidate| candidate.id == parent_id)
            .and_then(|parent| parent.parent_id.as_deref());
    }
    depth
}

fn format_migration_plan_rows(plans: &[MigrationPlan]) -> Vec<String> {
    if plans.is_empty() {
        return vec![
            "No migration plans. Catalog first, then review a project before proposing a move."
                .into(),
        ];
    }
    plans
        .iter()
        .map(|plan| {
            format!(
                "{}  [{:?}]\n{}\n{} actions — {}",
                plan.title,
                plan.status,
                plan.id,
                plan.actions.len(),
                plan.note
            )
        })
        .collect()
}

fn conversation_blocks(conversation: Conversation) -> Vec<DesktopChatBlock> {
    conversation
        .messages
        .into_iter()
        .map(|message| {
            if matches!(message.role, ConversationRole::Tool) {
                if let Some(raw) = message.content.strip_prefix("[DiffBubble]\n\n") {
                    if let Ok(payload) = serde_json::from_str::<DiffBubblePayload>(raw) {
                        return DesktopChatBlock {
                            message_id: message.id.clone(),
                            role: "tool".into(),
                            label: "Code changes".into(),
                            text: payload.diff,
                            kind: "diff".into(),
                            path: payload.path,
                            language: String::new(),
                            status: payload.status,
                            created_unix_ms: message.created_unix_ms,
                            feedback_score: message.feedback_score,
                            revision: message.revision,
                        };
                    }
                }
                let file_bubble = message
                    .content
                    .strip_prefix("[FileBubble]\n\n")
                    .or_else(|| message.content.strip_prefix("[FileBubble]\\n\\n"));
                if let Some(raw) = file_bubble {
                    if let Ok(payload) = serde_json::from_str::<FileBubblePayload>(raw) {
                        let mut preview = payload.preview;
                        if payload.truncated {
                            preview.push_str(&format!(
                                "\n\n… preview truncated; full file is {} bytes on disk.",
                                payload.bytes
                            ));
                        }
                        return DesktopChatBlock {
                            message_id: message.id.clone(),
                            role: "tool".into(),
                            label: "File".into(),
                            text: preview,
                            kind: "file".into(),
                            path: payload.path,
                            language: payload.language,
                            status: payload.status,
                            created_unix_ms: message.created_unix_ms,
                            feedback_score: message.feedback_score,
                            revision: message.revision,
                        };
                    }
                }
            }

            let (role, label, content) = match message.role {
                ConversationRole::User => ("user", "You", message.content.as_str()),
                ConversationRole::Assistant => {
                    if let Some(rest) = message.content.strip_prefix("[Developer]\n\n") {
                        ("assistant", "Cortex · Developer", rest)
                    } else if let Some(rest) = message.content.strip_prefix("[Inspect]\n\n") {
                        ("assistant", "Cortex · Inspect", rest)
                    } else if let Some(rest) = message.content.strip_prefix("[Plan]\n\n") {
                        ("assistant", "Cortex · Plan", rest)
                    } else if let Some(rest) = message.content.strip_prefix("[Code]\n\n") {
                        ("assistant", "Cortex · Code", rest)
                    } else if let Some(rest) = message.content.strip_prefix("[Apply]\n\n") {
                        ("assistant", "Cortex · Code", rest)
                    } else if let Some(rest) = message.content.strip_prefix("[Repair]\n\n") {
                        ("assistant", "Cortex · Repair", rest)
                    } else {
                        ("assistant", "Cortex", message.content.as_str())
                    }
                }
                ConversationRole::System => ("system", "System", message.content.as_str()),
                ConversationRole::Tool => ("tool", "Tool activity", message.content.as_str()),
            };

            DesktopChatBlock {
                message_id: message.id,
                role: role.to_string(),
                label: label.to_string(),
                text: readable_message(content),
                kind: "message".into(),
                path: String::new(),
                language: String::new(),
                status: String::new(),
                created_unix_ms: message.created_unix_ms,
                feedback_score: message.feedback_score,
                revision: message.revision,
            }
        })
        .collect()
}

fn format_conversation(conversation: Conversation) -> String {
    if conversation.messages.is_empty() {
        return "Start the conversation below.".into();
    }

    let mut output = String::new();
    for (index, message) in conversation.messages.into_iter().enumerate() {
        if index > 0 {
            output.push_str("\r\n\r\n");
        }

        let content = readable_message(&message.content);
        match message.role {
            ConversationRole::User => {
                output.push_str("                         YOU\r\n");
                output.push_str("                         ───\r\n");
                output.push_str(&indent_block(&content, "                         │ "));
            }
            ConversationRole::Assistant => {
                let (label, body) = if let Some(rest) = content.strip_prefix("[Developer]\r\n\r\n")
                {
                    ("CORTEX  ·  DEVELOPER", rest)
                } else if let Some(rest) = content.strip_prefix("[Inspect]\r\n\r\n") {
                    ("CORTEX  ·  INSPECT", rest)
                } else if let Some(rest) = content.strip_prefix("[Plan]\r\n\r\n") {
                    ("CORTEX  ·  PLAN", rest)
                } else if let Some(rest) = content.strip_prefix("[Code]\r\n\r\n") {
                    ("CORTEX  ·  CODE", rest)
                } else if let Some(rest) = content.strip_prefix("[Apply]\r\n\r\n") {
                    ("CORTEX  ·  CODE", rest)
                } else if let Some(rest) = content.strip_prefix("[Repair]\r\n\r\n") {
                    ("CORTEX  ·  REPAIR", rest)
                } else {
                    ("CORTEX", content.as_str())
                };
                output.push_str(label);
                output.push_str("\r\n");
                output.push_str(&"─".repeat(label.chars().count().clamp(6, 56)));
                output.push_str("\r\n");
                output.push_str(&indent_block(body, "│ "));
            }
            ConversationRole::System => {
                output.push_str("SYSTEM\r\n");
                output.push_str(&indent_block(&content, "· "));
            }
            ConversationRole::Tool => {
                output.push_str("TOOL ACTIVITY\r\n");
                output.push_str(&indent_block(&content, "· "));
            }
        }
    }
    output
}

fn tail_chars(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    text.chars().skip(total - max_chars).collect()
}

fn infer_project_intent(prompt: &str) -> ProjectIntentContract {
    let lower = prompt.to_ascii_lowercase();
    let contains_any = |terms: &[&str]| terms.iter().any(|term| lower.contains(term));

    let wants_cpp = contains_any(&["c++", "cpp", "c plus plus"]);
    let wants_rust = contains_any(&["rust", "cargo"]);
    let wants_cmake = lower.contains("cmake");
    let wants_windows = contains_any(&["windows", "win32", "win64"]);
    let wants_gui = contains_any(&[
        "desktop app",
        "desktop application",
        "windows app",
        "windows application",
        "gui",
        "windowed",
        "native window",
        "working window",
    ]);
    let wants_console = lower.contains("console");
    let runtime_required = prompt_requests_launch(prompt)
        || contains_any(&[
            "launch the app",
            "launch it",
            "run the actual",
            "prove it runs",
        ]);

    let (language, build_system, scaffold) = if wants_cpp || wants_cmake {
        (
            "cpp".to_string(),
            "cmake".to_string(),
            if wants_gui || wants_windows {
                "cpp_cmake_windows_gui".to_string()
            } else {
                "cpp_cmake_console".to_string()
            },
        )
    } else if wants_rust {
        (
            "rust".to_string(),
            "cargo".to_string(),
            "rust_cargo".to_string(),
        )
    } else if contains_any(&["python"]) {
        (
            "python".to_string(),
            "python".to_string(),
            "universal_empty".to_string(),
        )
    } else if contains_any(&["typescript", "javascript", "node"]) {
        (
            if lower.contains("typescript") {
                "typescript".to_string()
            } else {
                "javascript".to_string()
            },
            "node".to_string(),
            "universal_empty".to_string(),
        )
    } else if contains_any(&["java", "gradle"]) {
        (
            "java".to_string(),
            "gradle".to_string(),
            "universal_empty".to_string(),
        )
    } else if contains_any(&["c#", "csharp", ".net", "dotnet"]) {
        (
            "csharp".to_string(),
            "dotnet".to_string(),
            "universal_empty".to_string(),
        )
    } else if lower.contains("go ") || lower.starts_with("go ") || lower.contains("golang") {
        (
            "go".to_string(),
            "go".to_string(),
            "universal_empty".to_string(),
        )
    } else {
        // Compatibility default for existing Cortex new-project behavior. The
        // intent itself remains unspecified, so acceptance will not falsely claim
        // the user requested Rust. Explicit technologies always override this.
        (String::new(), String::new(), "rust_compat".to_string())
    };

    let application_kind = if wants_gui {
        "desktop_gui".to_string()
    } else if wants_console {
        "console".to_string()
    } else {
        String::new()
    };

    ProjectIntentContract {
        language,
        build_system,
        platform: if wants_windows {
            "windows".to_string()
        } else {
            String::new()
        },
        application_kind,
        scaffold,
        expected_window: wants_gui,
        runtime_required,
    }
}

fn normalized_project_intent(
    existing: &ProjectIntentContract,
    original_prompt: &str,
) -> ProjectIntentContract {
    if existing.scaffold.trim().is_empty() {
        infer_project_intent(original_prompt)
    } else {
        existing.clone()
    }
}

fn intent_value(value: &str) -> &str {
    if value.trim().is_empty() {
        "unspecified"
    } else {
        value
    }
}

fn project_scaffold_label(intent: &ProjectIntentContract) -> &'static str {
    match intent.scaffold.as_str() {
        "cpp_cmake_windows_gui" => "C++ / CMake / native Windows GUI",
        "cpp_cmake_console" => "C++ / CMake console",
        "rust_cargo" => "Rust / Cargo",
        "universal_empty" => "universal empty project boundary",
        _ => "Rust / Cargo compatibility default",
    }
}

fn write_project_intent_contract(
    target: &Path,
    intent: &ProjectIntentContract,
) -> Result<(), String> {
    let cortex_dir = target.join(".cortex");
    fs::create_dir_all(&cortex_dir).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(intent).map_err(|error| error.to_string())?;
    fs::write(cortex_dir.join("project-intent.json"), bytes).map_err(|error| error.to_string())
}

fn create_standalone_project(
    target: &Path,
    project_name: &str,
    intent: &ProjectIntentContract,
) -> Result<(), String> {
    match intent.scaffold.as_str() {
        "cpp_cmake_windows_gui" => {
            create_standalone_cpp_cmake(target, project_name, true)?;
        }
        "cpp_cmake_console" => {
            create_standalone_cpp_cmake(target, project_name, false)?;
        }
        "universal_empty" => {
            if target.exists() {
                return Err(format!(
                    "standalone project target already exists: {}",
                    target.display()
                ));
            }
            fs::create_dir_all(target.join(".cortex")).map_err(|error| error.to_string())?;
        }
        _ => create_standalone_rust_binary(target)?,
    }
    write_project_intent_contract(target, intent)
}

fn cmake_target_name(project_name: &str) -> String {
    let mut result = project_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    while result.starts_with('_') {
        result.remove(0);
    }
    if result.is_empty()
        || !result
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic())
    {
        result.insert_str(0, "cortex_app_");
    }
    result.chars().take(48).collect()
}

fn create_standalone_cpp_cmake(
    target: &Path,
    project_name: &str,
    windows_gui: bool,
) -> Result<(), String> {
    if target.exists() {
        return Err(format!(
            "standalone project target already exists: {}",
            target.display()
        ));
    }

    fs::create_dir_all(target.join("src")).map_err(|error| error.to_string())?;
    fs::create_dir_all(target.join(".cortex")).map_err(|error| error.to_string())?;

    let target_name = cmake_target_name(project_name);
    let add_executable = if windows_gui {
        format!("add_executable({target_name} WIN32 src/main.cpp)")
    } else {
        format!("add_executable({target_name} src/main.cpp)")
    };
    let cmake = format!(
        "cmake_minimum_required(VERSION 3.20)\n\
project({target_name} LANGUAGES CXX)\n\n\
set(CMAKE_CXX_STANDARD 20)\n\
set(CMAKE_CXX_STANDARD_REQUIRED ON)\n\
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY \"${{CMAKE_BINARY_DIR}}/bin\")\n\
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY_DEBUG \"${{CMAKE_BINARY_DIR}}/bin\")\n\
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY_RELEASE \"${{CMAKE_BINARY_DIR}}/bin\")\n\
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY_RELWITHDEBINFO \"${{CMAKE_BINARY_DIR}}/bin\")\n\
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY_MINSIZEREL \"${{CMAKE_BINARY_DIR}}/bin\")\n\n\
{add_executable}\n\
target_compile_definitions({target_name} PRIVATE UNICODE _UNICODE)\n"
    );
    fs::write(target.join("CMakeLists.txt"), cmake).map_err(|error| error.to_string())?;

    let source = if windows_gui {
        r#"#include <windows.h>

LRESULT CALLBACK CortexScaffoldWindowProc(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam) {
    switch (message) {
    case WM_DESTROY:
        PostQuitMessage(0);
        return 0;
    default:
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
}

int WINAPI wWinMain(HINSTANCE instance, HINSTANCE, PWSTR, int show_command) {
    const wchar_t class_name[] = L"CortexScaffoldWindow";
    WNDCLASSW window_class{};
    window_class.lpfnWndProc = CortexScaffoldWindowProc;
    window_class.hInstance = instance;
    window_class.lpszClassName = class_name;
    window_class.hCursor = LoadCursorW(nullptr, IDC_ARROW);

    if (!RegisterClassW(&window_class)) {
        return 1;
    }

    HWND window = CreateWindowExW(
        0,
        class_name,
        L"Cortex C++ scaffold - implement the requested application",
        WS_OVERLAPPEDWINDOW,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        720,
        480,
        nullptr,
        nullptr,
        instance,
        nullptr);

    if (!window) {
        return 2;
    }

    ShowWindow(window, show_command);
    UpdateWindow(window);

    MSG message{};
    while (GetMessageW(&message, nullptr, 0, 0) > 0) {
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }
    return static_cast<int>(message.wParam);
}
"#
    } else {
        r#"#include <iostream>

int main() {
    std::cout << "Cortex C++ scaffold - implement the requested application\n";
    return 0;
}
"#
    };
    fs::write(target.join("src").join("main.cpp"), source).map_err(|error| error.to_string())?;

    #[cfg(windows)]
    let artifact = format!(".cortex/build/cmake/bin/{target_name}.exe");
    #[cfg(not(windows))]
    let artifact = format!(".cortex/build/cmake/bin/{target_name}");

    let profile = json!({
        "validate": ["cmake", "-S", ".", "-B", ".cortex/build/cmake"],
        "build": ["cmake", "--build", ".cortex/build/cmake", "--config", "Debug"],
        "test": ["ctest", "--test-dir", ".cortex/build/cmake", "-C", "Debug", "--output-on-failure"],
        "run": [artifact.clone()],
        "artifact": artifact,
        "expected_window": windows_gui,
    });
    fs::write(
        target.join(".cortex").join("project.json"),
        serde_json::to_vec_pretty(&profile).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    Ok(())
}

fn project_intent_acceptance(root: &Path) -> Result<Value, String> {
    let path = root.join(".cortex").join("project-intent.json");
    if !path.is_file() {
        return Ok(json!({
            "required": false,
            "success": true,
            "diagnostics": [],
        }));
    }

    let intent: ProjectIntentContract =
        serde_json::from_slice(&fs::read(&path).map_err(|error| error.to_string())?)
            .map_err(|error| format!("invalid project intent contract: {error}"))?;
    let plan = plan_project(root, intent.runtime_required);
    let mut diagnostics = Vec::<String>::new();

    let language_ok = match intent.language.as_str() {
        "" => true,
        "cpp" => plan.profile.languages.contains(&UniversalLanguageKind::Cpp),
        "rust" => plan
            .profile
            .languages
            .contains(&UniversalLanguageKind::Rust),
        "python" => plan
            .profile
            .languages
            .contains(&UniversalLanguageKind::Python),
        "javascript" => plan
            .profile
            .languages
            .contains(&UniversalLanguageKind::JavaScript),
        "typescript" => plan
            .profile
            .languages
            .contains(&UniversalLanguageKind::TypeScript),
        "java" => plan
            .profile
            .languages
            .contains(&UniversalLanguageKind::Java),
        "csharp" => plan
            .profile
            .languages
            .contains(&UniversalLanguageKind::CSharp),
        "go" => plan.profile.languages.contains(&UniversalLanguageKind::Go),
        _ => true,
    };
    if !language_ok {
        diagnostics.push(format!(
            "requested language `{}` does not match detected languages {:?}",
            intent.language, plan.profile.languages
        ));
    }

    let build_ok = match intent.build_system.as_str() {
        "" => true,
        "cargo" => plan
            .profile
            .build_systems
            .contains(&UniversalBuildSystemKind::Cargo),
        "cmake" => plan
            .profile
            .build_systems
            .contains(&UniversalBuildSystemKind::CMake),
        "python" => plan
            .profile
            .build_systems
            .contains(&UniversalBuildSystemKind::Python),
        "node" => plan
            .profile
            .build_systems
            .contains(&UniversalBuildSystemKind::Node),
        "gradle" => plan
            .profile
            .build_systems
            .contains(&UniversalBuildSystemKind::Gradle),
        "dotnet" => plan
            .profile
            .build_systems
            .contains(&UniversalBuildSystemKind::DotNet),
        "go" => plan
            .profile
            .build_systems
            .contains(&UniversalBuildSystemKind::Go),
        _ => true,
    };
    if !build_ok {
        diagnostics.push(format!(
            "requested build system `{}` does not match detected build systems {:?}",
            intent.build_system, plan.profile.build_systems
        ));
    }

    if intent.build_system == "cmake" && root.join("Cargo.toml").is_file() {
        diagnostics.push(
            "requested standalone CMake project contains an unexpected Cargo.toml; refusing to accept a Rust substitution"
                .into(),
        );
    }
    if intent.build_system == "cargo" && root.join("CMakeLists.txt").is_file() {
        diagnostics.push(
            "requested standalone Cargo project contains an unexpected CMakeLists.txt; refusing to accept a CMake substitution"
                .into(),
        );
    }

    if intent.expected_window {
        let explicit_window_profile = plan
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.expected_window);
        let cmake_window_target = fs::read_to_string(root.join("CMakeLists.txt"))
            .map(|source| {
                source.to_ascii_lowercase().contains("add_executable")
                    && source.to_ascii_lowercase().contains("win32")
            })
            .unwrap_or(false);
        let rust_window_subsystem = fs::read_to_string(root.join("src").join("main.rs"))
            .map(|source| source.contains("windows_subsystem"))
            .unwrap_or(false);
        if !(explicit_window_profile || cmake_window_target || rust_window_subsystem) {
            diagnostics.push(
                "requested desktop/windowed application does not expose an expected-window runtime profile or a windowed build target"
                    .into(),
            );
        }
    }

    if intent.platform == "windows" && intent.language == "cpp" {
        let has_cpp_entry =
            root.join("src").join("main.cpp").is_file() || root.join("main.cpp").is_file();
        if !has_cpp_entry {
            diagnostics.push(
                "requested native Windows C++ application is missing a C++ entry source".into(),
            );
        }
    }

    let success = diagnostics.is_empty();
    Ok(json!({
        "required": true,
        "success": success,
        "intent": intent,
        "detected_profile": plan.profile,
        "runtime_artifact": plan.runtime,
        "diagnostics": diagnostics,
    }))
}

fn standalone_project_name(prompt: &str) -> String {
    let lower = prompt.to_ascii_lowercase();

    for marker in ["called ", "named ", "name it "] {
        if let Some(index) = lower.find(marker) {
            let original_tail = &prompt[index + marker.len()..];
            let candidate = original_tail
                .split(|character: char| {
                    character == ',' || character == '.' || character == ';' || character == '\n'
                })
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join("-");
            let sanitized = sanitize_cargo_project_name(&candidate);
            if sanitized != "cortex-project" {
                return sanitized;
            }
        }
    }

    "cortex-project".into()
}

fn sanitize_cargo_project_name(value: &str) -> String {
    let mut output = String::new();
    let mut last_dash = false;

    for character in value.chars() {
        let normalized = character.to_ascii_lowercase();
        if normalized.is_ascii_alphanumeric() || normalized == '_' {
            output.push(normalized);
            last_dash = false;
        } else if matches!(normalized, '-' | ' ') && !output.is_empty() && !last_dash {
            output.push('-');
            last_dash = true;
        }
    }

    while output.ends_with('-') {
        output.pop();
    }

    if output.is_empty()
        || !output
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
    {
        "cortex-project".into()
    } else {
        output.chars().take(48).collect()
    }
}

fn available_project_target(parent: &Path, project_name: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    for suffix in 1..=100 {
        let leaf = if suffix == 1 {
            project_name.to_string()
        } else {
            format!("{project_name}-{suffix}")
        };
        let candidate = parent.join(leaf);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("could not find an available managed project directory after 100 attempts".into())
}

fn available_sibling_project_target(
    current_root: &Path,
    project_name: &str,
) -> Result<PathBuf, String> {
    let parent = current_root.parent().ok_or_else(|| {
        "active workspace has no parent directory for sibling project creation".to_string()
    })?;

    for suffix in 1..=100 {
        let leaf = if suffix == 1 {
            project_name.to_string()
        } else {
            format!("{project_name}-{suffix}")
        };
        let candidate = parent.join(leaf);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err("could not find an available sibling project directory after 100 attempts".into())
}

fn create_standalone_rust_binary(target: &Path) -> Result<(), String> {
    let target_parent = target
        .parent()
        .ok_or_else(|| "standalone project target has no parent directory".to_string())?;
    fs::create_dir_all(target_parent).map_err(|error| error.to_string())?;
    let target_parent = fs::canonicalize(target_parent).map_err(|error| error.to_string())?;
    if target.exists() {
        return Err(format!(
            "standalone project target already exists: {}",
            target.display()
        ));
    }

    let leaf = target
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "standalone project directory name is invalid".to_string())?;

    let mut cargo = Command::new("cargo");
    cargo
        .args(["new", "--bin", "--vcs", "none", leaf])
        .current_dir(&target_parent)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        // Cortex Desktop is a GUI process. Keep toolchain helpers in the
        // background instead of flashing a detached cargo.exe console.
        cargo.creation_flags(0x08000000);
    }
    let output = cargo
        .output()
        .map_err(|error| format!("failed to run cargo new: {error}"))?;

    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        if target.exists() {
            let _ = fs::remove_dir_all(target);
        }
        return Err(format!("cargo new failed: {detail}"));
    }

    if !target.join("Cargo.toml").is_file() || !target.join("src").join("main.rs").is_file() {
        let _ = fs::remove_dir_all(target);
        return Err(
            "cargo new returned success but the expected Rust binary scaffold is incomplete".into(),
        );
    }

    Ok(())
}

fn gate_cortex_quality(gate: &Value) -> Option<&Value> {
    gate.get("stages")
        .and_then(Value::as_array)
        .and_then(|stages| {
            stages.iter().find_map(|stage| {
                stage
                    .get("result")
                    .and_then(|result| result.get("cortex_quality"))
            })
        })
}

fn repair_signal_from_gate(gate: &Value) -> RepairSignal {
    let quality = gate_cortex_quality(gate);
    let likely_api_version_mismatch = quality
        .and_then(|value| value.pointer("/current/likely_api_version_mismatch"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let errors = quality
        .and_then(|value| value.pointer("/current/errors"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let missing_dependency_grounding = quality
        .and_then(|value| value.get("missing_dependency_grounding"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut files = std::collections::BTreeSet::new();
    if let Some(stages) = gate.get("stages").and_then(Value::as_array) {
        for stage in stages {
            if let Some(diagnostics) = stage
                .get("result")
                .and_then(|value| value.get("diagnostics"))
                .and_then(Value::as_array)
            {
                for diagnostic in diagnostics {
                    if let Some(file) = diagnostic.get("file").and_then(Value::as_str) {
                        if !file.trim().is_empty() && !file.contains(".cargo\\registry") {
                            files.insert(file.to_string());
                        }
                    }
                }
            }
        }
    }
    let affected_files = files.len().max(1);
    RepairSignal {
        likely_api_version_mismatch,
        errors_from_same_dependency: if likely_api_version_mismatch {
            errors
        } else {
            0
        },
        affected_files,
        compact_module: affected_files == 1 && errors <= 64,
        missing_dependency_grounding,
    }
}

fn normalize_repair_target(workspace_root: &Path, raw: &str) -> Option<String> {
    fn normalize(value: &str) -> String {
        value
            .replace('\\', "/")
            .trim_start_matches("//?/")
            .trim_end_matches('/')
            .to_string()
    }

    let candidate = normalize(raw.trim());
    if candidate.is_empty()
        || candidate.contains("/.cargo/registry/")
        || candidate.contains("/target/")
    {
        return None;
    }

    let root = normalize(&workspace_root.to_string_lossy());
    if !root.is_empty() {
        let candidate_lower = candidate.to_ascii_lowercase();
        let root_lower = root.to_ascii_lowercase();
        if candidate_lower == root_lower {
            return None;
        }
        if candidate_lower.starts_with(&(root_lower + "/")) {
            return Some(candidate[root.len() + 1..].to_string());
        }
    }

    let is_absolute = candidate.starts_with('/')
        || candidate
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':');
    if is_absolute {
        None
    } else {
        Some(candidate)
    }
}

fn repair_targets_from_gate(gate: &Value, workspace_root: &Path) -> Vec<String> {
    let mut ranked = Vec::<(u8, usize, String)>::new();
    let mut ordinal = 0usize;

    if let Some(stages) = gate.get("stages").and_then(Value::as_array) {
        for stage in stages {
            let Some(diagnostics) = stage
                .get("result")
                .and_then(|value| value.get("diagnostics"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for diagnostic in diagnostics {
                let Some(raw_file) = diagnostic
                    .get("file")
                    .or_else(|| diagnostic.get("file_name"))
                    .or_else(|| diagnostic.get("path"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let Some(path) = normalize_repair_target(workspace_root, raw_file) else {
                    continue;
                };
                let severity = diagnostic
                    .get("level")
                    .or_else(|| diagnostic.get("severity"));
                let rank = match severity {
                    Some(Value::String(level)) if level.to_ascii_lowercase().contains("error") => 0,
                    Some(Value::Number(level)) if level.as_u64() == Some(1) => 0,
                    _ => 1,
                };
                ranked.push((rank, ordinal, path));
                ordinal = ordinal.saturating_add(1);
            }
        }
    }

    ranked.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    let mut targets = Vec::new();
    for (_, _, path) in ranked {
        if !targets.iter().any(|existing| existing == &path) {
            targets.push(path);
        }
    }
    targets
}

fn controller_repair_target_evidence(client: &CortexClient, targets: &[String]) -> Value {
    let Some(primary) = targets.first() else {
        return json!({
            "primary_target": Value::Null,
            "source": Value::Null,
            "instruction": "The compiler did not expose a project-relative source target. Refresh deterministic validation once instead of guessing a file."
        });
    };

    match client.tool(
        "source.read",
        json!({"path": primary, "offset": 0, "max_bytes": 16_000}),
    ) {
        Ok(source) => json!({
            "primary_target": primary,
            "source": source,
            "instruction": "The controller already selected and read the primary compiler-implicated target. Do not rediscover it with broad source.search/status loops. Make one bounded mutation, then consume automatic compiler validation."
        }),
        Err(error) => json!({
            "primary_target": primary,
            "source_read_error": error,
            "instruction": "The compiler selected this target but the controller read failed. Read this exact target once; do not broaden into repository exploration before mutation."
        }),
    }
}

fn diagnostic_grounding_queries(gate: &Value) -> Vec<String> {
    let mut queries = std::collections::BTreeSet::new();
    fn collect_backticks(text: &str, queries: &mut std::collections::BTreeSet<String>) {
        let mut rest = text;
        while let Some(start) = rest.find('`') {
            rest = &rest[start + 1..];
            let Some(end) = rest.find('`') else {
                break;
            };
            let token = rest[..end].trim();
            rest = &rest[end + 1..];
            for part in token.split("::") {
                let cleaned = part.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
                if cleaned.len() >= 3
                    && cleaned.len() <= 80
                    && cleaned.chars().any(|c| c.is_ascii_alphabetic())
                {
                    queries.insert(cleaned.to_string());
                }
            }
        }
    }
    if let Some(stages) = gate.get("stages").and_then(Value::as_array) {
        for stage in stages {
            if let Some(diagnostics) = stage
                .get("result")
                .and_then(|value| value.get("diagnostics"))
                .and_then(Value::as_array)
            {
                for diagnostic in diagnostics {
                    if let Some(message) = diagnostic.get("message").and_then(Value::as_str) {
                        collect_backticks(message, &mut queries);
                    }
                    if let Some(rendered) = diagnostic.get("rendered").and_then(Value::as_str) {
                        collect_backticks(rendered, &mut queries);
                    }
                }
            }
        }
    }
    queries.into_iter().take(10).collect()
}

fn ground_repair_dependencies(
    client: &CortexClient,
    gate: &Value,
    packages: &[String],
) -> Result<Value, String> {
    let queries = diagnostic_grounding_queries(gate);
    let mut evidence = Vec::new();
    for package in packages {
        let broad = client.tool(
            "dependency.ground",
            json!({"package": package, "ecosystem": "cargo", "max_hits": 32}),
        )?;
        evidence.push(json!({"package": package, "kind": "exact_local_package", "result": broad}));
    }
    Ok(json!({
        "authority": "controller_exact_local_dependency_source",
        "packages": packages,
        "diagnostic_queries": queries,
        "evidence": evidence,
        "instruction": "Exact package grounding is complete. Do not spend the controller's pre-mutation observation budget on broad diagnostic symbol searches. The Repair-profile model may use its own bounded source.search allowance only when a specific symbol lookup is still required before the controller-selected mutation."
    }))
}

fn ensure_active_repair_transaction(client: &CortexClient) -> Result<(), String> {
    let status = client.tool("source.transaction_status", json!({}))?;
    let active = status
        .get("transaction")
        .is_some_and(|value| !value.is_null());
    if !active {
        let _ = client.tool(
            "source.begin_transaction",
            json!({"label": "controller-strategy-repair"}),
        )?;
    }
    Ok(())
}

fn run_project_quality_gate(client: &CortexClient, profile: &Value) -> Result<Value, String> {
    let capabilities = profile
        .get("quality_capabilities")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<std::collections::BTreeSet<_>>()
        })
        .unwrap_or_default();
    let stages = [
        ("format", "build.project_format"),
        ("validate", "build.project_validate"),
        ("lint", "build.project_lint"),
        ("build", "build.project_build"),
        ("test", "build.project_test"),
    ];
    let mut results = Vec::new();
    let mut success = true;
    for (capability, tool) in stages {
        if !capabilities.contains(capability) {
            continue;
        }
        let result = client.tool(tool, json!({}))?;
        let stage_success = command_succeeded(&result);
        results.push(json!({
            "capability": capability,
            "tool": tool,
            "success": stage_success,
            "result": result
        }));
        if !stage_success {
            success = false;
            break;
        }
    }
    if results.is_empty() {
        return Err(
            "Cortex detected no executable quality-gate capabilities for this project adapter"
                .into(),
        );
    }
    Ok(json!({"success": success, "stages": results}))
}

fn command_succeeded(value: &Value) -> bool {
    value
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn is_internal_runtime_certification_conversation(conversation: &Conversation) -> bool {
    conversation.title == RUNTIME_CERTIFICATION_TITLE
        || conversation
            .title
            .starts_with("Cortex runtime certification probe")
        || conversation.messages.iter().any(|message| {
            matches!(message.role, ConversationRole::User)
                && message.content.trim() == RUNTIME_CERTIFICATION_PROMPT
        })
}

fn archive_stale_runtime_certification_conversations(
    conversations: &ConversationStore,
) -> Result<usize, String> {
    let stale = conversations
        .list(false)?
        .into_iter()
        .filter(is_internal_runtime_certification_conversation)
        .map(|conversation| conversation.id)
        .collect::<Vec<_>>();
    for id in &stale {
        conversations.archive(id)?;
    }
    Ok(stale.len())
}

fn compact_json(value: &Value, max_chars: usize) -> String {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "<unavailable>".into());
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut result = text.chars().take(max_chars).collect::<String>();
    result.push_str("\n…<truncated>");
    result
}

fn prompt_requests_launch(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    [
        "launch",
        "run it",
        "run the app",
        "run the project",
        "start it",
        "start the app",
        "open the app",
        "open it",
    ]
    .iter()
    .any(|term| lower.contains(term))
}

fn direct_project_operation(prompt: &str) -> Option<ProjectOperation> {
    // H56: reserve the raw command-runner path for explicit project-operation
    // commands. Rich natural language such as `build the clock and launch it`
    // must continue into Developer execution so Cortex can inspect/mutate/repair
    // the requested artifact instead of blindly invoking Cargo first.
    let lower = prompt
        .trim()
        .trim_end_matches(['.', '!', '?'])
        .to_ascii_lowercase();
    match lower.as_str() {
        "build"
        | "build project"
        | "build the project"
        | "build workspace"
        | "build the workspace"
        | "run build"
        | "run project build" => Some(ProjectOperation::Build),
        "test" | "test project" | "test the project" | "run tests" | "run project tests" => {
            Some(ProjectOperation::Test)
        }
        "validate"
        | "validate project"
        | "validate the project"
        | "check project"
        | "check the project" => Some(ProjectOperation::Validate),
        _ => None,
    }
}

fn is_recovery_snapshot_request(prompt: &str) -> bool {
    let lower = prompt.trim().to_ascii_lowercase();

    let explicit_recovery = lower.contains("recovery snapshot")
        || lower.contains("recovery checkpoint")
        || lower.contains("git snapshot")
        || lower.contains("git checkpoint")
        || lower.contains("shadow git");
    if explicit_recovery {
        return true;
    }

    let concise_snapshot_command = lower.split_whitespace().count() <= 12
        && (lower.starts_with("snapshot ")
            || lower.starts_with("create snapshot")
            || lower.starts_with("create a snapshot")
            || lower.starts_with("make snapshot")
            || lower.starts_with("make a snapshot")
            || lower.starts_with("take snapshot")
            || lower.starts_with("take a snapshot"))
        && (lower.contains("project") || lower.contains("workspace"));

    concise_snapshot_command
}

fn normalize_project_selector(value: &str) -> String {
    let mut normalized = value
        .trim()
        .trim_matches(|character| {
            matches!(
                character,
                '`' | '"' | '\'' | '.' | ',' | ';' | ':' | '(' | ')' | '[' | ']'
            )
        })
        .replace('/', "\\")
        .to_ascii_lowercase();
    if let Some(stripped) = normalized.strip_prefix(r"\\?\") {
        normalized = stripped.to_string();
    }
    normalized.trim_end_matches('\\').to_string()
}

fn explicit_project_command_target(prompt: &str, verbs: &[&str]) -> Option<String> {
    let trimmed = prompt.trim();
    let lower = trimmed.to_ascii_lowercase();
    for verb in verbs {
        for prefix in [
            format!("{verb} "),
            format!("please {verb} "),
            format!("can you {verb} "),
            format!("could you {verb} "),
        ] {
            if lower.starts_with(&prefix) {
                let mut target = trimmed[prefix.len()..].trim();
                for optional in ["the project ", "project ", "the candidate ", "candidate "] {
                    if target.to_ascii_lowercase().starts_with(optional) {
                        target = target[optional.len()..].trim();
                        break;
                    }
                }
                let normalized = normalize_project_selector(target);
                return (!normalized.is_empty()).then_some(normalized);
            }
        }
    }
    None
}

fn explicit_library_registration_target(prompt: &str) -> Option<String> {
    explicit_project_command_target(prompt, &["register"])
}

fn explicit_library_unregistration_target(prompt: &str) -> Option<String> {
    explicit_project_command_target(prompt, &["unregister", "detach"])
}

fn is_bulk_library_registration_selector(selector: &str) -> bool {
    matches!(
        selector,
        "all"
            | "all projects"
            | "all candidates"
            | "all high-confidence projects"
            | "all high confidence projects"
            | "all high-confidence candidates"
            | "all high confidence candidates"
    )
}

fn catalog_project_matches_selector(
    project: &cortex_registry::ProjectGraphNode,
    selector: &str,
) -> bool {
    normalize_project_selector(&project.name) == selector
        || normalize_project_selector(&project.root.to_string_lossy()) == selector
        || normalize_project_selector(&project.id) == selector
}

fn registered_workspace_matches_selector(
    workspace: &cortex_registry::RegisteredWorkspace,
    selector: &str,
) -> bool {
    normalize_project_selector(&workspace.name) == selector
        || normalize_project_selector(&workspace.root.to_string_lossy()) == selector
        || normalize_project_selector(&workspace.id) == selector
}

fn is_library_registration_request(prompt: &str) -> bool {
    explicit_library_registration_target(prompt).is_some()
}

fn is_library_unregistration_request(prompt: &str) -> bool {
    explicit_library_unregistration_target(prompt).is_some()
}

fn scan_request_is_negated(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    [
        "do not scan",
        "don't scan",
        "dont scan",
        "do not perform another drive scan",
        "do not perform another scan",
        "without scanning",
        "continue without scanning",
        "no drive scan",
        "no vault scan",
        "no storage scan",
        "stop scanning",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
}

fn is_machine_scan_request(prompt: &str) -> bool {
    if scan_request_is_negated(prompt) {
        return false;
    }
    let lower = prompt.to_ascii_lowercase();
    let scan_verb = lower.contains("scan") || lower.contains("catalog") || lower.contains("index");
    scan_verb
        && (lower.contains("entire pc")
            || lower.contains("whole pc")
            || lower.contains("all drives")
            || lower.contains("entire computer")
            || lower.contains("whole computer")
            || lower.contains("all local drives"))
}

fn is_library_scan_request(prompt: &str) -> bool {
    if scan_request_is_negated(prompt) || is_machine_scan_request(prompt) {
        return false;
    }
    let lower = prompt.to_ascii_lowercase();
    (lower.contains("scan") || lower.contains("index") || lower.contains("catalog"))
        && (lower.contains("vault")
            || lower.contains("library")
            || lower.contains("storage")
            || lower.contains("drive")
            || lower.contains("directories")
            || lower.contains("directory"))
}

fn library_scan_text(report: &LibraryScanReport) -> String {
    let high = report
        .candidates
        .iter()
        .filter(|candidate| candidate.confidence >= 80)
        .count();
    let possible = report.candidates.len().saturating_sub(high);
    let mut output = format!(
        "Vault: `{}`\nScanned directories: {}\nIgnored/generated directories: {}\nHigh-confidence projects: {}\nPossible projects: {}\n",
        report.root.display(), report.scanned_directories, report.ignored_directories, high, possible
    );
    for candidate in report.candidates.iter().take(40) {
        let languages = if candidate.languages.is_empty() {
            "unknown".to_string()
        } else {
            candidate.languages.join(", ")
        };
        output.push_str(&format!(
            "\n- {} — {} — confidence {}%{}\n  `{}`\n  markers: {}\n  languages: {}",
            candidate.name,
            candidate.kind,
            candidate.confidence,
            if candidate.already_registered {
                " — already registered"
            } else {
                ""
            },
            candidate.root.display(),
            candidate.markers.join(", "),
            languages
        ));
    }
    if report.candidates.len() > 40 {
        output.push_str(&format!(
            "\n\n… {} additional candidates are stored in the persistent catalog.",
            report.candidates.len() - 40
        ));
    }
    output
}

fn format_library_candidate(candidate: &LibraryCandidate) -> String {
    let languages = if candidate.languages.is_empty() {
        "unknown".to_string()
    } else {
        candidate.languages.join(", ")
    };
    format!(
        "Folder `{}` looks like a {} project ({}% confidence). Markers: {}. Languages: {}. {}",
        candidate.root.display(),
        candidate.kind,
        candidate.confidence,
        candidate.markers.join(", "),
        languages,
        if candidate.already_registered {
            "It is already registered with Cortex."
        } else {
            "It is not registered yet; Cortex can create/attach a project for this folder after review."
        }
    )
}

fn managed_attachment_name(name: &str, canonical: &Path) -> String {
    let safe = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let material = canonical.to_string_lossy();
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in material.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}-{safe}")
}

fn is_text_attachment(extension: &str) -> bool {
    matches!(
        extension,
        "txt"
            | "md"
            | "rs"
            | "toml"
            | "json"
            | "yaml"
            | "yml"
            | "xml"
            | "csv"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "cc"
            | "cs"
            | "java"
            | "kt"
            | "kts"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "py"
            | "ps1"
            | "sh"
            | "bat"
            | "cmd"
            | "wgsl"
            | "glsl"
            | "vert"
            | "frag"
            | "ron"
            | "ini"
            | "cfg"
            | "log"
    )
}

fn conversation_title(prompt: &str) -> String {
    let compact = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title = compact.chars().take(56).collect::<String>();
    if compact.chars().count() > 56 {
        title.push('…');
    }
    if title.trim().is_empty() {
        "New conversation".into()
    } else {
        title
    }
}

fn verified_mutation_response_text(value: &Value) -> String {
    let files = value.get("transaction_files");
    let mut ordered = Vec::<String>::new();
    let mut seen = std::collections::BTreeSet::new();

    if let Some(files) = files {
        for key in ["created", "touched"] {
            if let Some(items) = files.get(key).and_then(Value::as_array) {
                for item in items {
                    if let Some(path) = item.as_str() {
                        if seen.insert(path.to_string()) {
                            ordered.push(path.replace('\\', "/"));
                        }
                    }
                }
            }
        }
    }

    let repair_attempts = value
        .get("repair_attempts")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let runtime_verified = value
        .get("runtime_verified")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let intent_required = value
        .pointer("/intent_acceptance/required")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let intent_verified = value
        .pointer("/intent_acceptance/success")
        .and_then(Value::as_bool)
        .unwrap_or(!intent_required);

    let mut text =
        "Done — Cortex applied the requested project changes and the detected project quality gate passed."
            .to_string();

    if !ordered.is_empty() {
        text.push_str("\n\nChanged files");
        for path in ordered.iter().take(12) {
            text.push_str("\n- `");
            text.push_str(path);
            text.push('`');
        }
        if ordered.len() > 12 {
            text.push_str(&format!("\n- … {} additional file(s)", ordered.len() - 12));
        }
    }

    if let Some(stages) = value
        .pointer("/verification/stages")
        .and_then(Value::as_array)
    {
        if !stages.is_empty() {
            text.push_str("\n\nVerification");
            for stage in stages.iter().take(8) {
                let capability = stage
                    .get("capability")
                    .and_then(Value::as_str)
                    .unwrap_or("check");
                let passed = stage
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                text.push_str(&format!(
                    "\n- {}: {}",
                    capability,
                    if passed { "passed" } else { "failed" }
                ));
            }
        }
    }

    if intent_required {
        text.push_str(&format!(
            "\n- requested project intent: {}",
            if intent_verified {
                "verified"
            } else {
                "failed"
            }
        ));
    }
    if runtime_verified {
        text.push_str("\n- runtime launch: verified");
    }

    if repair_attempts > 0 {
        text.push_str(&format!(
            "\n\nCortex used {repair_attempts} bounded repair continuation attempt(s) before reaching GREEN."
        ));
    }

    text.push_str(
        "\n\nDetailed code changes are available below and in Workbench; raw tool telemetry remains in Activity.",
    );
    text
}

fn response_text(value: &Value) -> String {
    value
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("result")
                .and_then(|result| result.get("text"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            value
                .get("agent")
                .and_then(|agent| agent.get("text"))
                .and_then(Value::as_str)
        })
        .map(str::to_string)
        .unwrap_or_else(|| {
            serde_json::to_string_pretty(value).unwrap_or_else(|_| "<Cortex response>".into())
        })
}

fn service_label(state: Option<&ServiceState>) -> &'static str {
    match state.map(|state| &state.status) {
        Some(ServiceStatus::Running) => "running",
        Some(ServiceStatus::Stale) => "stale",
        None => "offline",
    }
}

fn same_executable(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn native_models_unavailable_detail(models_root: &Path) -> String {
    format!(
        "Cortex Native Models are not configured: no GGUF text models were found under {} or the standard LM Studio model folders. Cortex itself remains available; place portable models under this drive's Models folder or select/configure another provider before sending an AI request.",
        models_root.display()
    )
}

fn sibling_model_host_executable() -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let directory = current
        .parent()
        .ok_or_else(|| "Cortex Desktop executable has no parent directory".to_string())?;
    #[cfg(windows)]
    let name = "cortex_model_host.exe";
    #[cfg(not(windows))]
    let name = "cortex_model_host";
    let candidate = directory.join(name);
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(format!(
            "Cortex native model-host executable was not found next to Cortex Desktop: {}",
            candidate.display()
        ))
    }
}

fn locate_lms_executable() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["lms.exe", "lms.cmd", "lms.bat"]
    } else {
        &["lms"]
    };
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            for name in names {
                let candidate = directory.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    #[cfg(windows)]
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        let profile = PathBuf::from(profile);
        for candidate in [
            profile.join(".lmstudio").join("bin").join("lms.exe"),
            profile.join(".lmstudio").join("bin").join("lms.cmd"),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn lms_command(executable: &Path) -> Command {
    #[cfg(windows)]
    {
        let is_wrapper = executable
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| {
                value.eq_ignore_ascii_case("cmd") || value.eq_ignore_ascii_case("bat")
            });
        if is_wrapper {
            let mut command = Command::new("cmd.exe");
            command.args(["/d", "/c"]).arg(executable);
            return command;
        }
    }
    Command::new(executable)
}

fn loopback_port(url: &str) -> Option<u16> {
    let rest = url
        .strip_prefix("http://127.0.0.1:")
        .or_else(|| url.strip_prefix("http://localhost:"))?;
    rest.split(['/', '?', '#']).next()?.parse::<u16>().ok()
}

fn sibling_cortex_executable() -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let directory = current
        .parent()
        .ok_or_else(|| "Cortex Desktop executable has no parent directory".to_string())?;
    #[cfg(windows)]
    let name = "cortex.exe";
    #[cfg(not(windows))]
    let name = "cortex";
    let candidate = directory.join(name);
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(format!(
            "standalone Cortex service executable was not found next to Cortex Desktop: {}",
            candidate.display()
        ))
    }
}

impl Drop for DesktopController {
    fn drop(&mut self) {
        if !self.owns_runtime_lifetime {
            return;
        }
        let _ = self.service.stop();
        if let Some(model_host) = &self.model_host {
            let _ = model_host.stop();
        }
        if self.lmstudio_started_by_cortex {
            if let Some(lms) = locate_lms_executable() {
                let mut command = lms_command(&lms);
                let _ = command
                    .args(["server", "stop"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_registration_requires_explicit_affirmative_exact_command() {
        assert!(is_library_registration_request("register AI"));
        assert!(is_library_registration_request(
            "please register project D:\\AI"
        ));
        assert_eq!(
            explicit_library_registration_target("register AI").as_deref(),
            Some("ai")
        );
        assert_eq!(
            explicit_library_registration_target(r"register \\?\D:\AI").as_deref(),
            Some(r"d:\ai")
        );
        assert!(!is_library_registration_request(
            "Do not register unrelated projects. Report PASS/FAIL evidence."
        ));
        assert!(!is_library_registration_request(
            "Keep the Vault scan cataloged only and do not register another project."
        ));
        assert!(!is_library_registration_request(
            "The acceptance project is hello3d; reject registration of unrelated candidates."
        ));
    }

    #[test]
    fn project_registration_bulk_selector_is_review_gated() {
        for selector in [
            "all",
            "all projects",
            "all candidates",
            "all high-confidence projects",
            "all high confidence projects",
        ] {
            assert!(is_bulk_library_registration_selector(selector));
        }
        assert!(!is_bulk_library_registration_selector("ai"));
    }

    #[test]
    fn project_unregistration_is_explicit_and_negative_prose_does_not_route() {
        assert!(is_library_unregistration_request("unregister AI"));
        assert!(is_library_unregistration_request("detach project D:\\AI"));
        assert!(!is_library_unregistration_request(
            "Do not unregister the active project."
        ));
    }

    #[test]
    fn standalone_project_requests_take_deterministic_new_project_path() {
        assert_eq!(
            developer_intent(
                "can we make a separate project i want to build a simple hello world render that is 3D"
            ),
            Some(DeveloperIntent::NewProject)
        );
        assert_eq!(
            developer_intent("create another project called Tiny Renderer"),
            Some(DeveloperIntent::NewProject)
        );
        assert_eq!(
            standalone_project_name("create a separate project named hello3d"),
            "hello3d"
        );
        assert_eq!(
            standalone_project_name("a graphical demo"),
            "cortex-project"
        );
        assert_eq!(
            standalone_project_name("create another project called Tiny Renderer"),
            "tiny-renderer"
        );
    }

    #[test]
    fn explicit_cpp_windows_request_selects_cmake_gui_scaffold() {
        let intent = infer_project_intent(
            "Create and finish a small native C++ Windows desktop app with CMake and launch it.",
        );
        assert_eq!(intent.language, "cpp");
        assert_eq!(intent.build_system, "cmake");
        assert_eq!(intent.platform, "windows");
        assert_eq!(intent.application_kind, "desktop_gui");
        assert_eq!(intent.scaffold, "cpp_cmake_windows_gui");
        assert!(intent.expected_window);
        assert!(intent.runtime_required);
    }

    #[test]
    fn project_intent_acceptance_rejects_green_rust_project_for_cpp_contract() {
        let root =
            std::env::temp_dir().join(format!("cortex-intent-negative-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join(".cortex")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"wrong_app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let intent = ProjectIntentContract {
            language: "cpp".into(),
            build_system: "cmake".into(),
            platform: "windows".into(),
            application_kind: "desktop_gui".into(),
            scaffold: "cpp_cmake_windows_gui".into(),
            expected_window: true,
            runtime_required: true,
        };
        write_project_intent_contract(&root, &intent).unwrap();

        let acceptance = project_intent_acceptance(&root).unwrap();
        assert_eq!(
            acceptance.get("success").and_then(Value::as_bool),
            Some(false)
        );
        let diagnostics = acceptance
            .get("diagnostics")
            .and_then(Value::as_array)
            .unwrap();
        assert!(!diagnostics.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cpp_cmake_gui_scaffold_satisfies_static_intent_contract() {
        let root =
            std::env::temp_dir().join(format!("cortex-intent-positive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let intent = ProjectIntentContract {
            language: "cpp".into(),
            build_system: "cmake".into(),
            platform: "windows".into(),
            application_kind: "desktop_gui".into(),
            scaffold: "cpp_cmake_windows_gui".into(),
            expected_window: true,
            runtime_required: true,
        };
        create_standalone_cpp_cmake(&root, "native_test", true).unwrap();
        write_project_intent_contract(&root, &intent).unwrap();

        let acceptance = project_intent_acceptance(&root).unwrap();
        assert_eq!(
            acceptance.get("success").and_then(Value::as_bool),
            Some(true)
        );
        assert!(root.join("CMakeLists.txt").is_file());
        assert!(root.join("src/main.cpp").is_file());
        assert!(root.join(".cortex/project.json").is_file());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn api_mismatch_gate_selects_reconstruction_and_preserves_missing_package() {
        let gate = json!({
            "success": false,
            "stages": [{
                "capability": "validate",
                "result": {
                    "cortex_quality": {
                        "current": {
                            "errors": 12,
                            "likely_api_version_mismatch": true
                        },
                        "missing_dependency_grounding": ["wgpu"]
                    },
                    "diagnostics": [{
                        "file": "src/main.rs",
                        "message": "unresolved import `wgpu::SurfaceOutputView`",
                        "rendered": "cannot find `BackendFlags` in `wgpu`"
                    }]
                }
            }]
        });
        let signal = repair_signal_from_gate(&gate);
        assert!(signal.likely_api_version_mismatch);
        assert_eq!(signal.errors_from_same_dependency, 12);
        assert_eq!(signal.affected_files, 1);
        assert!(signal.compact_module);
        assert_eq!(signal.missing_dependency_grounding, vec!["wgpu"]);
        assert_eq!(
            choose_repair_strategy(&signal),
            RepairStrategy::Reconstruction
        );
        let queries = diagnostic_grounding_queries(&gate);
        assert!(queries.iter().any(|query| query == "SurfaceOutputView"));
        assert!(queries.iter().any(|query| query == "BackendFlags"));
    }

    #[test]
    fn cargo_project_names_are_sanitized() {
        assert_eq!(sanitize_cargo_project_name("Hello 3D!"), "hello-3d");
        assert_eq!(sanitize_cargo_project_name("123"), "cortex-project");
        assert_eq!(sanitize_cargo_project_name("my_project"), "my_project");
    }

    #[test]
    fn provider_recovery_errors_are_classified_without_hiding_other_failures() {
        assert!(is_recoverable_provider_error(
            "connect failed: target machine actively refused it (os error 10061)"
        ));
        assert!(is_recoverable_provider_error(
            "LM Studio has no available text model"
        ));
        assert!(!is_recoverable_provider_error("cargo check failed"));
    }

    #[test]
    fn provider_status_detail_is_actionable() {
        let detail = provider_status_detail(&json!({
            "state": "online_no_model",
            "selected_model": null,
            "detail": "LM Studio is online but no model is currently loaded."
        }));
        assert!(detail.contains("online_no_model"));
        assert!(detail.contains("Selected text model: none"));
        assert!(detail.contains("no model is currently loaded"));
    }

    #[test]
    fn provider_retry_words_are_narrow_and_deterministic() {
        assert!(is_provider_retry_request("retry provider"));
        assert!(is_provider_retry_request("Resume Provider"));
        assert!(is_provider_retry_request("retry last provider request"));
        assert!(!is_provider_retry_request("continue"));
        assert!(!is_provider_retry_request("retry"));
        assert!(!is_provider_retry_request("resume"));
        assert!(!is_provider_retry_request(
            "continue implementing the renderer"
        ));
    }

    #[test]
    fn conversation_export_preserves_full_persisted_history() {
        let conversation = Conversation {
            schema_version: 1,
            id: "export-test".into(),
            title: "Export Test".into(),
            workspace_root: PathBuf::from("C:/project"),
            created_unix_ms: 1,
            updated_unix_ms: 2,
            archived: false,
            messages: vec![
                cortex_conversation::ConversationMessage {
                    id: "m1".into(),
                    role: ConversationRole::User,
                    content: "first message".into(),
                    created_unix_ms: 1,
                    feedback_score: 0,
                    revision: 1,
                    parent_message_id: None,
                    superseded_by: None,
                },
                cortex_conversation::ConversationMessage {
                    id: "m2".into(),
                    role: ConversationRole::Assistant,
                    content: "last message".into(),
                    created_unix_ms: 2,
                    feedback_score: 0,
                    revision: 1,
                    parent_message_id: None,
                    superseded_by: None,
                },
            ],
        };
        let txt = render_conversation_export(&conversation, "txt").unwrap();
        let md = render_conversation_export(&conversation, "md").unwrap();
        assert!(txt.contains("first message"));
        assert!(txt.contains("last message"));
        assert!(md.contains("## USER"));
        assert!(md.contains("## CORTEX"));
        assert_eq!(safe_export_stem("Hello / Chat?"), "Hello_Chat");
    }

    #[test]
    fn brainstorming_stays_in_chat_instead_of_consuming_agent_turn_budget() {
        assert_eq!(developer_intent("can we add new features?"), None);
        assert_eq!(
            developer_intent("brainstorm new features for this project"),
            None
        );
    }

    #[test]
    fn context_index_requests_are_deterministic() {
        assert!(is_context_index_request("can we index this project?"));
        assert!(is_context_index_request("rebuild context index"));
        assert!(!is_context_index_request("what features can we add?"));
    }

    #[test]
    fn developer_router_handles_natural_project_conversation() {
        assert_eq!(
            developer_intent("can we add new features?"),
            None,
            "broad feature brainstorming should stay in normal project chat"
        );
        assert_eq!(
            developer_intent("brainstorm new features for this project"),
            None,
            "brainstorming should not consume the bounded project-agent tool loop"
        );
        assert_eq!(
            developer_intent("inspect this project and tell me what is wrong"),
            Some(DeveloperIntent::ReadOnly),
            "explicit inspection remains a read-only agent request"
        );
        assert_eq!(
            developer_intent("audit the current project architecture"),
            Some(DeveloperIntent::ReadOnly),
            "explicit audit remains a read-only agent request"
        );
    }

    #[test]
    fn explicit_clock_completion_request_routes_to_real_project_execution() {
        let prompt = "can we finish up this simple clock and build it to prove this loop works for you to be able to code?";
        assert_eq!(
            route_developer_intent(prompt, true),
            Some(DeveloperIntent::Code),
            "the real user blocker prompt must never fall through to tool-less chat"
        );
        assert!(mutation_execution_retry_required(
            "Cortex apply completed without a successful project-file mutation tool call."
        ));
    }

    #[test]
    fn build_named_artifact_routes_to_developer_and_launch_contract() {
        let prompt = "build the clock and launch it so I can inspect it";
        assert_eq!(
            direct_project_operation(prompt),
            None,
            "artifact implementation language must not be swallowed by the raw build command runner"
        );
        assert_eq!(
            route_developer_intent(prompt, true),
            Some(DeveloperIntent::Code)
        );
        assert!(prompt_requests_launch(prompt));
        assert_eq!(
            direct_project_operation("build project"),
            Some(ProjectOperation::Build),
            "explicit project-operation commands should keep the fast command-runner path"
        );
    }

    #[test]
    fn project_chat_followups_inherit_authoritative_workspace_context() {
        for prompt in [
            "finish up",
            "complete it",
            "wrap this up",
            "get this working",
        ] {
            assert!(matches!(
                route_developer_intent(prompt, true),
                Some(DeveloperIntent::ContinueAndCode | DeveloperIntent::Code)
            ));
        }
    }

    #[test]
    fn developer_followups_inherit_active_project_context() {
        for prompt in [
            "why can you not fix this?",
            "please fix it",
            "that didn't work",
            "still broken",
            "try again",
            "it crashed",
        ] {
            assert_eq!(
                route_developer_intent(prompt, true),
                Some(DeveloperIntent::Repair),
                "repair phrase should inherit active project context: {prompt}"
            );
        }

        for prompt in [
            "go ahead and do it",
            "wire it in",
            "hook this up",
            "clean this up",
            "use this instead",
            "same thing here",
            "polish this",
        ] {
            assert_eq!(
                route_developer_intent(prompt, true),
                Some(DeveloperIntent::Code),
                "implementation phrase should inherit active project context: {prompt}"
            );
        }

        for prompt in [
            "continue",
            "keep working",
            "finish this",
            "do the rest",
            "get us to the next build point",
        ] {
            assert_eq!(
                route_developer_intent(prompt, true),
                Some(DeveloperIntent::ContinueAndCode),
                "continuation phrase should inherit active project context: {prompt}"
            );
        }

        for prompt in ["why is this happening?", "what's wrong?", "explain this"] {
            assert_eq!(
                route_developer_intent(prompt, true),
                Some(DeveloperIntent::ReadOnly),
                "diagnostic question should stay read-only until the user asks for a fix: {prompt}"
            );
        }

        assert_eq!(
            route_developer_intent("why can you not fix this?", false),
            None,
            "pronoun-only followups must not hijack unrelated general chat without active developer context"
        );
        assert_eq!(
            route_developer_intent("retry", true),
            Some(DeveloperIntent::Repair)
        );
        assert_eq!(
            route_developer_intent("resume", true),
            Some(DeveloperIntent::ContinueAndCode)
        );
    }

    #[test]
    fn deictic_followups_stay_in_durable_developer_context() {
        for prompt in [
            "carry on",
            "continue from there",
            "do that too",
            "same as before",
            "go with that",
            "make it like that",
        ] {
            assert!(
                matches!(
                    route_developer_intent(prompt, true),
                    Some(DeveloperIntent::ContinueAndCode | DeveloperIntent::Code)
                ),
                "{prompt}"
            );
        }
    }

    #[test]
    fn failure_referents_route_to_repair_in_project_context() {
        for prompt in [
            "that failed",
            "why did that fail",
            "fix whatever failed",
            "repair whatever is broken",
        ] {
            assert_eq!(
                route_developer_intent(prompt, true),
                Some(DeveloperIntent::Repair),
                "{prompt}"
            );
        }
    }

    #[test]
    fn natural_handoff_approval_and_rejection_phrases_are_supported() {
        for prompt in [
            "yes",
            "go ahead",
            "sounds good",
            "apply that",
            "ship it",
            "let's do it",
        ] {
            assert!(
                is_handoff_yes(prompt),
                "approval phrase should be accepted: {prompt}"
            );
        }
        for prompt in [
            "no",
            "cancel that",
            "stop",
            "never mind",
            "leave it",
            "don't change that",
        ] {
            assert!(
                is_handoff_no(prompt),
                "rejection phrase should be accepted: {prompt}"
            );
        }
    }

    #[test]
    fn tail_chars_preserves_recent_context() {
        assert_eq!(tail_chars("abcdef", 4), "cdef");
        assert_eq!(tail_chars("abc", 10), "abc");
    }

    #[test]
    fn readable_message_removes_html_breaks_and_markdown_table_noise() {
        let source = "**Overview**<br>| Category | Detail |\n|---|---|\n| Apps | 5 |";
        let rendered = readable_message(source);
        assert!(rendered.contains("Overview"));
        assert!(rendered.contains("Apps: 5"));
        assert!(!rendered.contains("<br>"));
        assert!(!rendered.contains("|---|"));
    }

    #[test]
    fn human_bytes_scales_storage_values() {
        assert_eq!(human_bytes(512), "512 B");
        assert!(human_bytes(1024 * 1024).contains("MiB"));
        assert!(human_bytes(1024_u64.pow(4)).contains("TiB"));
    }

    #[test]
    fn default_layout_enforces_single_owner_workspace_authority() {
        let layout = DesktopLayout::default();
        assert_eq!(
            layout.center,
            vec![PaneKind::Chat, PaneKind::Workspace, PaneKind::Vault]
        );
        assert_eq!(layout.right, vec![PaneKind::Context]);
        assert!(layout.bottom.contains(&PaneKind::Build));
        assert!(layout.bottom.contains(&PaneKind::Tasks));
    }
    #[test]
    fn unified_diff_splitter_preserves_file_boundaries() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-x\n+y\n";
        let files = split_unified_diff_by_file(diff);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, "a.rs");
        assert_eq!(files[1].0, "b.rs");
    }
    #[test]
    fn windows_loopback_resets_are_recoverable_provider_failures() {
        let error =
            "An existing connection was forcibly closed by the remote host. (os error 10054)";
        assert!(is_provider_transport_reset(error));
        assert!(is_recoverable_provider_error(error));
    }

    #[test]
    fn project_switch_uses_runtime_preserving_context_rebind() {
        let source = include_str!("lib.rs");
        assert!(source.contains("replace_workspace_context"));
        assert!(source.contains("do NOT tear down the global"));
        assert!(source.contains("provider / ready on demand"));
    }

    #[test]
    fn h63_two_turn_provider_probe_is_certified() {
        let probe = json!({
            "first_tool_calls": [
                {"name":"workspace.status","arguments":{},"call_id":"call-1"}
            ],
            "second_tool_calls": [
                {"name":"workspace.status","arguments":{},"call_id":"call-2"}
            ],
            "multi_turn_tool_continuity": true
        });
        assert!(DesktopController::structured_tool_probe_proves_continuity(
            &probe
        ));
    }

    #[test]
    fn h63_probe_requires_both_structured_tool_turns() {
        let probe = json!({
            "first_tool_calls": [
                {"name":"workspace.status","arguments":{},"call_id":"call-1"}
            ],
            "second_tool_calls": [],
            "multi_turn_tool_continuity": true
        });
        assert!(!DesktopController::structured_tool_probe_proves_continuity(
            &probe
        ));
    }

    #[test]
    fn legacy_single_turn_provider_probe_remains_supported() {
        let probe = json!({
            "tool_calls": [
                {"name":"workspace.status","arguments":{},"call_id":"call-legacy"}
            ]
        });
        assert!(DesktopController::structured_tool_probe_proves_continuity(
            &probe
        ));
    }

    #[test]
    fn runtime_certification_conversations_are_classified_as_internal() {
        let mut conversation = Conversation {
            schema_version: 1,
            id: "cert-chat".into(),
            title: RUNTIME_CERTIFICATION_TITLE.into(),
            workspace_root: PathBuf::from(r"C:\fixture"),
            created_unix_ms: 1,
            updated_unix_ms: 1,
            archived: false,
            messages: Vec::new(),
        };
        assert!(is_internal_runtime_certification_conversation(
            &conversation
        ));

        conversation.title = "Cortex runtime certification probe. Reply exactly with C...".into();
        assert!(is_internal_runtime_certification_conversation(
            &conversation
        ));

        conversation.title = "Normal project work".into();
        assert!(!is_internal_runtime_certification_conversation(
            &conversation
        ));
    }

    #[test]
    fn runtime_certification_classifier_does_not_hide_normal_cortex_chat() {
        let conversation = Conversation {
            schema_version: 1,
            id: "normal-chat".into(),
            title: "Cortex integration work".into(),
            workspace_root: PathBuf::from(r"C:\fixture"),
            created_unix_ms: 1,
            updated_unix_ms: 1,
            archived: false,
            messages: Vec::new(),
        };
        assert!(!is_internal_runtime_certification_conversation(
            &conversation
        ));
    }

    #[test]
    fn affirmative_proceed_followups_remain_in_developer_execution() {
        for prompt in [
            "yes proceed",
            "yes, proceed",
            "please proceed",
            "yes go ahead",
            "okay proceed",
            "proceed with the fix",
        ] {
            assert_eq!(
                route_developer_intent(prompt, true),
                Some(DeveloperIntent::ContinueAndCode),
                "execution affirmation should stay in Developer mode: {prompt}"
            );
        }
    }

    #[test]
    fn verified_mutation_reply_uses_quality_evidence_not_stale_approval_prose() {
        let value = json!({
            "agent": {
                "text": "Should I proceed with fixing the formatting and rebuilding now?"
            },
            "transaction_files": {
                "created": [],
                "touched": ["src/main.rs", "Cargo.toml"]
            },
            "verification": {
                "stages": [
                    {"capability": "format", "success": true},
                    {"capability": "validate", "success": true},
                    {"capability": "build", "success": true}
                ]
            },
            "intent_acceptance": {
                "required": true,
                "success": true
            },
            "quality_verified": true,
            "compile_verified": true,
            "runtime_verified": true,
            "repair_attempts": 2
        });
        let rendered = verified_mutation_response_text(&value);
        assert!(rendered.contains("quality gate passed"));
        assert!(rendered.contains("Changed files"));
        assert!(rendered.contains("src/main.rs"));
        assert!(rendered.contains("Cargo.toml"));
        assert!(rendered.contains("Verification"));
        assert!(rendered.contains("validate: passed"));
        assert!(rendered.contains("requested project intent: verified"));
        assert!(rendered.contains("runtime launch: verified"));
        assert!(!rendered.contains("Should I proceed"));
    }

    #[test]
    fn implementation_go_ahead_phrase_keeps_established_code_route() {
        assert_eq!(
            route_developer_intent("go ahead and do it", true),
            Some(DeveloperIntent::Code)
        );
        assert_eq!(
            route_developer_intent("yes proceed", true),
            Some(DeveloperIntent::ContinueAndCode)
        );
    }

    #[test]
    fn explicit_recovery_snapshot_commands_remain_direct_commands() {
        for prompt in [
            "Create a recovery snapshot for this project.",
            "Make a git checkpoint before I change things.",
            "Snapshot the project now.",
            "Take a snapshot of this workspace.",
        ] {
            assert!(
                is_recovery_snapshot_request(prompt),
                "explicit recovery command should remain direct: {prompt}"
            );
        }
    }

    #[test]
    fn project_quality_checkpoint_language_is_not_a_recovery_snapshot() {
        for prompt in [
            "Run the project checkpoint.",
            "Repair the project, run its checkpoint, build it, and launch it.",
            "Finish this project and run its highest-authority project checkpoint or quality gate.",
        ] {
            assert!(
                !is_recovery_snapshot_request(prompt),
                "project quality language must continue into Developer/Repair: {prompt}"
            );
        }
    }

    #[test]
    fn hello3d_acceptance_prompt_is_not_stolen_by_recovery_routing() {
        let prompt = r#"Finish this project.

It should be a small Rust/wgpu desktop application that opens a working window,
renders something simple, and displays the current local time in the window title,
updating once per second.

Inspect the existing project first. Preserve wgpu where appropriate. Repair the project,
run its highest-authority project checkpoint or quality gate, fix failures until it passes,
build it, launch the actual built executable, verify it remains running and behaves as
requested, and only claim completion after the project is verified.

Do not stop after explaining the errors. Complete the work."#;

        assert!(!is_recovery_snapshot_request(prompt));
        assert!(
            route_developer_intent(prompt, true).is_some(),
            "the Hello3D acceptance prompt must remain eligible for Developer execution"
        );
    }
    #[test]
    fn m11u2_repair_capsule_bounds_diagnostics() {
        let gate = json!({
            "success": false,
            "stages": [{
                "capability": "validate",
                "tool": "build.project_validate",
                "success": false,
                "result": {
                    "diagnostics": (0..20)
                        .map(|index| json!({"message": format!("e{index}")}))
                        .collect::<Vec<_>>(),
                    "cortex_quality": {"missing_dependency_grounding": ["wgpu"]}
                }
            }]
        });
        let capsule = DesktopController::m11u2_repair_gate_capsule(&gate);
        assert_eq!(
            capsule
                .pointer("/stages/0/diagnostics")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(12)
        );
    }

    #[test]
    fn m11u2_runtime_intent_detects_hello3d_acceptance_prompt() {
        let prompt = concat!(
            "Build the actual executable, launch it, open a working native window, ",
            "and keep updating the time in the window title once per second."
        );
        assert!(DesktopController::m11u2_prompt_requests_runtime(prompt));
        assert!(DesktopController::m11u2_prompt_requests_window(prompt));
        assert!(DesktopController::m11u2_prompt_requests_title_change(
            prompt
        ));
    }

    #[test]
    fn m11u4h_controller_grounding_does_not_spend_repair_search_budget() {
        let source = include_str!("lib.rs");
        let forbidden = ["for query in queries.iter()", ".take(8)"].concat();
        assert!(
            !source.contains(&forbidden),
            "controller dependency grounding must not issue the old eight-search diagnostic storm"
        );
        assert!(source.contains("Exact package grounding is complete."));
    }

    #[test]
    fn m11u2_repair_prompt_is_compiler_cadenced() {
        let source = include_str!("lib.rs");
        assert!(source.contains("ONE SOURCE MUTATION PER COMPILER CYCLE"));
        assert!(source.contains("COMPILER REPAIR CAPSULE"));
        assert!(source.contains("Repair diagnostic convergence guard"));
    }

    #[test]
    fn m11u4_compiler_diagnostics_bind_project_relative_repair_targets() {
        let gate = json!({
            "success": false,
            "stages": [{
                "result": {
                    "diagnostics": [
                        {"level": "warning", "file": r"C:\fixture\src\lib.rs", "message": "warning"},
                        {"level": "error", "file": r"\\?\C:\fixture\src\main.rs", "message": "error"},
                        {"level": "error", "file": r"C:\fixture\src\main.rs", "message": "duplicate"},
                        {"level": "error", "file": r"C:\Users\name\.cargo\registry\src\crate.rs", "message": "external"}
                    ]
                }
            }]
        });
        assert_eq!(
            repair_targets_from_gate(&gate, Path::new(r"C:\fixture")),
            vec!["src/main.rs".to_string(), "src/lib.rs".to_string()]
        );
    }

    #[test]
    fn m11u4_absolute_diagnostic_outside_workspace_is_not_a_repair_target() {
        assert_eq!(
            normalize_repair_target(Path::new(r"C:\fixture"), r"C:\other-project\src\main.rs"),
            None
        );
        assert_eq!(
            normalize_repair_target(Path::new(r"C:\fixture"), r"src\main.rs"),
            Some("src/main.rs".to_string())
        );
    }

    #[test]
    fn m11u2_initial_repair_is_compiler_first() {
        let source = include_str!("lib.rs");
        assert!(source.contains("M11U2 COMPILER-FIRST REPAIR"));
        assert!(source.contains("Initial Repair baseline"));
        assert!(source.contains("m11u2_prepare_initial_repair_prompt"));
    }
    #[test]
    fn stor1_negative_scan_language_never_starts_a_catalog() {
        for prompt in [
            "Do not scan D: again",
            "Do not perform another drive scan",
            "Don't scan the Vault",
            "Continue without scanning",
            "No drive scan; continue Hello3D",
        ] {
            assert!(!is_library_scan_request(prompt), "{prompt}");
            assert!(!is_machine_scan_request(prompt), "{prompt}");
        }
        assert!(is_library_scan_request("scan vault"));
        assert!(is_library_scan_request("catalog storage"));
        assert!(is_machine_scan_request("scan my entire PC for projects"));
        assert!(is_machine_scan_request("catalog all drives"));
    }

    #[test]
    fn native_models_missing_is_degraded_configuration_not_process_crash_text() {
        let detail = native_models_unavailable_detail(Path::new(r"E:\Models"));
        assert!(detail.contains("not configured"));
        assert!(detail.contains("Cortex itself remains available"));
        assert!(detail.contains("Models"));
        assert!(!detail.contains("PID"));
    }
}
