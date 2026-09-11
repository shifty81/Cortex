use cortex_client::CortexClient as RuntimeClient;
use cortex_desktop_core::{DesktopBootstrap, DesktopController};
use cortex_protocol::CortexStreamEvent;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

pub use cortex_desktop_core::{
    DesktopChatBlock, DesktopFileEntry, DesktopFileListing, DesktopLibraryView, DesktopView,
    DesktopWorkbenchDocument,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CortexConnectionState {
    Disconnected,
    Connecting,
    Ready,
    Error,
}

impl Default for CortexConnectionState {
    fn default() -> Self {
        Self::Disconnected
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CortexCapability {
    pub id: String,
    pub label: String,
    pub risk: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CortexRequestKind {
    Chat,
    Inspect,
    Plan,
    Apply,
    Repair,
}

impl CortexRequestKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Inspect => "Inspect",
            Self::Plan => "Plan",
            Self::Apply => "Apply",
            Self::Repair => "Repair",
        }
    }

    #[must_use]
    pub const fn agent_mode(self) -> Option<&'static str> {
        match self {
            Self::Chat => None,
            Self::Inspect => Some("inspect"),
            Self::Plan => Some("plan"),
            Self::Apply => Some("apply"),
            Self::Repair => Some("repair"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CortexTaskKind {
    ProviderProbe,
    RefreshRepositoryProvider,
    RefreshRepositoryVaultAudit,
    CreateSafetyCheckpoint,
    PushRepositoryLocal,
    BuildCheck,
    RebuildContext,
    ScanLibrary,
    ScanStorageCatalog,
    ScanMachineCatalog,
}

impl CortexTaskKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ProviderProbe => "Provider probe",
            Self::RefreshRepositoryProvider => "Refresh repository provider",
            Self::RefreshRepositoryVaultAudit => "Refresh repository Vault audit",
            Self::CreateSafetyCheckpoint => "Create repository safety checkpoint",
            Self::PushRepositoryLocal => "Push repository local",
            Self::BuildCheck => "Cortex build check",
            Self::RebuildContext => "Rebuild project context index",
            Self::ScanLibrary => "Scan Cortex library",
            Self::ScanStorageCatalog => "Scan configured storage catalog preview",
            Self::ScanMachineCatalog => "Scan machine catalog preview",
        }
    }

    #[must_use]
    pub const fn supports_cooperative_cancel(self) -> bool {
        matches!(self, Self::ScanStorageCatalog | Self::ScanMachineCatalog)
    }
}

#[derive(Debug, Clone, Default)]
pub struct CortexConfigSnapshot {
    pub library_root: String,
    pub offsite_backup_root: String,
    pub service_port: u16,
    pub auto_start_service: bool,
    pub provider: String,
    pub native_model_host_port: u16,
    pub native_auto_bootstrap: bool,
    pub native_models_max: u8,
    pub lmstudio_auto_start: bool,
    pub lmstudio_url: String,
    pub comfyui_url: String,
    pub chat_model: String,
    pub tool_model: String,
    pub vision_model: String,
    pub embedding_model: String,
    pub theme: String,
    pub compact_tool_cards: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CortexRuntimeSnapshot {
    pub connection: CortexConnectionState,
    pub desktop_api_ready: bool,
    pub service_registered: bool,
    pub provider_recovery_pending: bool,
    pub conversation_count: usize,
    pub health_json: String,
    pub models_json: String,
    pub provider_json: String,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct CortexStreamFrame {
    pub request_id: String,
    pub kind: String,
    pub phase: String,
    pub message: String,
    pub text_delta: String,
    pub elapsed_ms: u64,
    pub data: String,
}

impl From<CortexStreamEvent> for CortexStreamFrame {
    fn from(event: CortexStreamEvent) -> Self {
        Self {
            request_id: event.request_id,
            kind: format!("{:?}", event.kind).to_ascii_lowercase(),
            phase: event.phase,
            message: event.message,
            text_delta: event.text_delta,
            elapsed_ms: event.elapsed_ms,
            data: pretty_value(&event.data),
        }
    }
}

#[derive(Debug, Clone)]
pub enum CortexHostEvent {
    Stream(CortexStreamFrame),
    Completed { label: String },
    Failed { label: String, error: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CortexParityState {
    Integrated,
    ForgeAuthority,
    DonorReady,
}

impl CortexParityState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Integrated => "integrated",
            Self::ForgeAuthority => "forge_authority",
            Self::DonorReady => "donor_ready",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CortexParityItem {
    pub id: &'static str,
    pub label: &'static str,
    pub state: CortexParityState,
    pub authority: &'static str,
}

#[must_use]
pub fn cortex_parity_items() -> Vec<CortexParityItem> {
    use CortexParityState::{DonorReady, ForgeAuthority, Integrated};
    vec![
        parity("chat.streaming", "Streaming project chat", Integrated, "Cortex"),
        parity("agent.inspect", "Inspect agent", Integrated, "Cortex"),
        parity("agent.plan", "Plan agent", Integrated, "Cortex"),
        parity("agent.apply", "Apply agent", Integrated, "Cortex"),
        parity("agent.repair", "Repair agent", Integrated, "Cortex"),
        parity("chat.conversations", "Persistent conversations", Integrated, "Cortex"),
        parity("chat.export", "Conversation export", Integrated, "Cortex"),
        parity("chat.feedback", "Response feedback", Integrated, "Cortex"),
        parity("chat.redo", "Response revision / redo", Integrated, "Cortex"),
        parity("files.browser", "Project/library file browser", Integrated, "Cortex"),
        parity("files.workbench", "Read-only workbench document view", Integrated, "Cortex"),
        parity("context.index", "Context index show/rebuild", Integrated, "Cortex"),
        parity("provider.status", "Provider/model health", Integrated, "Cortex"),
        parity("provider.recovery", "Provider probe/recovery", Integrated, "Cortex"),
        parity("vault.search", "Cortex library/Vault search", Integrated, "Cortex"),
        parity("vault.scan", "Library/storage/machine catalog scans", Integrated, "Cortex"),
        parity("activity.jobs", "Activity/jobs/system telemetry", Integrated, "Cortex"),
        parity("review.changes", "Project review/change state", Integrated, "Cortex"),
        parity("repo.checkpoint", "Repository safety checkpoint", Integrated, "Cortex"),
        parity("repo.push_local", "Repository local push", Integrated, "Cortex"),
        parity("build.check", "Cortex project build check", Integrated, "Cortex"),
        parity("workspace.switch", "Active project/workspace switching", ForgeAuthority, "Forge"),
        parity("storage.authority", "Vault root/offsite storage authority", ForgeAuthority, "Forge"),
        parity("source.git", "GitHub + Forge Repository/Internal Git", ForgeAuthority, "Forge"),
        parity("patch.intake", "Patch/update intake and lineage", ForgeAuthority, "Forge"),
        parity("artifacts.central", "Artifact Central lifecycle", ForgeAuthority, "Forge"),
        parity("ide.edit", "Writable IDE/editor file mutations", ForgeAuthority, "Forge"),
        parity("chat.attachments", "Rich chat attachments/images", DonorReady, "Cortex donor"),
        parity("review.rich_diff", "Native rich diff/review widgets", DonorReady, "Cortex donor"),
        parity("ui.accessibility", "Native keyboard/accessibility parity", DonorReady, "Cortex donor"),
    ]
}

fn parity(
    id: &'static str,
    label: &'static str,
    state: CortexParityState,
    authority: &'static str,
) -> CortexParityItem {
    CortexParityItem {
        id,
        label,
        state,
        authority,
    }
}

pub struct CortexWorkspaceHost {
    root: PathBuf,
    controller: DesktopController,
    view: DesktopView,
    runtime: CortexRuntimeSnapshot,
    event_tx: Sender<CortexHostEvent>,
    event_rx: Receiver<CortexHostEvent>,
    busy: bool,
    active_label: String,
    cancel_flag: Arc<AtomicBool>,
    last_error: String,
}

impl CortexWorkspaceHost {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = canonical_or_original(root.as_ref());
        let mut controller = DesktopController::open(&root)?;
        assert_project_binding(&root, controller.workspace().root())?;
        let view = controller.view();
        let mut runtime = probe_runtime(&root);
        runtime.provider_recovery_pending = controller.provider_recovery_pending();
        let (event_tx, event_rx) = mpsc::channel();

        Ok(Self {
            root,
            controller,
            view,
            runtime,
            event_tx,
            event_rx,
            busy: false,
            active_label: String::new(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            last_error: String::new(),
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn view(&self) -> &DesktopView {
        &self.view
    }

    #[must_use]
    pub fn runtime(&self) -> &CortexRuntimeSnapshot {
        &self.runtime
    }

    #[must_use]
    pub fn busy(&self) -> bool {
        self.busy
    }

    #[must_use]
    pub fn active_label(&self) -> &str {
        &self.active_label
    }

    #[must_use]
    pub fn last_error(&self) -> &str {
        &self.last_error
    }

    pub fn refresh(&mut self) -> Result<(), String> {
        assert_project_binding(&self.root, self.controller.workspace().root())?;
        self.controller.sync_pending_state()?;
        self.controller.refresh_overview()?;
        self.refresh_view_only();
        self.runtime = probe_runtime(&self.root);
        self.runtime.provider_recovery_pending = self.controller.provider_recovery_pending();
        self.last_error.clear();
        Ok(())
    }

    pub fn config_snapshot(&self) -> Result<CortexConfigSnapshot, String> {
        let settings = self.controller.settings().clone();
        Ok(CortexConfigSnapshot {
            library_root: self
                .controller
                .library_root()?
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            offsite_backup_root: self
                .controller
                .offsite_backup_root()?
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            service_port: settings.service_port,
            auto_start_service: settings.auto_start_service,
            provider: settings.provider,
            native_model_host_port: settings.native_model_host_port,
            native_auto_bootstrap: settings.native_auto_bootstrap,
            native_models_max: settings.native_models_max,
            lmstudio_auto_start: settings.lmstudio_auto_start,
            lmstudio_url: settings.lmstudio_url,
            comfyui_url: settings.comfyui_url,
            chat_model: settings.chat_model.unwrap_or_default(),
            tool_model: settings.tool_model.unwrap_or_default(),
            vision_model: settings.vision_model.unwrap_or_default(),
            embedding_model: settings.embedding_model.unwrap_or_default(),
            theme: settings.theme,
            compact_tool_cards: settings.compact_tool_cards,
        })
    }

    pub fn update_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        self.require_idle()?;
        if matches!(key.trim(), "library_root" | "offsite_backup_root") {
            return Err(
                "Vault/library storage authority is Forge-owned in the normalized host; change it from Forge Vault/Settings, not the Cortex tab."
                    .to_owned(),
            );
        }
        self.controller.update_setting(key, value)?;
        self.refresh()
    }

    pub fn new_conversation(&mut self) -> Result<(), String> {
        self.require_idle()?;
        self.controller.new_conversation()?;
        self.refresh_view_only();
        Ok(())
    }

    pub fn archive_current_conversation(&mut self) -> Result<(), String> {
        self.require_idle()?;
        self.controller.archive_current_conversation()?;
        self.refresh_view_only();
        Ok(())
    }

    pub fn select_conversation(&mut self, index: usize) -> Result<(), String> {
        self.require_idle()?;
        self.controller.select_conversation(index)?;
        self.refresh_view_only();
        Ok(())
    }

    pub fn export_conversation(&mut self, index: usize, format: &str) -> Result<String, String> {
        self.require_idle()?;
        self.controller
            .export_conversation(index, format)
            .map(|path| path.display().to_string())
    }

    pub fn export_project_conversations(&mut self, format: &str) -> Result<String, String> {
        self.require_idle()?;
        self.controller
            .export_project_conversations(format)
            .map(|path| path.display().to_string())
    }

    pub fn rate_message(&mut self, message_id: &str, score: i8) -> Result<(), String> {
        self.require_idle()?;
        self.controller.rate_response(message_id, score)?;
        self.refresh_view_only();
        Ok(())
    }

    pub fn browse_files(
        &self,
        library_scope: bool,
        relative_directory: &str,
    ) -> Result<DesktopFileListing, String> {
        self.controller
            .browse_files(library_scope, relative_directory)
    }

    pub fn open_workbench_file(
        &mut self,
        relative_path: &str,
    ) -> Result<DesktopWorkbenchDocument, String> {
        self.controller.open_workbench_file(relative_path)
    }

    pub fn show_files(&mut self, query: &str) -> Result<(), String> {
        self.controller.show_files(query)?;
        self.refresh_view_only();
        Ok(())
    }

    pub fn show_changes(&mut self) -> Result<(), String> {
        self.controller.show_changes()?;
        self.refresh_view_only();
        Ok(())
    }

    pub fn search_vault(&mut self, query: &str) -> Result<(), String> {
        self.controller.search_vault(query)?;
        self.refresh_view_only();
        Ok(())
    }

    pub fn show_artifacts(&mut self) -> Result<(), String> {
        self.controller.show_artifacts()?;
        self.refresh_view_only();
        Ok(())
    }

    pub fn show_tasks(&mut self) -> Result<(), String> {
        self.controller.show_tasks()?;
        self.refresh_view_only();
        Ok(())
    }

    pub fn show_settings(&mut self) -> Result<(), String> {
        self.controller.show_settings()?;
        self.refresh_view_only();
        Ok(())
    }

    pub fn show_provider_status(&mut self) -> Result<(), String> {
        self.controller.show_provider_status()?;
        self.refresh_view_only();
        Ok(())
    }

    pub fn show_context_index(&mut self) -> Result<(), String> {
        self.controller.show_context_index()?;
        self.refresh_view_only();
        Ok(())
    }

    pub fn cancel_pending_provider_request(&mut self) -> Result<bool, String> {
        let cancelled = self.controller.cancel_pending_provider_request()?;
        self.refresh_view_only();
        Ok(cancelled)
    }

    pub fn start_request(
        &mut self,
        kind: CortexRequestKind,
        prompt: &str,
    ) -> Result<(), String> {
        self.require_idle()?;
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err("Cortex request cannot be empty".to_owned());
        }

        let mut worker = self.controller.fork_for_background()?;
        let tx = self.event_tx.clone();
        let prompt = prompt.to_owned();
        let label = kind.label().to_owned();
        self.begin_background(&label);

        thread::spawn(move || {
            let stream_tx = tx.clone();
            let mut on_event = move |event: CortexStreamEvent| {
                let _ = stream_tx.send(CortexHostEvent::Stream(event.into()));
            };

            let result = match kind.agent_mode() {
                None => worker.send_chat_stream(&prompt, &mut on_event),
                Some(mode) => worker
                    .run_agent_stream(mode, &prompt, &mut on_event)
                    .map(|_| ()),
            };
            send_terminal(&tx, label, result);
        });

        Ok(())
    }

    pub fn start_task(&mut self, kind: CortexTaskKind) -> Result<(), String> {
        self.require_idle()?;
        let mut worker = self.controller.fork_for_background()?;
        let tx = self.event_tx.clone();
        let label = kind.label().to_owned();
        self.begin_background(&label);
        let cancel = self.cancel_flag.clone();

        thread::spawn(move || {
            let result = match kind {
                CortexTaskKind::ProviderProbe => {
                    let _ = worker.probe_provider_quiet();
                    Ok(())
                }
                CortexTaskKind::RefreshRepositoryProvider => {
                    worker.refresh_repository_provider()
                }
                CortexTaskKind::RefreshRepositoryVaultAudit => {
                    worker.refresh_repository_vault_audit()
                }
                CortexTaskKind::CreateSafetyCheckpoint => {
                    worker.create_repository_safety_checkpoint()
                }
                CortexTaskKind::PushRepositoryLocal => worker.push_repository_local(),
                CortexTaskKind::BuildCheck => worker.run_build_check(),
                CortexTaskKind::RebuildContext => worker
                    .rebuild_context_index(50_000, 512 * 1024)
                    .map(|_| ()),
                CortexTaskKind::ScanLibrary => worker.scan_library().map(|_| ()),
                CortexTaskKind::ScanStorageCatalog => {
                    let mut should_cancel = || cancel.load(Ordering::Relaxed);
                    worker
                        .scan_storage_catalog_preview(&mut should_cancel)
                        .map(|_| ())
                }
                CortexTaskKind::ScanMachineCatalog => {
                    let mut should_cancel = || cancel.load(Ordering::Relaxed);
                    worker
                        .scan_machine_catalog_preview(false, &mut should_cancel)
                        .map(|_| ())
                }
            };
            send_terminal(&tx, label, result);
        });

        Ok(())
    }

    pub fn start_ingest_paths(&mut self, paths: Vec<String>) -> Result<(), String> {
        self.require_idle()?;
        let paths = paths
            .into_iter()
            .map(|path| path.trim().to_owned())
            .filter(|path| !path.is_empty())
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return Err("No Cortex ingest paths were supplied".to_owned());
        }
        let mut worker = self.controller.fork_for_background()?;
        let tx = self.event_tx.clone();
        let label = format!("Ingest {} path(s)", paths.len());
        self.begin_background(&label);

        thread::spawn(move || {
            let result = worker.ingest_paths(&paths);
            send_terminal(&tx, label, result);
        });
        Ok(())
    }

    pub fn start_redo(
        &mut self,
        message_id: String,
        instructions: String,
    ) -> Result<(), String> {
        self.require_idle()?;
        if message_id.trim().is_empty() {
            return Err("Cannot redo a Cortex response without a message ID".to_owned());
        }
        let mut worker = self.controller.fork_for_background()?;
        let tx = self.event_tx.clone();
        let label = "Redo Cortex response".to_owned();
        self.begin_background(&label);

        thread::spawn(move || {
            let result = worker.redo_response(&message_id, &instructions);
            send_terminal(&tx, label, result);
        });
        Ok(())
    }

    pub fn cancel_active(&mut self) -> Result<bool, String> {
        if !self.busy {
            return Ok(false);
        }
        self.cancel_flag.store(true, Ordering::Relaxed);
        let active_cancelled = self.controller.cancel_active_request().unwrap_or(false);
        let pending_cancelled = self
            .controller
            .cancel_pending_provider_request()
            .unwrap_or(false);
        Ok(active_cancelled || pending_cancelled || self.busy)
    }

    pub fn poll_events(&mut self) -> Vec<CortexHostEvent> {
        let mut events = Vec::new();
        let mut terminal = false;

        while let Ok(event) = self.event_rx.try_recv() {
            match &event {
                CortexHostEvent::Completed { .. } => terminal = true,
                CortexHostEvent::Failed { label, error } => {
                    terminal = true;
                    self.last_error = error.clone();
                    let _ = self.controller.record_desktop_error(label, error);
                }
                CortexHostEvent::Stream(_) => {}
            }
            events.push(event);
        }

        if terminal {
            self.busy = false;
            self.active_label.clear();
            self.cancel_flag.store(false, Ordering::Relaxed);
            if let Err(error) = self.refresh() {
                self.last_error = error.clone();
                events.push(CortexHostEvent::Failed {
                    label: "Cortex workspace refresh".to_owned(),
                    error,
                });
            }
        }

        events
    }

    #[must_use]
    pub fn capabilities(&self) -> Vec<CortexCapability> {
        vec![
            capability("cortex.chat", "Streaming project chat", "read_write"),
            capability("cortex.inspect", "Project inspection", "read_only"),
            capability("cortex.plan", "Project planning", "read_only"),
            capability("cortex.apply", "Project implementation", "source_mutation"),
            capability("cortex.repair", "Project repair", "source_mutation"),
            capability(
                "cortex.conversations",
                "Persistent project conversations",
                "local_mutation",
            ),
            capability("cortex.files", "Project/library file browsing", "read_only"),
            capability("cortex.context", "Project context index", "local_mutation"),
            capability("cortex.models", "Provider/model status", "read_only"),
            capability("cortex.vault", "Cortex library/Vault queries", "read_only"),
            capability("cortex.catalog", "Read-only storage catalog previews", "read_only"),
            capability("cortex.review", "Project review/change state", "read_only"),
            capability(
                "cortex.repository",
                "Repository safety/checkpoint actions",
                "source_mutation",
            ),
            capability(
                "cortex.settings",
                "Project-scoped Cortex settings",
                "local_mutation",
            ),
            capability(
                "cortex.cancel",
                "Active Cortex request cancellation",
                "local_mutation",
            ),
        ]
    }

    fn begin_background(&mut self, label: &str) {
        self.busy = true;
        self.active_label = label.to_owned();
        self.last_error.clear();
        self.cancel_flag = Arc::new(AtomicBool::new(false));
    }

    fn require_idle(&self) -> Result<(), String> {
        if self.busy {
            Err(format!(
                "Cortex is already busy{}",
                if self.active_label.is_empty() {
                    String::new()
                } else {
                    format!(" with {}", self.active_label)
                }
            ))
        } else {
            Ok(())
        }
    }

    fn refresh_view_only(&mut self) {
        self.view = self.controller.view();
        self.runtime.provider_recovery_pending = self.controller.provider_recovery_pending();
    }
}

fn send_terminal(tx: &Sender<CortexHostEvent>, label: String, result: Result<(), String>) {
    let event = match result {
        Ok(()) => CortexHostEvent::Completed { label },
        Err(error) => CortexHostEvent::Failed { label, error },
    };
    let _ = tx.send(event);
}

fn capability(id: &str, label: &str, risk: &str) -> CortexCapability {
    CortexCapability {
        id: id.to_owned(),
        label: label.to_owned(),
        risk: risk.to_owned(),
    }
}

fn probe_runtime(root: &Path) -> CortexRuntimeSnapshot {
    let bootstrap = DesktopBootstrap::load(root);
    let (service_registered, conversation_count) = match bootstrap {
        Ok(bootstrap) => (bootstrap.service.is_some(), bootstrap.conversations.len()),
        Err(_) => (false, 0),
    };

    let client = match RuntimeClient::discover(root) {
        Ok(client) => client,
        Err(error) => {
            return CortexRuntimeSnapshot {
                connection: CortexConnectionState::Disconnected,
                service_registered,
                conversation_count,
                error,
                ..Default::default()
            };
        }
    };

    let health = match client.require_desktop_api() {
        Ok(health) => health,
        Err(error) => {
            return CortexRuntimeSnapshot {
                connection: CortexConnectionState::Error,
                service_registered,
                conversation_count,
                health_json: client
                    .health()
                    .map_or_else(|_| String::new(), |value| pretty_value(&value)),
                error,
                ..Default::default()
            };
        }
    };

    let models = client.models();
    let provider = client.provider_status();
    let mut error_parts = Vec::new();
    if let Err(error) = &models {
        error_parts.push(format!("models: {error}"));
    }
    if let Err(error) = &provider {
        error_parts.push(format!("provider: {error}"));
    }

    CortexRuntimeSnapshot {
        connection: if error_parts.is_empty() {
            CortexConnectionState::Ready
        } else {
            CortexConnectionState::Error
        },
        desktop_api_ready: true,
        service_registered,
        conversation_count,
        health_json: pretty_value(&health),
        models_json: models.map_or_else(
            |error| format!("ERROR: {error}"),
            |value| pretty_value(&value),
        ),
        provider_json: provider.map_or_else(
            |error| format!("ERROR: {error}"),
            |value| pretty_value(&value),
        ),
        error: error_parts.join("; "),
        ..Default::default()
    }
}

fn assert_project_binding(expected: &Path, reported: &Path) -> Result<(), String> {
    if same_path(expected, reported) {
        Ok(())
    } else {
        Err(format!(
            "Cortex/Forge workspace authority mismatch: Forge selected `{}`, Cortex reports `{}`. Independent Cortex workspace switching is disabled inside Forge.",
            expected.display(),
            reported.display()
        ))
    }
}

fn canonical_or_original(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = canonical_or_original(left);
    let right = canonical_or_original(right);
    #[cfg(windows)]
    {
        let left = left.to_string_lossy().replace('\\', "/");
        let right = right.to_string_lossy().replace('\\', "/");
        left.trim_end_matches('/')
            .eq_ignore_ascii_case(right.trim_end_matches('/'))
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn pretty_value(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_modes_keep_mutating_modes_explicit() {
        assert_eq!(CortexRequestKind::Chat.agent_mode(), None);
        assert_eq!(CortexRequestKind::Inspect.agent_mode(), Some("inspect"));
        assert_eq!(CortexRequestKind::Plan.agent_mode(), Some("plan"));
        assert_eq!(CortexRequestKind::Apply.agent_mode(), Some("apply"));
        assert_eq!(CortexRequestKind::Repair.agent_mode(), Some("repair"));
    }

    #[test]
    fn only_catalog_scans_claim_cooperative_cancel() {
        assert!(CortexTaskKind::ScanStorageCatalog.supports_cooperative_cancel());
        assert!(CortexTaskKind::ScanMachineCatalog.supports_cooperative_cancel());
        assert!(!CortexTaskKind::BuildCheck.supports_cooperative_cancel());
    }

    #[test]
    fn parity_matrix_preserves_forge_authority_boundaries() {
        let items = cortex_parity_items();
        assert!(items.iter().any(|item| {
            item.id == "source.git" && item.state == CortexParityState::ForgeAuthority
        }));
        assert!(items.iter().any(|item| {
            item.id == "patch.intake" && item.state == CortexParityState::ForgeAuthority
        }));
        assert!(items.iter().any(|item| {
            item.id == "chat.streaming" && item.state == CortexParityState::Integrated
        }));
    }
}
