use cortex_bridge::{
    cortex_parity_items, CortexConnectionState, CortexHostEvent, CortexRequestKind,
    CortexRuntimeSnapshot, CortexTaskKind, CortexWorkspaceHost, DesktopFileListing, DesktopView,
    DesktopWorkbenchDocument,
};
use eframe::egui;
use forge_core::ProjectSession;
use forge_diagnostics::collect as collect_diagnostics;
use forge_ember::{inspect as inspect_ember, EmberHostState};
use forge_ide::{native_ide_capabilities, EditorSession, IdeSearchHit};
use forge_toolchain::doctor_project;
use forge_process::{
    operation_host_capabilities, recent_operation_receipts, recover_interrupted_operations, spawn,
    OperationEvent, OperationHandle, OperationReceipt, OperationState,
};
use forge_state::{
    ForgeStateStore, HostWorkspaceKind, HostWorkspaceRecord, HostWorkspaceState, TakeoverStatus,
};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

const VERSION: &str = "0.1.0-FR293-NATIVE-IDE-HARDENING";
const BG: egui::Color32 = egui::Color32::from_rgb(9, 11, 14);
const PANEL_2: egui::Color32 = egui::Color32::from_rgb(23, 28, 34);
const BORDER: egui::Color32 = egui::Color32::from_rgb(40, 49, 58);
const TEXT: egui::Color32 = egui::Color32::from_rgb(237, 242, 245);
const MUTED: egui::Color32 = egui::Color32::from_rgb(146, 154, 163);
const CYAN: egui::Color32 = egui::Color32::from_rgb(0, 217, 255);
const GREEN: egui::Color32 = egui::Color32::from_rgb(67, 240, 113);
const YELLOW: egui::Color32 = egui::Color32::from_rgb(255, 212, 74);
const RED: egui::Color32 = egui::Color32::from_rgb(255, 93, 104);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum AppTab {
    Projects,
    Workspace,
    Vault,
    SourceControl,
    Ide,
    Ember,
    Cortex,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum WorkspacePage {
    Dashboard,
    BuildRun,
    Operations,
    CommandRegistry,
    Updates,
    SourceControl,
    Diagnostics,
    Tooling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum CortexPage {
    Chat,
    Files,
    Context,
    Models,
    Activity,
    Reviews,
    Vault,
    Tasks,
    Settings,
    Parity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedUiState {
    tab: AppTab,
    workspace_page: WorkspacePage,
    cortex_page: CortexPage,
    cortex_mode: String,
}

impl Default for PersistedUiState {
    fn default() -> Self {
        Self {
            tab: AppTab::Workspace,
            workspace_page: WorkspacePage::Operations,
            cortex_page: CortexPage::Chat,
            cortex_mode: "chat".to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
struct ConsoleLine {
    text: String,
    color: egui::Color32,
}

struct ForgeApp {
    root: PathBuf,
    session: Result<ProjectSession, String>,
    tab: AppTab,
    workspace_page: WorkspacePage,
    cortex_page: CortexPage,
    console: Vec<ConsoleLine>,
    operation_tx: Sender<OperationEvent>,
    operation_rx: Receiver<OperationEvent>,
    active_operation: Option<OperationHandle>,
    operation_history: Vec<OperationReceipt>,
    busy: bool,
    active_pid: Option<u32>,
    health_score: f32,
    forge_state: Option<ForgeStateStore>,
    forge_state_error: String,
    forge_scan_root_input: String,
    ide_session: Option<EditorSession>,
    ide_error: String,
    ide_path_input: String,
    ide_file_filter: String,
    ide_file_cache: Vec<String>,
    ide_find: String,
    ide_find_hits: Vec<IdeSearchHit>,
    ide_status: String,
    cortex: Option<CortexWorkspaceHost>,
    cortex_error: String,
    cortex_prompt: String,
    cortex_live_text: String,
    cortex_phase: String,
    cortex_mode: CortexRequestKind,
    cortex_setting_key: String,
    cortex_setting_value: String,
    cortex_file_scope_library: bool,
    cortex_directory: String,
    cortex_listing: Option<DesktopFileListing>,
    cortex_document: Option<DesktopWorkbenchDocument>,
    cortex_file_query: String,
    cortex_vault_query: String,
    cortex_ingest_paths: String,
    cortex_redo_instructions: String,
    cortex_redo_target: String,
    cortex_last_export: String,
}

impl ForgeApp {
    fn new(root: PathBuf) -> Self {
        let persisted = load_ui_state(&root);
        let (operation_tx, operation_rx) = mpsc::channel();
        let recovered = recover_interrupted_operations(&root);
        let operation_history = recent_operation_receipts(&root, 32).unwrap_or_default();
        let session = ProjectSession::load(&root).map_err(|error| error.to_string());

        let (mut forge_state, mut forge_state_error) = match ForgeStateStore::open_default(&root) {
            Ok(mut store) => {
                let mut error = String::new();
                if let Err(value) = store.register_project_root(&root) {
                    error = value;
                }
                if let Err(value) = store.recover_running_queue() {
                    if !error.is_empty() {
                        error.push_str("; ");
                    }
                    error.push_str(&value);
                }
                (Some(store), error)
            }
            Err(error) => (None, error),
        };

        let (ide_session, ide_error, ide_file_cache) = match EditorSession::open(&root) {
            Ok(session) => {
                let files = session.workspace().list_text_files(4_000).unwrap_or_default();
                (Some(session), String::new(), files)
            }
            Err(error) => (None, error, Vec::new()),
        };

        let ember_status = inspect_ember(&root).ok();

        let (cortex, cortex_error) = match CortexWorkspaceHost::open(&root) {
            Ok(host) => (Some(host), String::new()),
            Err(error) => (None, error),
        };

        if let Some(store) = forge_state.as_mut() {
            let cortex_state = if cortex.is_some() {
                HostWorkspaceState::Candidate
            } else {
                HostWorkspaceState::Degraded
            };
            if let Err(error) = store.upsert_host_workspace(HostWorkspaceRecord {
                kind: HostWorkspaceKind::Cortex,
                state: cortex_state,
                detail: if cortex.is_some() {
                    "Cortex Desktop core is bound into the Forge-hosted workspace; build certification remains pending".to_owned()
                } else {
                    cortex_error.clone()
                },
                updated_unix_ms: 0,
            }) {
                if !forge_state_error.is_empty() {
                    forge_state_error.push_str("; ");
                }
                forge_state_error.push_str(&error);
            }
            let ember_ready = ember_status
                .as_ref()
                .is_some_and(|status| status.state == EmberHostState::Ready);
            let _ = store.upsert_host_workspace(HostWorkspaceRecord {
                kind: HostWorkspaceKind::Ember,
                state: if ember_ready {
                    HostWorkspaceState::Candidate
                } else {
                    HostWorkspaceState::Unavailable
                },
                detail: ember_status
                    .as_ref()
                    .map(|status| status.detail.clone())
                    .unwrap_or_else(|| "Ember host adapter is not configured for this project yet".to_owned()),
                updated_unix_ms: 0,
            });
            let _ = store.upsert_host_workspace(HostWorkspaceRecord {
                kind: HostWorkspaceKind::Ide,
                state: if ide_session.is_some() {
                    HostWorkspaceState::Candidate
                } else {
                    HostWorkspaceState::Degraded
                },
                detail: if ide_session.is_some() {
                    "Native Rust Forge IDE is active: no WebView/Monaco dependency; candidate gate pending".to_owned()
                } else {
                    ide_error.clone()
                },
                updated_unix_ms: 0,
            });
            let _ = store.set_takeover_check(
                "cortex_host",
                TakeoverStatus::Candidate,
                "Forge-hosted Cortex bridge is implemented in source and awaits candidate gate verification",
            );
        }

        let mut app = Self {
            root,
            session,
            tab: persisted.tab,
            workspace_page: persisted.workspace_page,
            cortex_page: persisted.cortex_page,
            console: Vec::new(),
            operation_tx,
            operation_rx,
            active_operation: None,
            operation_history,
            busy: false,
            active_pid: None,
            health_score: 0.0,
            forge_state,
            forge_state_error,
            forge_scan_root_input: String::new(),
            ide_session,
            ide_error,
            ide_path_input: String::new(),
            ide_file_filter: String::new(),
            ide_file_cache,
            ide_find: String::new(),
            ide_find_hits: Vec::new(),
            ide_status: String::new(),
            cortex,
            cortex_error,
            cortex_prompt: String::new(),
            cortex_live_text: String::new(),
            cortex_phase: String::new(),
            cortex_mode: parse_cortex_mode(&persisted.cortex_mode),
            cortex_setting_key: "provider".to_owned(),
            cortex_setting_value: String::new(),
            cortex_file_scope_library: false,
            cortex_directory: String::new(),
            cortex_listing: None,
            cortex_document: None,
            cortex_file_query: String::new(),
            cortex_vault_query: String::new(),
            cortex_ingest_paths: String::new(),
            cortex_redo_instructions: String::new(),
            cortex_redo_target: String::new(),
            cortex_last_export: String::new(),
        };

        app.append("[INFO] Forge Rust unified host started.");
        match recovered {
            Ok(0) => {}
            Ok(count) => app.append(&format!(
                "[WARN] Recovered {count} unterminated operation receipt(s) as interrupted."
            )),
            Err(error) => app.append(&format!("[WARN] Operation recovery scan failed: {error}")),
        }

        if let Ok(session) = &app.session {
            app.append(&format!(
                "[PASS] Active project: {}",
                session.contract.display_name()
            ));
            if session.provider_ready() {
                app.append("[PASS] Project-native provider ready.");
            } else {
                app.append(
                    "[WARN] No project-native provider resolved; direct contract commands only.",
                );
            }
        } else if let Err(error) = &app.session {
            app.append(&format!("[FAIL] {error}"));
        }

        if app.forge_state.is_some() {
            app.append("[PASS] Forge persistent machine-state spine ready.");
        } else if !app.forge_state_error.is_empty() {
            app.append(&format!("[WARN] Forge state unavailable: {}", app.forge_state_error));
        }

        if app.ide_session.is_some() {
            app.append("[PASS] Native Forge IDE ready; WebView/Monaco is not required.");
        } else if !app.ide_error.is_empty() {
            app.append(&format!("[WARN] Native Forge IDE unavailable: {}", app.ide_error));
        }

        if app.cortex.is_some() {
            app.append("[PASS] Cortex Desktop core normalized into Forge Cortex workspace.");
        } else if !app.cortex_error.is_empty() {
            app.append(&format!(
                "[WARN] Cortex workspace unavailable: {}",
                app.cortex_error
            ));
        }

        app.recalculate_health();
        app
    }

    fn append(&mut self, text: &str) {
        self.console.push(ConsoleLine {
            text: text.to_owned(),
            color: semantic_color(text),
        });
        if self.console.len() > 4_000 {
            self.console.drain(..1_000);
        }
    }

    fn recalculate_health(&mut self) {
        let project_score = match &self.session {
            Ok(session) if session.provider_ready() => 1.0,
            Ok(_) => 0.72,
            Err(_) => 0.2,
        };
        let cortex_score = self.cortex.as_ref().map_or(0.5, |host| {
            if host.runtime().connection == CortexConnectionState::Ready {
                1.0
            } else {
                0.75
            }
        });
        self.health_score = project_score * 0.7 + cortex_score * 0.3;
    }

    fn refresh_operation_history(&mut self) {
        match recent_operation_receipts(&self.root, 32) {
            Ok(history) => self.operation_history = history,
            Err(error) => self.append(&format!("[WARN] Operation history refresh failed: {error}")),
        }
    }

    fn finish_operation(&mut self, operation_id: &str) {
        let active_matches = self
            .active_operation
            .as_ref()
            .is_some_and(|handle| handle.id() == operation_id);
        if active_matches {
            self.active_operation = None;
            self.busy = false;
            self.active_pid = None;
        }
        self.refresh_operation_history();
    }

    fn cancel_active_operation(&mut self) {
        let Some(handle) = self.active_operation.as_ref() else {
            self.append("[WARN] No active Rust Forge operation to stop.");
            return;
        };
        handle.cancel();
        let operation_id = handle.id().to_owned();
        self.append(&format!(
            "[WARN] Stop requested for durable operation {operation_id}."
        ));
    }

    fn drain_operation_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.operation_rx.try_recv() {
            match event {
                OperationEvent::Queued {
                    operation_id,
                    label,
                    log_path,
                    receipt_path,
                } => {
                    self.busy = true;
                    self.append(&format!(
                        "[INFO] QUEUED {label} [{operation_id}] receipt={receipt_path} log={log_path}"
                    ));
                }
                OperationEvent::Started {
                    operation_id,
                    label,
                    pid,
                } => {
                    self.busy = true;
                    self.active_pid = Some(pid);
                    self.append(&format!(
                        "[INFO] START {label} [{operation_id}] (pid {pid})"
                    ));
                }
                OperationEvent::Output {
                    operation_id: _,
                    stderr,
                    line,
                } => {
                    if stderr && !line.contains("[PASS]") && !line.contains("[WARN]") {
                        self.console.push(ConsoleLine {
                            text: line,
                            color: MUTED,
                        });
                    } else {
                        self.append(&line);
                    }
                }
                OperationEvent::CancelRequested {
                    operation_id,
                    label,
                } => self.append(&format!(
                    "[WARN] CANCEL REQUESTED {label} [{operation_id}]"
                )),
                OperationEvent::Cancelled {
                    operation_id,
                    label,
                    elapsed_ms,
                    log_path,
                    receipt_path,
                } => {
                    self.append(&format!(
                        "[WARN] CANCELLED {label} [{operation_id}] ({elapsed_ms} ms) receipt={receipt_path} log={log_path}"
                    ));
                    self.finish_operation(&operation_id);
                }
                OperationEvent::Finished {
                    operation_id,
                    label,
                    success,
                    code,
                    elapsed_ms,
                    log_path,
                    receipt_path,
                } => {
                    let token = if success { "PASS" } else { "FAIL" };
                    self.append(&format!(
                        "[{token}] END {label} [{operation_id}] ({elapsed_ms} ms, exit {}) receipt={receipt_path} log={log_path}",
                        code.map_or_else(|| "?".to_owned(), |value| value.to_string())
                    ));
                    self.finish_operation(&operation_id);
                }
                OperationEvent::FailedToStart {
                    operation_id,
                    label,
                    error,
                    log_path,
                    receipt_path,
                } => {
                    self.append(&format!(
                        "[FAIL] START {label} [{operation_id}]: {error} receipt={receipt_path} log={log_path}"
                    ));
                    self.finish_operation(&operation_id);
                }
                OperationEvent::HostError {
                    operation_id,
                    label,
                    error,
                    log_path,
                    receipt_path,
                } => {
                    self.append(&format!(
                        "[FAIL] OPERATION HOST {label} [{operation_id}]: {error} receipt={receipt_path} log={log_path}"
                    ));
                    self.finish_operation(&operation_id);
                }
            }
            ctx.request_repaint();
        }
    }

    fn drain_cortex_events(&mut self, ctx: &egui::Context) {
        let events = self
            .cortex
            .as_mut()
            .map_or_else(Vec::new, CortexWorkspaceHost::poll_events);

        for event in events {
            match event {
                CortexHostEvent::Stream(frame) => {
                    self.cortex_phase = if frame.message.is_empty() {
                        frame.phase.clone()
                    } else {
                        format!("{} — {}", frame.phase, frame.message)
                    };
                    if !frame.text_delta.is_empty() {
                        self.cortex_live_text.push_str(&frame.text_delta);
                    }
                    if !frame.data.is_empty() && frame.data != "null" {
                        self.append(&format!(
                            "[INFO] Cortex {} {} ms",
                            frame.kind, frame.elapsed_ms
                        ));
                    }
                }
                CortexHostEvent::Completed { label } => {
                    self.cortex_live_text.clear();
                    self.cortex_phase = format!("{label} complete");
                    self.append(&format!("[PASS] Cortex {label} completed."));
                    self.recalculate_health();
                }
                CortexHostEvent::Failed { label, error } => {
                    self.cortex_phase = format!("{label} failed");
                    self.append(&format!("[FAIL] Cortex {label}: {error}"));
                    self.recalculate_health();
                }
            }
            ctx.request_repaint();
        }
    }

    fn start_operation(&mut self, operation: &str) {
        if self.busy {
            self.append("[WARN] A Forge operation is already running.");
            return;
        }
        if self
            .cortex
            .as_ref()
            .is_some_and(CortexWorkspaceHost::busy)
        {
            self.append("[WARN] Cortex is active; Forge project operations are held until Cortex reaches a terminal state.");
            return;
        }
        let spec = match &self.session {
            Ok(session) => session.resolve(operation),
            Err(error) => {
                self.append(&format!("[FAIL] Cannot run operation: {error}"));
                return;
            }
        };
        match spec {
            Ok(spec) => match spawn(spec, self.operation_tx.clone()) {
                Ok(handle) => {
                    self.busy = true;
                    self.active_operation = Some(handle);
                }
                Err(error) => self.append(&format!(
                    "[FAIL] Durable operation could not be reserved: {error}"
                )),
            },
            Err(error) => self.append(&format!("[FAIL] {error}")),
        }
    }

    fn start_cortex_request(&mut self) {
        if self.busy {
            self.append("[WARN] A Forge project operation is active; Cortex execution is held until it finishes.");
            return;
        }
        let prompt = self.cortex_prompt.trim().to_owned();
        if prompt.is_empty() {
            self.append("[WARN] Cortex prompt is empty.");
            return;
        }
        let result = self
            .cortex
            .as_mut()
            .ok_or_else(|| "Cortex host is unavailable".to_owned())
            .and_then(|host| host.start_request(self.cortex_mode, &prompt));

        match result {
            Ok(()) => {
                self.cortex_live_text.clear();
                self.cortex_phase = format!("{} starting", self.cortex_mode.label());
                self.cortex_prompt.clear();
                self.append(&format!(
                    "[INFO] Cortex {} request started.",
                    self.cortex_mode.label()
                ));
            }
            Err(error) => self.append(&format!("[FAIL] Cortex request: {error}")),
        }
    }

    fn cancel_cortex_request(&mut self) {
        let result = self
            .cortex
            .as_mut()
            .ok_or_else(|| "Cortex host is unavailable".to_owned())
            .and_then(CortexWorkspaceHost::cancel_active);
        match result {
            Ok(true) => self.append("[WARN] Cortex cancellation requested."),
            Ok(false) => self.append("[INFO] No active Cortex request to cancel."),
            Err(error) => self.append(&format!("[FAIL] Cortex cancellation: {error}")),
        }
    }

    fn header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(CYAN, egui::RichText::new("FORGE").strong().size(20.0));
                ui.label(egui::RichText::new(format!("Rust {VERSION}")).color(MUTED));
                ui.separator();
                let active = self
                    .session
                    .as_ref()
                    .map_or("No active project", |session| session.contract.display_name());
                ui.label(egui::RichText::new(active).color(TEXT));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Refresh").clicked() {
                        self.recalculate_health();
                        self.refresh_operation_history();
                        if let Some(host) = self.cortex.as_mut() {
                            if let Err(error) = host.refresh() {
                                self.cortex_error = error;
                            }
                        }
                        self.append("[PASS] Forge project state refreshed.");
                    }
                    if ui.button("Project Status").clicked() {
                        self.start_operation("project.status");
                    }
                });
            });
        });
    }

    fn app_rail(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("app_rail")
            .exact_width(122.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("FORGE WORKSPACES")
                        .color(MUTED)
                        .size(10.0),
                );
                ui.add_space(8.0);
                for (tab, label) in [
                    (AppTab::Projects, "Projects"),
                    (AppTab::Workspace, "Workspace"),
                    (AppTab::Vault, "Vault"),
                    (AppTab::SourceControl, "Source Control"),
                    (AppTab::Ide, "IDE"),
                    (AppTab::Ember, "Ember"),
                    (AppTab::Cortex, "Cortex"),
                    (AppTab::Settings, "Settings"),
                ] {
                    if ui.selectable_label(self.tab == tab, label).clicked() {
                        self.tab = tab;
                    }
                    ui.add_space(4.0);
                }
            });
    }

    fn health_rail(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("health_rail")
            .exact_width(190.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("FORGE HEALTH")
                        .color(MUTED)
                        .size(10.0),
                );
                ui.add_space(8.0);
                draw_health_gauge(ui, self.health_score);
                ui.separator();
                status_row(ui, "Contract", self.session.is_ok());
                status_row(
                    ui,
                    "Provider",
                    self.session
                        .as_ref()
                        .is_ok_and(ProjectSession::provider_ready),
                );
                status_row(ui, "Operation Host", true);
                status_row(ui, "Forge State", self.forge_state.is_some());
                status_row(ui, "Cortex Host", self.cortex.is_some());
                status_row(
                    ui,
                    "Cortex Runtime",
                    self.cortex.as_ref().is_some_and(|host| {
                        host.runtime().connection == CortexConnectionState::Ready
                    }),
                );
                ui.separator();

                if self.busy {
                    ui.colored_label(YELLOW, "Forge operation running");
                    if let Some(handle) = self.active_operation.as_ref() {
                        ui.label(
                            egui::RichText::new(handle.id())
                                .color(MUTED)
                                .size(9.0),
                        );
                    }
                    if let Some(pid) = self.active_pid {
                        ui.label(egui::RichText::new(format!("PID {pid}")).color(MUTED));
                    }
                    if ui.button("STOP OPERATION").clicked() {
                        self.cancel_active_operation();
                    }
                } else {
                    ui.colored_label(GREEN, "Forge operations ready");
                }

                if self
                    .cortex
                    .as_ref()
                    .is_some_and(CortexWorkspaceHost::busy)
                {
                    ui.add_space(6.0);
                    ui.colored_label(CYAN, "Cortex working");
                    if ui.button("STOP CORTEX").clicked() {
                        self.cancel_cortex_request();
                    }
                }

                ui.add_space(10.0);
                ui.label(egui::RichText::new("ARCHITECTURE").color(MUTED).size(10.0));
                ui.label(egui::RichText::new("Forge = workstation").color(TEXT));
                ui.label(egui::RichText::new("Cortex = intelligence").color(TEXT));
                ui.label(egui::RichText::new("Ember = game authoring").color(TEXT));
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("ForgePY remains production authority until takeover GREEN")
                        .color(MUTED)
                        .size(9.0),
                );
            });
    }

    fn quick_actions(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("QUICK ACTIONS").color(CYAN).strong());
            ui.separator();
            if action_button(ui, "FULL GATE / CERTIFY GREEN", self.busy) {
                self.start_operation("gate.full");
            }
            if action_button(ui, "BUILD", self.busy) {
                self.start_operation("build.native");
            }
            if action_button(ui, "RUN", self.busy) {
                self.start_operation("run.gui");
            }
            if action_button(ui, "DEBUG BUNDLE", self.busy) {
                self.start_operation("diagnostics.bundle");
            }
            if action_button(ui, "COMMIT + PUSH GREEN", self.busy) {
                self.start_operation("git.commit-push-green");
            }
            if self.busy && ui.button("STOP").clicked() {
                self.cancel_active_operation();
            }
        });
        ui.separator();
    }

    fn workspace(&mut self, ui: &mut egui::Ui) {
        self.quick_actions(ui);
        ui.columns(2, |columns| {
            columns[0].set_min_width(380.0);
            columns[0].vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_min_width(140.0);
                        ui.label(
                            egui::RichText::new("PROJECT OPERATIONS")
                                .color(MUTED)
                                .size(10.0),
                        );
                        for (page, label) in [
                            (WorkspacePage::Dashboard, "Dashboard"),
                            (WorkspacePage::BuildRun, "Build & Run"),
                            (WorkspacePage::Operations, "Operations"),
                            (WorkspacePage::CommandRegistry, "Command Registry"),
                            (WorkspacePage::Updates, "Updates"),
                            (WorkspacePage::SourceControl, "Source Control"),
                            (WorkspacePage::Diagnostics, "Diagnostics"),
                            (WorkspacePage::Tooling, "Tooling"),
                        ] {
                            if ui
                                .selectable_label(self.workspace_page == page, label)
                                .clicked()
                            {
                                self.workspace_page = page;
                            }
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_min_width(255.0);
                        self.workspace_detail(ui);
                    });
                });
            });
            columns[1].vertical(|ui| self.console_panel(ui));
        });
    }

    fn workspace_detail(&mut self, ui: &mut egui::Ui) {
        match self.workspace_page {
            WorkspacePage::Dashboard => {
                ui.heading("Dashboard");
                if let Ok(session) = &self.session {
                    ui.label(format!("Project: {}", session.contract.display_name()));
                    ui.label(format!("Kind: {}", session.contract.project.kind));
                    ui.label(format!("Root: {}", session.root.display()));
                    let capabilities = session.capabilities();
                    ui.label(format!(
                        "Contract: {} v{}",
                        capabilities.contract.schema, capabilities.contract.schema_version
                    ));
                    ui.label(format!("Operations: {}", capabilities.operations.len()));
                    ui.label(format!("Artifacts: {}", capabilities.artifacts.len()));
                }
                ui.separator();
                ui.label(format!(
                    "Durable operation receipts: {}",
                    self.operation_history.len()
                ));
                ui.label(format!(
                    "Cortex host: {}",
                    if self.cortex.is_some() { "ready" } else { "unavailable" }
                ));
            }
            WorkspacePage::BuildRun => {
                ui.heading("Build / Run");
                if action_button(ui, "Full Gate", self.busy) {
                    self.start_operation("gate.full");
                }
                if action_button(ui, "Build", self.busy) {
                    self.start_operation("build.native");
                }
                if action_button(ui, "Run", self.busy) {
                    self.start_operation("run.gui");
                }
                if action_button(ui, "Project Self-Test", self.busy) {
                    self.start_operation("project.self-test");
                }
            }
            WorkspacePage::Operations => self.operations_workspace(ui),
            WorkspacePage::CommandRegistry => self.command_registry(ui),
            WorkspacePage::Updates => {
                ui.heading("Updates");
                ui.label(
                    egui::RichText::new(
                        "Project patch execution remains routed through the project-owned provider while the native Rust transaction engine is certified.",
                    )
                    .color(MUTED),
                );
                if action_button(ui, "Patch Status", self.busy) {
                    self.start_operation("patch.status");
                }
                if action_button(ui, "Apply Validated Project Queue", self.busy) {
                    self.start_operation("patch.apply");
                }
            }
            WorkspacePage::SourceControl => self.source_control_workspace(ui),
            WorkspacePage::Diagnostics => {
                ui.heading("Diagnostics");
                if action_button(ui, "Create Debug Bundle", self.busy) {
                    self.start_operation("diagnostics.bundle");
                }
                if action_button(ui, "Doctor Status", self.busy) {
                    self.start_operation("doctor.status");
                }
                ui.separator();
                ui.label(format!(
                    "Durable operation history: {} receipt(s)",
                    self.operation_history.len()
                ));
            }
            WorkspacePage::Tooling => {
                ui.heading("Tooling");
                let capabilities = operation_host_capabilities();
                ui.label(format!("Operation host: {}", capabilities.schema));
                ui.label(format!(
                    "Durable IDs / receipts / logs: {} / {} / {}",
                    capabilities.durable_operation_ids,
                    capabilities.persistent_receipts,
                    capabilities.persistent_logs
                ));
                ui.label(format!(
                    "Cancellation / recovery / history: {} / {} / {}",
                    capabilities.cancellation,
                    capabilities.interrupted_recovery,
                    capabilities.durable_history
                ));
                ui.separator();
                ui.label(
                    egui::RichText::new(
                        "Blender/tool inventory, dependency doctor, fleet scheduling and editor toolchains remain dedicated Forge subsystems.",
                    )
                    .color(MUTED),
                );
            }
        }
    }

    fn operations_workspace(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Operations");
            if ui.button("Refresh").clicked() {
                self.refresh_operation_history();
            }
        });
        ui.label(
            egui::RichText::new(
                "Durable ID + receipt + persistent log + explicit terminal state for every Rust Forge operation.",
            )
            .color(MUTED),
        );
        ui.separator();
        egui::ScrollArea::vertical()
            .max_height(430.0)
            .show(ui, |ui| {
                if self.operation_history.is_empty() {
                    ui.label(egui::RichText::new("No durable operations yet.").color(MUTED));
                }
                for receipt in &self.operation_history {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(
                                state_color(receipt.state),
                                receipt.state.as_str().to_ascii_uppercase(),
                            );
                            ui.label(egui::RichText::new(&receipt.label).strong());
                        });
                        ui.label(
                            egui::RichText::new(&receipt.operation_id)
                                .monospace()
                                .size(9.0)
                                .color(MUTED),
                        );
                        ui.label(format!(
                            "PID {}  Exit {}  Elapsed {}",
                            receipt
                                .pid
                                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                            receipt
                                .exit_code
                                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                            receipt
                                .elapsed_ms
                                .map_or_else(|| "-".to_owned(), |value| format!("{value} ms"))
                        ));
                        ui.label(
                            egui::RichText::new(format!("Receipt: {}", receipt.receipt_path))
                                .size(9.0)
                                .color(MUTED),
                        );
                        ui.label(
                            egui::RichText::new(format!("Log: {}", receipt.log_path))
                                .size(9.0)
                                .color(MUTED),
                        );
                        if let Some(error) = &receipt.error {
                            ui.colored_label(YELLOW, error);
                        }
                    });
                    ui.add_space(4.0);
                }
            });
    }

    fn command_registry(&mut self, ui: &mut egui::Ui) {
        ui.heading("Command Registry");
        ui.label(
            egui::RichText::new(
                "Forge renders operations from the project contract/capability authority instead of duplicating project command logic.",
            )
            .color(MUTED),
        );
        ui.separator();

        let operations = self
            .session
            .as_ref()
            .map(|session| session.capabilities().operations)
            .unwrap_or_default();
        let mut selected = None;

        egui::ScrollArea::vertical()
            .max_height(430.0)
            .show(ui, |ui| {
                for operation in operations {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&operation.label).strong());
                            ui.label(
                                egui::RichText::new(&operation.category)
                                    .color(MUTED)
                                    .size(9.0),
                            );
                        });
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · risk={} · source={}",
                                operation.key, operation.risk, operation.source
                            ))
                            .monospace()
                            .size(9.0)
                            .color(MUTED),
                        );
                        if action_button(ui, "Run", self.busy) {
                            selected = Some(operation.key);
                        }
                    });
                }
            });

        if let Some(operation) = selected {
            self.start_operation(&operation);
        }
    }

    fn source_control_workspace(&mut self, ui: &mut egui::Ui) {
        ui.heading("Source Control");
        if action_button(ui, "Git Status", self.busy) {
            self.start_operation("git.status");
        }
        if action_button(ui, "Commit GREEN", self.busy) {
            self.start_operation("git.commit-green");
        }
        if action_button(ui, "Commit + Push GREEN", self.busy) {
            self.start_operation("git.commit-push-green");
        }
        if action_button(ui, "Push", self.busy) {
            self.start_operation("git.push");
        }
        ui.separator();
        ui.label(
            egui::RichText::new(
                "GitHub + Forge Internal Git remain universal Forge authorities. Cortex consumes their project state; it does not own a competing source-control implementation.",
            )
            .color(MUTED),
        );
    }

    fn console_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.colored_label(CYAN, egui::RichText::new("PROJECT CONSOLE").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Clear").clicked() {
                    self.console.clear();
                }
                if self.busy && ui.button("STOP").clicked() {
                    self.cancel_active_operation();
                }
            });
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_height(620.0);
                for line in &self.console {
                    ui.label(
                        egui::RichText::new(&line.text)
                            .monospace()
                            .color(line.color),
                    );
                }
            });
    }

    fn cortex_workspace(&mut self, ui: &mut egui::Ui) {
        if self.cortex.is_none() {
            ui.heading("Cortex");
            ui.colored_label(RED, "Cortex host unavailable");
            ui.label(&self.cortex_error);
            if ui.button("Retry Cortex Host").clicked() {
                match CortexWorkspaceHost::open(&self.root) {
                    Ok(host) => {
                        self.cortex = Some(host);
                        self.cortex_error.clear();
                        self.append("[PASS] Cortex host connected.");
                    }
                    Err(error) => self.cortex_error = error,
                }
            }
            return;
        }

        let snapshot = self
            .cortex
            .as_ref()
            .map(|host| host.view().clone())
            .unwrap_or_default();
        let runtime = self
            .cortex
            .as_ref()
            .map(|host| host.runtime().clone())
            .unwrap_or_default();
        let cortex_busy = self.busy
            || self
                .cortex
                .as_ref()
                .is_some_and(CortexWorkspaceHost::busy);

        let mut new_conversation = false;
        let mut archive_conversation = false;
        let mut select_conversation = None;
        let mut refresh = false;
        let mut export_current = false;
        let mut export_project = false;

        ui.horizontal(|ui| {
            ui.heading("Cortex");
            ui.label(
                egui::RichText::new("project-bound intelligence workspace")
                    .color(MUTED)
                    .size(10.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Refresh").clicked() {
                    refresh = true;
                }
                connection_badge(ui, runtime.connection);
            });
        });
        ui.separator();

        ui.horizontal_wrapped(|ui| {
            for (page, label) in [
                (CortexPage::Chat, "Chat"),
                (CortexPage::Files, "Files"),
                (CortexPage::Context, "Context"),
                (CortexPage::Models, "Models"),
                (CortexPage::Activity, "Activity"),
                (CortexPage::Reviews, "Reviews"),
                (CortexPage::Vault, "Vault"),
                (CortexPage::Tasks, "Tasks"),
                (CortexPage::Settings, "Settings"),
                (CortexPage::Parity, "Parity"),
            ] {
                if ui.selectable_label(self.cortex_page == page, label).clicked() {
                    self.cortex_page = page;
                }
            }
        });
        ui.separator();

        ui.columns(2, |columns| {
            columns[0].set_min_width(220.0);
            columns[0].vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("CONVERSATIONS").color(MUTED).size(10.0));
                    if ui
                        .add_enabled(!cortex_busy, egui::Button::new("+ New"))
                        .clicked()
                    {
                        new_conversation = true;
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(430.0)
                    .show(ui, |ui| {
                        for (index, title) in snapshot.conversations.iter().enumerate() {
                            if ui
                                .selectable_label(snapshot.active_conversation == Some(index), title)
                                .clicked()
                            {
                                select_conversation = Some(index);
                            }
                        }
                    });
                ui.separator();
                if ui
                    .add_enabled(!cortex_busy, egui::Button::new("Archive Current"))
                    .clicked()
                {
                    archive_conversation = true;
                }
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            !cortex_busy && snapshot.active_conversation.is_some(),
                            egui::Button::new("Export MD"),
                        )
                        .clicked()
                    {
                        export_current = true;
                    }
                    if ui
                        .add_enabled(!cortex_busy, egui::Button::new("Export Project"))
                        .clicked()
                    {
                        export_project = true;
                    }
                });
                if !self.cortex_last_export.is_empty() {
                    ui.label(
                        egui::RichText::new(&self.cortex_last_export)
                            .color(MUTED)
                            .size(9.0),
                    );
                }
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(format!("Forge project: {}", snapshot.workspace_title))
                        .color(MUTED)
                        .size(9.0),
                );
                ui.label(
                    egui::RichText::new(
                        "Workspace switching is Forge-owned; Cortex cannot silently rebind itself.",
                    )
                    .color(MUTED)
                    .size(9.0),
                );
            });

            columns[1].vertical(|ui| match self.cortex_page {
                CortexPage::Chat => self.cortex_chat_page(ui, &snapshot, cortex_busy),
                CortexPage::Files => self.cortex_files_page(ui, &snapshot, cortex_busy),
                CortexPage::Context => self.cortex_context_page(ui, &snapshot, cortex_busy),
                CortexPage::Models => self.cortex_models_page(ui, &runtime, cortex_busy),
                CortexPage::Activity => self.cortex_activity_page(ui, &snapshot),
                CortexPage::Reviews => self.cortex_reviews_page(ui, &snapshot, cortex_busy),
                CortexPage::Vault => self.cortex_vault_page(ui, &snapshot, cortex_busy),
                CortexPage::Tasks => self.cortex_tasks_page(ui, &snapshot, cortex_busy),
                CortexPage::Settings => {
                    self.cortex_settings_page(ui, &runtime, cortex_busy);
                }
                CortexPage::Parity => self.cortex_parity_page(ui),
            });
        });

        if refresh {
            if let Some(host) = self.cortex.as_mut() {
                if let Err(error) = host.refresh() {
                    self.append(&format!("[FAIL] Cortex refresh: {error}"));
                }
            }
        }
        if new_conversation {
            if let Some(host) = self.cortex.as_mut() {
                if let Err(error) = host.new_conversation() {
                    self.append(&format!("[FAIL] New Cortex conversation: {error}"));
                }
            }
        }
        if archive_conversation {
            if let Some(host) = self.cortex.as_mut() {
                if let Err(error) = host.archive_current_conversation() {
                    self.append(&format!("[FAIL] Archive Cortex conversation: {error}"));
                }
            }
        }
        if let Some(index) = select_conversation {
            if let Some(host) = self.cortex.as_mut() {
                if let Err(error) = host.select_conversation(index) {
                    self.append(&format!("[FAIL] Select Cortex conversation: {error}"));
                }
            }
        }
        if export_current {
            if let Some(index) = snapshot.active_conversation {
                let result = self
                    .cortex
                    .as_mut()
                    .ok_or_else(|| "Cortex host unavailable".to_owned())
                    .and_then(|host| host.export_conversation(index, "md"));
                match result {
                    Ok(path) => {
                        self.cortex_last_export = path.clone();
                        self.append(&format!("[PASS] Cortex conversation exported: {path}"));
                    }
                    Err(error) => self.append(&format!("[FAIL] Cortex export: {error}")),
                }
            }
        }
        if export_project {
            let result = self
                .cortex
                .as_mut()
                .ok_or_else(|| "Cortex host unavailable".to_owned())
                .and_then(|host| host.export_project_conversations("md"));
            match result {
                Ok(path) => {
                    self.cortex_last_export = path.clone();
                    self.append(&format!("[PASS] Cortex project conversations exported: {path}"));
                }
                Err(error) => self.append(&format!("[FAIL] Cortex project export: {error}")),
            }
        }
    }

    fn cortex_chat_page(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &DesktopView,
        cortex_busy: bool,
    ) {
        ui.heading(if snapshot.conversation_title.is_empty() {
            "Chat"
        } else {
            &snapshot.conversation_title
        });

        let cortex_active = self
            .cortex
            .as_ref()
            .is_some_and(CortexWorkspaceHost::busy);
        let mut feedback_action: Option<(String, i8)> = None;
        let mut redo_action: Option<String> = None;

        egui::ScrollArea::vertical()
            .stick_to_bottom(cortex_active)
            .max_height(455.0)
            .show(ui, |ui| {
                for block in &snapshot.chat_blocks {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(if block.label.is_empty() {
                                    &block.role
                                } else {
                                    &block.label
                                })
                                .strong(),
                            );
                            if !block.status.is_empty() {
                                ui.label(
                                    egui::RichText::new(&block.status)
                                        .color(MUTED)
                                        .size(9.0),
                                );
                            }
                            if block.role.eq_ignore_ascii_case("assistant")
                                && !block.message_id.is_empty()
                            {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .add_enabled(
                                                !cortex_busy,
                                                egui::Button::new("Redo"),
                                            )
                                            .clicked()
                                        {
                                            redo_action = Some(block.message_id.clone());
                                        }
                                        if ui
                                            .add_enabled(
                                                !cortex_busy,
                                                egui::Button::new("−"),
                                            )
                                            .clicked()
                                        {
                                            feedback_action =
                                                Some((block.message_id.clone(), -1));
                                        }
                                        if ui
                                            .add_enabled(
                                                !cortex_busy,
                                                egui::Button::new("+"),
                                            )
                                            .clicked()
                                        {
                                            feedback_action =
                                                Some((block.message_id.clone(), 1));
                                        }
                                    },
                                );
                            }
                        });
                        if !block.path.is_empty() {
                            ui.label(
                                egui::RichText::new(&block.path)
                                    .monospace()
                                    .color(CYAN)
                                    .size(9.0),
                            );
                        }
                        ui.label(&block.text);
                    });
                    ui.add_space(4.0);
                }

                if cortex_active && !self.cortex_live_text.is_empty() {
                    ui.group(|ui| {
                        ui.colored_label(CYAN, "Cortex · live");
                        ui.label(&self.cortex_live_text);
                    });
                }
            });

        if let Some((message_id, score)) = feedback_action {
            let result = self
                .cortex
                .as_mut()
                .ok_or_else(|| "Cortex host unavailable".to_owned())
                .and_then(|host| host.rate_message(&message_id, score));
            match result {
                Ok(()) => self.append("[PASS] Cortex response feedback recorded."),
                Err(error) => self.append(&format!("[FAIL] Cortex feedback: {error}")),
            }
        }

        if let Some(message_id) = redo_action {
            self.cortex_redo_target = message_id;
        }

        if !self.cortex_phase.is_empty() {
            ui.label(
                egui::RichText::new(&self.cortex_phase)
                    .color(MUTED)
                    .size(9.0),
            );
        }

        ui.separator();
        ui.horizontal_wrapped(|ui| {
            for (kind, label) in [
                (CortexRequestKind::Chat, "Chat"),
                (CortexRequestKind::Inspect, "Inspect"),
                (CortexRequestKind::Plan, "Plan"),
                (CortexRequestKind::Apply, "Apply"),
                (CortexRequestKind::Repair, "Repair"),
            ] {
                if ui
                    .selectable_label(self.cortex_mode == kind, label)
                    .clicked()
                {
                    self.cortex_mode = kind;
                }
            }
        });

        ui.add_sized(
            [ui.available_width(), 78.0],
            egui::TextEdit::multiline(&mut self.cortex_prompt)
                .hint_text("Ask Cortex about the active Forge project…"),
        );

        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !cortex_busy && !self.cortex_prompt.trim().is_empty(),
                    egui::Button::new(format!("Send {}", self.cortex_mode.label())),
                )
                .clicked()
            {
                self.start_cortex_request();
            }
            if ui
                .add_enabled(cortex_active, egui::Button::new("Stop Cortex"))
                .clicked()
            {
                self.cancel_cortex_request();
            }
        });

        if !self.cortex_redo_target.is_empty() {
            ui.separator();
            ui.label("Redo instructions");
            ui.text_edit_singleline(&mut self.cortex_redo_instructions);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!cortex_busy, egui::Button::new("Regenerate response"))
                    .clicked()
                {
                    let message_id = self.cortex_redo_target.clone();
                    let instructions = self.cortex_redo_instructions.clone();
                    let result = self
                        .cortex
                        .as_mut()
                        .ok_or_else(|| "Cortex host unavailable".to_owned())
                        .and_then(|host| host.start_redo(message_id, instructions));
                    match result {
                        Ok(()) => {
                            self.cortex_redo_target.clear();
                            self.cortex_redo_instructions.clear();
                            self.append("[INFO] Cortex response regeneration started.");
                        }
                        Err(error) => self.append(&format!("[FAIL] Cortex redo: {error}")),
                    }
                }
                if ui.button("Cancel Redo").clicked() {
                    self.cortex_redo_target.clear();
                    self.cortex_redo_instructions.clear();
                }
            });
        }
    }

    fn cortex_files_page(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &DesktopView,
        cortex_busy: bool,
    ) {
        ui.heading("Files / Workbench");
        ui.label(
            egui::RichText::new(
                "Read-only project/library browsing reuses Cortex's guarded file authority. Writable editing belongs to the Forge IDE.",
            )
            .color(MUTED),
        );

        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.cortex_file_scope_library,
                false,
                "Project",
            );
            ui.selectable_value(
                &mut self.cortex_file_scope_library,
                true,
                "Library",
            );
            ui.label("Directory");
            ui.text_edit_singleline(&mut self.cortex_directory);
        });

        let mut browse = false;
        let mut up = false;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!cortex_busy, egui::Button::new("Browse"))
                .clicked()
            {
                browse = true;
            }
            if ui
                .add_enabled(
                    !cortex_busy && !self.cortex_directory.trim().is_empty(),
                    egui::Button::new("Up"),
                )
                .clicked()
            {
                up = true;
            }
            ui.separator();
            ui.text_edit_singleline(&mut self.cortex_file_query);
            if ui
                .add_enabled(!cortex_busy, egui::Button::new("Search"))
                .clicked()
            {
                let query = self.cortex_file_query.trim().to_owned();
                let result = self
                    .cortex
                    .as_mut()
                    .ok_or_else(|| "Cortex host unavailable".to_owned())
                    .and_then(|host| host.show_files(&query));
                if let Err(error) = result {
                    self.append(&format!("[FAIL] Cortex file search: {error}"));
                }
            }
        });

        if up {
            let current = Path::new(self.cortex_directory.trim());
            self.cortex_directory = current
                .parent()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default();
            browse = true;
        }

        if browse {
            let directory = self.cortex_directory.trim().to_owned();
            let result = self
                .cortex
                .as_ref()
                .ok_or_else(|| "Cortex host unavailable".to_owned())
                .and_then(|host| host.browse_files(self.cortex_file_scope_library, &directory));
            match result {
                Ok(listing) => self.cortex_listing = Some(listing),
                Err(error) => self.append(&format!("[FAIL] Cortex browse: {error}")),
            }
        }

        let mut next_directory = None;
        let mut open_file = None;
        if let Some(listing) = &self.cortex_listing {
            ui.separator();
            ui.label(
                egui::RichText::new(format!(
                    "{} · {}",
                    listing.scope, listing.current
                ))
                .color(CYAN)
                .size(10.0),
            );
            let entries = listing.entries.clone();
            egui::ScrollArea::vertical()
                .max_height(250.0)
                .show(ui, |ui| {
                    for entry in entries {
                        let icon = if entry.is_directory { "[DIR]" } else { "[FILE]" };
                        if ui
                            .selectable_label(
                                false,
                                format!("{icon}  {}  {}", entry.name, human_bytes(entry.bytes)),
                            )
                            .clicked()
                        {
                            if entry.is_directory {
                                next_directory = Some(entry.relative_path);
                            } else if !self.cortex_file_scope_library {
                                open_file = Some(entry.relative_path);
                            }
                        }
                    }
                });
        } else {
            ui.separator();
            ui.label(egui::RichText::new("Browse to load a file listing.").color(MUTED));
        }

        if let Some(directory) = next_directory {
            self.cortex_directory = directory;
            if let Some(host) = self.cortex.as_ref() {
                match host.browse_files(self.cortex_file_scope_library, &self.cortex_directory) {
                    Ok(listing) => self.cortex_listing = Some(listing),
                    Err(error) => self.append(&format!("[FAIL] Cortex browse: {error}")),
                }
            }
        }
        if let Some(relative_path) = open_file {
            let result = self
                .cortex
                .as_mut()
                .ok_or_else(|| "Cortex host unavailable".to_owned())
                .and_then(|host| host.open_workbench_file(&relative_path));
            match result {
                Ok(document) => self.cortex_document = Some(document),
                Err(error) => self.append(&format!("[FAIL] Cortex workbench: {error}")),
            }
        }

        if self.cortex_file_scope_library {
            ui.label(
                egui::RichText::new(
                    "Library scope is browse/search only here. Open/edit is deliberately project-scoped.",
                )
                .color(MUTED)
                .size(9.0),
            );
        }

        if let Some(document) = &self.cortex_document {
            ui.separator();
            ui.label(
                egui::RichText::new(format!(
                    "{} · {} · {}{}",
                    document.path,
                    document.language,
                    human_bytes(document.bytes),
                    if document.truncated { " · truncated" } else { "" }
                ))
                .color(CYAN),
            );
            egui::ScrollArea::both()
                .max_height(260.0)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(&document.content)
                            .monospace()
                            .size(10.0),
                    );
                });
        }

        if !snapshot.info.trim().is_empty() {
            ui.separator();
            monospace_block(ui, "Cortex file/search result", &snapshot.info);
        }
    }

    fn cortex_context_page(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &DesktopView,
        cortex_busy: bool,
    ) {
        ui.heading("Project Context");
        ui.label(format!("Workspace: {}", snapshot.workspace_title));
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!cortex_busy, egui::Button::new("Show Context Index"))
                .clicked()
            {
                let result = self
                    .cortex
                    .as_mut()
                    .ok_or_else(|| "Cortex host unavailable".to_owned())
                    .and_then(CortexWorkspaceHost::show_context_index);
                if let Err(error) = result {
                    self.append(&format!("[FAIL] Cortex context index: {error}"));
                }
            }
            if ui
                .add_enabled(!cortex_busy, egui::Button::new("Rebuild Context Index"))
                .clicked()
            {
                self.start_cortex_task(CortexTaskKind::RebuildContext);
            }
        });
        ui.separator();
        monospace_block(ui, "Context / Info", &snapshot.info);
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Library / Vault context").strong());
        ui.label(&snapshot.library.summary);
        for item in snapshot.library.projects.iter().take(12) {
            ui.label(format!("Project: {item}"));
        }
        for item in snapshot.library.storage.iter().take(12) {
            ui.label(format!("Storage: {item}"));
        }
        if snapshot.library.truncated {
            ui.label(egui::RichText::new("Library view truncated").color(YELLOW));
        }
    }

    fn cortex_models_page(
        &mut self,
        ui: &mut egui::Ui,
        runtime: &CortexRuntimeSnapshot,
        cortex_busy: bool,
    ) {
        ui.heading("Models & Providers");
        connection_badge(ui, runtime.connection);
        ui.label(format!("Desktop API ready: {}", runtime.desktop_api_ready));
        ui.label(format!("Service registered: {}", runtime.service_registered));
        ui.label(format!(
            "Provider recovery pending: {}",
            runtime.provider_recovery_pending
        ));
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!cortex_busy, egui::Button::new("Probe Provider"))
                .clicked()
            {
                self.start_cortex_task(CortexTaskKind::ProviderProbe);
            }
            if ui
                .add_enabled(
                    !cortex_busy && runtime.provider_recovery_pending,
                    egui::Button::new("Cancel Pending Provider Request"),
                )
                .clicked()
            {
                let result = self
                    .cortex
                    .as_mut()
                    .ok_or_else(|| "Cortex host unavailable".to_owned())
                    .and_then(CortexWorkspaceHost::cancel_pending_provider_request);
                match result {
                    Ok(true) => self.append("[WARN] Pending Cortex provider request cancelled."),
                    Ok(false) => self.append("[INFO] No pending Cortex provider request."),
                    Err(error) => self.append(&format!("[FAIL] Provider cancel: {error}")),
                }
            }
        });
        if !runtime.error.is_empty() {
            ui.colored_label(YELLOW, &runtime.error);
        }
        ui.separator();
        monospace_block(ui, "Provider", &runtime.provider_json);
        monospace_block(ui, "Models", &runtime.models_json);
        monospace_block(ui, "Health", &runtime.health_json);
    }

    fn cortex_activity_page(&mut self, ui: &mut egui::Ui, snapshot: &DesktopView) {
        ui.heading("Cortex Activity");
        monospace_block(ui, "Activity", &snapshot.activity);
        monospace_block(ui, "Jobs", &snapshot.jobs);
        monospace_block(ui, "Build", &snapshot.build);
        monospace_block(ui, "Git", &snapshot.git);
        monospace_block(ui, "Vault", &snapshot.vault_activity);
        monospace_block(ui, "System", &snapshot.system_activity);
        monospace_block(ui, "Notifications", &snapshot.notifications);
        if !snapshot.native_model_log_root.is_empty() {
            ui.label(format!(
                "Native model logs: {}",
                snapshot.native_model_log_root
            ));
        }
    }

    fn cortex_reviews_page(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &DesktopView,
        cortex_busy: bool,
    ) {
        ui.heading("Reviews / Changes");
        ui.label(
            egui::RichText::new(
                "Cortex supplies analysis/review state; Forge retains universal source-control authority.",
            )
            .color(MUTED),
        );
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!cortex_busy, egui::Button::new("Refresh Changes"))
                .clicked()
            {
                let result = self
                    .cortex
                    .as_mut()
                    .ok_or_else(|| "Cortex host unavailable".to_owned())
                    .and_then(CortexWorkspaceHost::show_changes);
                if let Err(error) = result {
                    self.append(&format!("[FAIL] Cortex changes: {error}"));
                }
            }
            if ui
                .add_enabled(!cortex_busy, egui::Button::new("Safety Checkpoint"))
                .clicked()
            {
                self.start_cortex_task(CortexTaskKind::CreateSafetyCheckpoint);
            }
            if ui
                .add_enabled(!cortex_busy, egui::Button::new("Push Repository Local"))
                .clicked()
            {
                self.start_cortex_task(CortexTaskKind::PushRepositoryLocal);
            }
        });
        ui.separator();
        monospace_block(ui, "Review / info", &snapshot.info);
        monospace_block(ui, "Git / change state", &snapshot.git);
        monospace_block(ui, "Build / verification state", &snapshot.build);
        monospace_block(ui, "Notifications", &snapshot.notifications);
    }

    fn cortex_vault_page(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &DesktopView,
        cortex_busy: bool,
    ) {
        ui.heading("Cortex Library / Vault Intelligence");
        ui.label(
            egui::RichText::new(
                "Cortex may search/catalog the library. Forge owns Vault storage authority, patch intake and Artifact Central lifecycle.",
            )
            .color(MUTED),
        );
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.cortex_vault_query);
            if ui
                .add_enabled(!cortex_busy, egui::Button::new("Search Vault"))
                .clicked()
            {
                let query = self.cortex_vault_query.trim().to_owned();
                let result = self
                    .cortex
                    .as_mut()
                    .ok_or_else(|| "Cortex host unavailable".to_owned())
                    .and_then(|host| host.search_vault(&query));
                if let Err(error) = result {
                    self.append(&format!("[FAIL] Cortex Vault search: {error}"));
                }
            }
            if ui
                .add_enabled(!cortex_busy, egui::Button::new("Show Artifacts"))
                .clicked()
            {
                let result = self
                    .cortex
                    .as_mut()
                    .ok_or_else(|| "Cortex host unavailable".to_owned())
                    .and_then(CortexWorkspaceHost::show_artifacts);
                if let Err(error) = result {
                    self.append(&format!("[FAIL] Cortex artifacts: {error}"));
                }
            }
        });

        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!cortex_busy, egui::Button::new("Scan Library"))
                .clicked()
            {
                self.start_cortex_task(CortexTaskKind::ScanLibrary);
            }
            if ui
                .add_enabled(!cortex_busy, egui::Button::new("Storage Preview"))
                .clicked()
            {
                self.start_cortex_task(CortexTaskKind::ScanStorageCatalog);
            }
            if ui
                .add_enabled(!cortex_busy, egui::Button::new("Machine Preview"))
                .clicked()
            {
                self.start_cortex_task(CortexTaskKind::ScanMachineCatalog);
            }
            if ui
                .add_enabled(!cortex_busy, egui::Button::new("Repository Vault Audit"))
                .clicked()
            {
                self.start_cortex_task(CortexTaskKind::RefreshRepositoryVaultAudit);
            }
        });

        ui.separator();
        ui.label("Ingest project/reference paths (one path per line)");
        ui.add_sized(
            [ui.available_width(), 54.0],
            egui::TextEdit::multiline(&mut self.cortex_ingest_paths),
        );
        if ui
            .add_enabled(
                !cortex_busy && !self.cortex_ingest_paths.trim().is_empty(),
                egui::Button::new("Ingest Paths"),
            )
            .clicked()
        {
            let paths = self
                .cortex_ingest_paths
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let result = self
                .cortex
                .as_mut()
                .ok_or_else(|| "Cortex host unavailable".to_owned())
                .and_then(|host| host.start_ingest_paths(paths));
            match result {
                Ok(()) => self.append("[INFO] Cortex path ingestion started."),
                Err(error) => self.append(&format!("[FAIL] Cortex ingest: {error}")),
            }
        }

        ui.separator();
        ui.label(&snapshot.library.summary);
        for row in snapshot.library.lineage.iter().take(8) {
            ui.label(format!("Lineage: {row}"));
        }
        for row in snapshot.library.inbox.iter().take(8) {
            ui.label(format!("Inbox: {row}"));
        }
        monospace_block(ui, "Vault / result", &snapshot.info);
    }

    fn cortex_tasks_page(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &DesktopView,
        cortex_busy: bool,
    ) {
        ui.heading("Cortex Project Tasks");
        ui.label(
            egui::RichText::new(
                "Long Cortex operations run on a background controller so the Forge shell remains responsive.",
            )
            .color(MUTED),
        );

        for (kind, label) in [
            (CortexTaskKind::RefreshRepositoryProvider, "Refresh Repository Provider"),
            (CortexTaskKind::RefreshRepositoryVaultAudit, "Refresh Repository Vault Audit"),
            (CortexTaskKind::CreateSafetyCheckpoint, "Create Safety Checkpoint"),
            (CortexTaskKind::PushRepositoryLocal, "Push Repository Local"),
            (CortexTaskKind::BuildCheck, "Run Cortex Build Check"),
            (CortexTaskKind::RebuildContext, "Rebuild Context"),
            (CortexTaskKind::ScanLibrary, "Scan Library"),
            (CortexTaskKind::ScanStorageCatalog, "Storage Catalog Preview"),
            (CortexTaskKind::ScanMachineCatalog, "Machine Catalog Preview"),
        ] {
            if ui
                .add_enabled(!cortex_busy, egui::Button::new(label))
                .clicked()
            {
                self.start_cortex_task(kind);
            }
        }

        let cortex_active = self
            .cortex
            .as_ref()
            .is_some_and(CortexWorkspaceHost::busy);
        if cortex_active {
            ui.separator();
            let active = self
                .cortex
                .as_ref()
                .map(CortexWorkspaceHost::active_label)
                .unwrap_or("");
            ui.colored_label(CYAN, format!("Running: {active}"));
            if ui.button("Request Stop").clicked() {
                self.cancel_cortex_request();
            }
            ui.label(
                egui::RichText::new(
                    "Catalog scans are cooperatively cancellable. Other donor operations may finish their current safe unit before returning.",
                )
                .color(MUTED)
                .size(9.0),
            );
        }

        ui.separator();
        monospace_block(ui, "Tasks", &snapshot.jobs);
        monospace_block(ui, "Activity", &snapshot.activity);
    }

    fn cortex_settings_page(
        &mut self,
        ui: &mut egui::Ui,
        runtime: &CortexRuntimeSnapshot,
        cortex_busy: bool,
    ) {
        ui.heading("Cortex Settings");
        ui.label(
            egui::RichText::new(
                "Only Cortex runtime/provider settings are editable here. Vault roots and universal Forge settings remain Forge-owned.",
            )
            .color(MUTED),
        );

        if let Some(host) = self.cortex.as_ref() {
            match host.config_snapshot() {
                Ok(config) => {
                    ui.label(format!("Provider: {}", config.provider));
                    ui.label(format!("Service port: {}", config.service_port));
                    ui.label(format!("Native model host port: {}", config.native_model_host_port));
                    ui.label(format!("LM Studio: {}", config.lmstudio_url));
                    ui.label(format!("ComfyUI: {}", config.comfyui_url));
                    ui.label(format!("Chat model: {}", config.chat_model));
                    ui.label(format!("Tool model: {}", config.tool_model));
                    ui.label(format!("Vision model: {}", config.vision_model));
                    ui.label(format!("Embedding model: {}", config.embedding_model));
                    ui.label(
                        egui::RichText::new(format!(
                            "Forge-owned library root: {}",
                            if config.library_root.is_empty() {
                                "not configured"
                            } else {
                                &config.library_root
                            }
                        ))
                        .color(MUTED),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "Forge-owned offsite root: {}",
                            if config.offsite_backup_root.is_empty() {
                                "not configured"
                            } else {
                                &config.offsite_backup_root
                            }
                        ))
                        .color(MUTED),
                    );
                }
                Err(error) => {
                    ui.colored_label(YELLOW, format!("Configuration snapshot unavailable: {error}"));
                }
            }
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Key");
            ui.text_edit_singleline(&mut self.cortex_setting_key);
        });
        ui.horizontal(|ui| {
            ui.label("Value");
            ui.text_edit_singleline(&mut self.cortex_setting_value);
        });

        if ui
            .add_enabled(!cortex_busy, egui::Button::new("Apply Cortex Setting"))
            .clicked()
        {
            let key = self.cortex_setting_key.trim().to_owned();
            let value = self.cortex_setting_value.trim().to_owned();
            let result = self
                .cortex
                .as_mut()
                .ok_or_else(|| "Cortex host unavailable".to_owned())
                .and_then(|host| host.update_setting(&key, &value));
            match result {
                Ok(()) => self.append(&format!("[PASS] Cortex setting updated: {key}")),
                Err(error) => self.append(&format!("[FAIL] Cortex setting: {error}")),
            }
        }

        ui.separator();
        ui.label(format!("Service registered: {}", runtime.service_registered));
        ui.label(format!("Desktop API ready: {}", runtime.desktop_api_ready));
        ui.label(format!("Conversations: {}", runtime.conversation_count));
        if !runtime.error.is_empty() {
            ui.colored_label(YELLOW, &runtime.error);
        }
    }

    fn cortex_parity_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Cortex → Forge Normalization Parity");
        ui.label(
            egui::RichText::new(
                "Integrated means the Forge-hosted Cortex surface calls the existing Cortex authority. Forge authority means the capability intentionally belongs outside Cortex. Donor ready means the old native shell still has UX we have not migrated yet.",
            )
            .color(MUTED),
        );
        ui.separator();
        egui::ScrollArea::vertical()
            .max_height(610.0)
            .show(ui, |ui| {
                for item in cortex_parity_items() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            let color = match item.state.as_str() {
                                "integrated" => GREEN,
                                "forge_authority" => CYAN,
                                _ => YELLOW,
                            };
                            ui.colored_label(color, item.state.as_str().to_ascii_uppercase());
                            ui.label(egui::RichText::new(item.label).strong());
                        });
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · authority={}",
                                item.id, item.authority
                            ))
                            .monospace()
                            .size(9.0)
                            .color(MUTED),
                        );
                    });
                }
            });
    }

    fn start_cortex_task(&mut self, kind: CortexTaskKind) {
        if self.busy {
            self.append("[WARN] A Forge project operation is active; Cortex task execution is held until it finishes.");
            return;
        }
        let result = self
            .cortex
            .as_mut()
            .ok_or_else(|| "Cortex host unavailable".to_owned())
            .and_then(|host| host.start_task(kind));
        match result {
            Ok(()) => self.append(&format!("[INFO] Cortex {} started.", kind.label())),
            Err(error) => self.append(&format!("[FAIL] Cortex {}: {error}", kind.label())),
        }
    }

    fn activate_project(&mut self, project_id: &str, root: PathBuf) {
        if self.busy || self.cortex.as_ref().is_some_and(CortexWorkspaceHost::busy) {
            self.append("[WARN] Project activation is blocked while Forge/Cortex work is active.");
            return;
        }
        let session = match ProjectSession::load(&root) {
            Ok(session) => session,
            Err(error) => {
                self.append(&format!("[FAIL] Project activation contract: {error}"));
                return;
            }
        };
        let (cortex, cortex_error) = match CortexWorkspaceHost::open(&root) {
            Ok(host) => (Some(host), String::new()),
            Err(error) => (None, error),
        };
        self.root = root.clone();
        self.session = Ok(session);
        self.operation_history = recent_operation_receipts(&root, 32).unwrap_or_default();
        self.active_operation = None;
        self.active_pid = None;
        self.busy = false;
        self.cortex = cortex;
        self.cortex_error = cortex_error;
        self.cortex_listing = None;
        self.cortex_document = None;
        self.cortex_live_text.clear();
        self.cortex_phase.clear();
        let state_message = if let Some(store) = self.forge_state.as_mut() {
            match store.register_project_root(&root) {
                Err(error) => Some(format!("[WARN] Forge registry activation update: {error}")),
                Ok(_) => store
                    .set_active_project(project_id)
                    .err()
                    .map(|error| format!("[WARN] Forge active-project state: {error}")),
            }
        } else {
            None
        };
        if let Some(message) = state_message {
            self.append(&message);
        }
        self.recalculate_health();
        self.append(&format!("[PASS] Forge active project changed to {project_id}: {}", root.display()));
    }

    fn projects_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Projects / Fleet");
        ui.label(
            egui::RichText::new(
                "Rust Forge keeps a machine-level registry of project.control.json authorities and preserves nested project relationships.",
            )
            .color(MUTED),
        );
        ui.separator();

        let mut scan_requested = false;
        let mut add_root_requested = false;
        let mut activate: Option<(String, PathBuf)> = None;

        let registry_view = self.forge_state.as_ref().map(|store| {
            (
                store.root().to_path_buf(),
                store.snapshot().clone(),
            )
        });
        if let Some((state_root, snapshot)) = registry_view {
            ui.label(format!("State root: {}", state_root.display()));
            ui.label(format!("Registered projects: {}", snapshot.projects.len()));
            ui.label(format!(
                "Active project id: {}",
                snapshot.active_project_id.as_deref().unwrap_or("none")
            ));
            ui.horizontal(|ui| {
                if ui.button("Scan configured roots").clicked() {
                    scan_requested = true;
                }
                ui.label(format!(
                    "depth={} directory-budget={}",
                    snapshot.settings.max_scan_depth, snapshot.settings.max_scan_directories
                ));
            });
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.forge_scan_root_input);
                if ui.button("Add scan root").clicked() {
                    add_root_requested = true;
                }
            });
            ui.separator();

            let active_project_id = snapshot.active_project_id.clone();
            egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
                for project in snapshot.projects {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&project.name).strong());
                            ui.label(
                                egui::RichText::new(format!("[{}]", project.kind))
                                    .color(MUTED)
                                    .size(9.0),
                            );
                            if active_project_id.as_deref() == Some(project.id.as_str()) {
                                ui.colored_label(GREEN, "ACTIVE");
                            } else if ui.button("Activate").clicked() {
                                activate = Some((project.id.clone(), project.root.clone()));
                            }
                        });
                        ui.label(
                            egui::RichText::new(project.root.display().to_string())
                                .monospace()
                                .size(9.0)
                                .color(MUTED),
                        );
                        if let Some(parent) = &project.parent_id {
                            ui.label(format!("Parent: {parent}"));
                        }
                        if !project.child_ids.is_empty() {
                            ui.label(format!("Children: {}", project.child_ids.join(", ")));
                        }
                        if !project.duplicate_roots.is_empty() {
                            ui.colored_label(
                                YELLOW,
                                format!("{} duplicate/lineage root(s) need review", project.duplicate_roots.len()),
                            );
                        }
                    });
                }
            });
        } else {
            ui.colored_label(RED, "Forge state registry unavailable");
            ui.label(&self.forge_state_error);
        }

        if add_root_requested {
            let value = self.forge_scan_root_input.trim().to_owned();
            if value.is_empty() {
                self.append("[WARN] Forge scan root is empty.");
            } else {
                let result = self
                    .forge_state
                    .as_mut()
                    .ok_or_else(|| "Forge state unavailable".to_owned())
                    .and_then(|store| store.add_scan_root(PathBuf::from(&value)));
                match result {
                    Ok(()) => {
                        self.forge_scan_root_input.clear();
                        self.append(&format!("[PASS] Forge scan root registered: {value}"));
                    }
                    Err(error) => self.append(&format!("[FAIL] Forge scan root: {error}")),
                }
            }
        }
        if scan_requested {
            let result = self
                .forge_state
                .as_mut()
                .ok_or_else(|| "Forge state unavailable".to_owned())
                .and_then(ForgeStateStore::scan_projects);
            match result {
                Ok(report) => self.append(&format!(
                    "[PASS] Forge fleet scan: {} project(s), {} directories, duplicates={}, truncated={}",
                    report.project_count,
                    report.scanned_directories,
                    report.duplicate_project_ids,
                    report.truncated
                )),
                Err(error) => self.append(&format!("[FAIL] Forge fleet scan: {error}")),
            }
        }
        if let Some((project_id, root)) = activate {
            self.activate_project(&project_id, root);
        }
    }

    fn vault_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Vault / Artifact Central");
        if let Some(store) = self.forge_state.as_ref() {
            let snapshot = store.snapshot();
            ui.label(format!(
                "Artifact Central: {}",
                snapshot
                    .settings
                    .artifact_central_root
                    .as_ref()
                    .map_or_else(|| "not configured".to_owned(), |path| path.display().to_string())
            ));
            ui.label(format!("Indexed artifact records: {}", snapshot.artifacts.len()));
            ui.label(format!("Patch lineage records: {}", snapshot.patch_lineage.len()));
            ui.label(format!(
                "Downloads auto-queue: {} (approval remains required)",
                snapshot.settings.downloads_auto_queue
            ));
            ui.label(format!(
                "Patch stale window: {} hour(s)",
                snapshot.settings.patch_stale_age_hours
            ));
            if !snapshot.settings.intake_roots.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("Configured intake roots").strong());
                for root in &snapshot.settings.intake_roots {
                    ui.label(root.display().to_string());
                }
            }
            ui.separator();
            for record in snapshot.patch_lineage.iter().rev().take(12) {
                ui.label(format!(
                    "{} · {:?} · {}",
                    record.patch_id, record.disposition, record.reason
                ));
            }
        } else {
            ui.colored_label(RED, "Forge state unavailable");
        }
        ui.separator();
        if let Ok(session) = &self.session {
            let capabilities = session.capabilities();
            ui.label(format!("Active project artifact descriptors: {}", capabilities.artifacts.len()));
            for artifact in capabilities.artifacts {
                ui.label(format!("{} → {}", artifact.key, artifact.path));
            }
        }
        ui.separator();
        ui.label(
            egui::RichText::new(
                "ForgePY remains production Artifact Central/patch authority until the cumulative RS04–RS293 native candidates pass gate, runtime smoke and takeover certification.",
            )
            .color(MUTED),
        );
    }

    fn source_control_tab(&mut self, ui: &mut egui::Ui) {
        self.source_control_workspace(ui);
        ui.separator();
        if let Some(store) = self.forge_state.as_ref() {
            let active = store.snapshot().active_project_id.as_deref();
            if let Some(record) = store
                .snapshot()
                .source_control
                .iter()
                .find(|record| Some(record.project_id.as_str()) == active)
            {
                ui.label(egui::RichText::new("GitHub lane").strong());
                ui.label(format!("Configured: {}", record.github.configured));
                ui.label(format!("Authority: {}", record.github.authority));
                ui.label(format!("Location: {}", record.github.location.as_deref().unwrap_or("unknown")));
                ui.label(egui::RichText::new("Forge Repository / Internal Git lane").strong());
                ui.label(format!("Configured: {}", record.internal_git.configured));
                ui.label(format!("Authority: {}", record.internal_git.authority));
                ui.label(format!("Location: {}", record.internal_git.location.as_deref().unwrap_or("unknown")));
            } else {
                ui.label(
                    egui::RichText::new(
                        "Dual-lane source-control state remains visible here. The cumulative candidate includes Rust Git inspection/Internal-Git/GitHub command backends plus branch/history operations, pending gate and richer UI wiring.",
                    )
                    .color(MUTED),
                );
            }
        }
        ui.separator();
        ui.label(
            egui::RichText::new(
                "GitHub and Forge Repository/Internal Git are separate first-class Forge authorities shared by Cortex and Ember.",
            )
            .color(MUTED),
        );
    }

    fn ide_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("IDE");
            ui.colored_label(CYAN, "NATIVE RUST");
            ui.label(egui::RichText::new("No WebView / Monaco dependency").color(MUTED));
        });
        let capabilities = native_ide_capabilities();
        ui.label(format!(
            "Native rendering={} · multi-tab={} · undo/redo={} · guarded save={} · LSP contract={} · DAP contract={}",
            capabilities.native_rendering,
            capabilities.multi_tab,
            capabilities.undo_redo,
            capabilities.guarded_save,
            capabilities.lsp_contract,
            capabilities.dap_contract
        ));
        ui.separator();

        if self.ide_session.is_none() {
            ui.colored_label(RED, "Native IDE session unavailable");
            ui.label(&self.ide_error);
            if ui.button("Retry Native IDE").clicked() {
                match EditorSession::open(&self.root) {
                    Ok(session) => {
                        self.ide_file_cache = session.workspace().list_text_files(4_000).unwrap_or_default();
                        self.ide_session = Some(session);
                        self.ide_error.clear();
                        self.ide_status = "Native IDE session restored".to_owned();
                    }
                    Err(error) => self.ide_error = error,
                }
            }
            return;
        }

        let mut refresh_files = false;
        let mut open_path = None;
        let mut activate_tab = None;
        let mut close_tab = None;
        let mut save = false;
        let mut undo = false;
        let mut redo = false;
        let mut refresh_conflict = false;

        ui.horizontal(|ui| {
            ui.label("Open");
            ui.add(
                egui::TextEdit::singleline(&mut self.ide_path_input)
                    .hint_text("project-relative path, e.g. crates/foo/src/lib.rs"),
            );
            if ui.button("Open File").clicked() && !self.ide_path_input.trim().is_empty() {
                open_path = Some(self.ide_path_input.trim().to_owned());
            }
            if ui.button("Refresh Files").clicked() {
                refresh_files = true;
            }
        });

        ui.columns(2, |columns| {
            columns[0].set_min_width(260.0);
            columns[0].vertical(|ui| {
                ui.label(egui::RichText::new("PROJECT FILES").color(MUTED).size(10.0));
                ui.add(
                    egui::TextEdit::singleline(&mut self.ide_file_filter)
                        .hint_text("filter files"),
                );
                ui.separator();
                let filter = self.ide_file_filter.to_ascii_lowercase();
                egui::ScrollArea::vertical().max_height(650.0).show(ui, |ui| {
                    for path in self.ide_file_cache.iter().filter(|path| {
                        filter.is_empty() || path.to_ascii_lowercase().contains(&filter)
                    }).take(1200) {
                        if ui.selectable_label(false, path).clicked() {
                            open_path = Some(path.clone());
                        }
                    }
                });
            });

            columns[1].vertical(|ui| {
                let summaries = self
                    .ide_session
                    .as_ref()
                    .map(EditorSession::tab_summaries)
                    .unwrap_or_default();
                ui.horizontal_wrapped(|ui| {
                    for (index, tab) in summaries.iter().enumerate() {
                        let name = Path::new(&tab.relative_path)
                            .file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or(&tab.relative_path);
                        let marker = if tab.conflict {
                            "!"
                        } else if tab.dirty {
                            "*"
                        } else {
                            ""
                        };
                        if ui
                            .selectable_label(tab.active, format!("{name}{marker}"))
                            .clicked()
                        {
                            activate_tab = Some(index);
                        }
                        if tab.active && ui.small_button("×").clicked() {
                            close_tab = Some(index);
                        }
                    }
                });
                ui.separator();

                let active_snapshot = self.ide_session.as_ref().and_then(|session| {
                    session.active_buffer().map(|buffer| {
                        (
                            buffer.document().relative_path.clone(),
                            buffer.document().language.clone(),
                            buffer.content().to_owned(),
                            buffer.dirty(),
                            buffer.conflict(),
                        )
                    })
                });

                if let Some((path, language, mut content, dirty, conflict)) = active_snapshot {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&path).monospace().strong());
                        ui.label(egui::RichText::new(&language).color(CYAN));
                        if dirty {
                            ui.colored_label(YELLOW, "DIRTY");
                        }
                        if conflict {
                            ui.colored_label(RED, "DISK CONFLICT");
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Save").clicked() {
                                save = true;
                            }
                            if ui.button("Redo").clicked() {
                                redo = true;
                            }
                            if ui.button("Undo").clicked() {
                                undo = true;
                            }
                            if ui.button("Check Disk").clicked() {
                                refresh_conflict = true;
                            }
                        });
                    });
                    ui.separator();

                    let response = ui.add_sized(
                        [ui.available_width(), 500.0],
                        egui::TextEdit::multiline(&mut content)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY),
                    );
                    if response.changed() {
                        if let Some(session) = self.ide_session.as_mut() {
                            if let Err(error) = session.replace_active_content(content) {
                                self.ide_status = error;
                            }
                        }
                    }

                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Find");
                        if ui
                            .add(egui::TextEdit::singleline(&mut self.ide_find).hint_text("text in active file"))
                            .changed()
                        {
                            self.ide_find_hits = self
                                .ide_session
                                .as_ref()
                                .map(|session| session.find_active(&self.ide_find, 100))
                                .unwrap_or_default();
                        }
                        ui.label(format!("{} hit(s)", self.ide_find_hits.len()));
                    });
                    egui::ScrollArea::vertical().max_height(100.0).show(ui, |ui| {
                        for hit in self.ide_find_hits.iter().take(30) {
                            ui.label(format!("{}:{}  {}", hit.line, hit.column, hit.preview));
                        }
                    });

                    if let Some(session) = self.ide_session.as_ref() {
                        let tools = session.workspace().language_tools(&language);
                        if !tools.is_empty() {
                            ui.separator();
                            ui.label(egui::RichText::new("LANGUAGE TOOLS").color(MUTED).size(10.0));
                            for tool in tools {
                                ui.label(format!(
                                    "{} · {:?} · {}",
                                    tool.program,
                                    tool.kind,
                                    if tool.available { "available" } else { "not found" }
                                ));
                            }
                        }
                    }
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new("Open a project source file to begin editing")
                                .color(MUTED),
                        );
                    });
                }
            });
        });

        if refresh_files {
            if let Some(session) = self.ide_session.as_ref() {
                match session.workspace().list_text_files(4_000) {
                    Ok(files) => {
                        self.ide_file_cache = files;
                        self.ide_status = format!("Indexed {} text files", self.ide_file_cache.len());
                    }
                    Err(error) => self.ide_status = error,
                }
            }
        }
        if let Some(path) = open_path {
            if let Some(session) = self.ide_session.as_mut() {
                match session.open_tab(&path) {
                    Ok(_) => {
                        self.ide_path_input = path.clone();
                        self.ide_find_hits.clear();
                        self.ide_status = format!("Opened {path}");
                    }
                    Err(error) => self.ide_status = error,
                }
            }
        }
        if let Some(index) = activate_tab {
            if let Some(session) = self.ide_session.as_mut() {
                if let Err(error) = session.activate(index) {
                    self.ide_status = error;
                } else {
                    self.ide_find_hits.clear();
                }
            }
        }
        if let Some(index) = close_tab {
            if let Some(session) = self.ide_session.as_mut() {
                if let Err(error) = session.close_tab(index, false) {
                    self.ide_status = error;
                }
            }
        }
        if undo {
            if let Some(session) = self.ide_session.as_mut() {
                if !session.undo() {
                    self.ide_status = "Nothing to undo".to_owned();
                }
            }
        }
        if redo {
            if let Some(session) = self.ide_session.as_mut() {
                if !session.redo() {
                    self.ide_status = "Nothing to redo".to_owned();
                }
            }
        }
        if refresh_conflict {
            if let Some(session) = self.ide_session.as_mut() {
                match session.refresh_conflict() {
                    Ok(true) => self.ide_status = "Disk conflict detected; save is blocked".to_owned(),
                    Ok(false) => self.ide_status = "Disk preimage still matches".to_owned(),
                    Err(error) => self.ide_status = error,
                }
            }
        }
        if save {
            if let Some(session) = self.ide_session.as_mut() {
                match session.save_active() {
                    Ok(document) => {
                        self.ide_status = format!("Saved {}", document.relative_path);
                        self.append(&format!("[PASS] IDE saved {}", document.relative_path));
                    }
                    Err(error) => {
                        self.ide_status = error.clone();
                        self.append(&format!("[FAIL] IDE save: {error}"));
                    }
                }
            }
        }
        if !self.ide_status.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new(&self.ide_status).color(MUTED));
        }
    }

    fn ember_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Ember");
        ui.label(
            egui::RichText::new("Game authoring workspace")
                .color(CYAN)
                .strong(),
        );
        ui.label(
            "Ember is the game editing system hosted by Forge. Forge owns the workstation/project/tooling shell; Cortex supplies intelligence; Ember owns game-authoring behavior.",
        );
        ui.separator();
        match inspect_ember(&self.root) {
            Ok(status) => {
                let color = match status.state {
                    EmberHostState::Ready => GREEN,
                    EmberHostState::Configured => CYAN,
                    EmberHostState::Degraded => YELLOW,
                    EmberHostState::Unconfigured => MUTED,
                };
                ui.colored_label(color, format!("EMBER HOST: {:?}", status.state));
                ui.label(status.detail);
                if let Some(adapter) = status.adapter {
                    ui.label(format!("Capabilities: {}", adapter.capabilities.join(", ")));
                    for surface in adapter.surfaces {
                        ui.label(format!(
                            "{} · {} · {}",
                            surface.label,
                            surface.id,
                            if surface.required { "required" } else { "optional" }
                        ));
                    }
                }
            }
            Err(error) => ui.colored_label(RED, format!("Ember adapter error: {error}")),
        };
        ui.separator();
        ui.label("Hosted editor families remain:");
        for item in [
            "World / scene authoring",
            "Sprite, pixel and animation authoring",
            "Tiles / terrain / level authoring",
            "Node graphs and gameplay logic",
            "Asset/content browser",
            "UI/audio/dialogue authoring",
            "Runtime / PIE / validation",
            "Cortex-assisted editor actions",
        ] {
            ui.label(format!("• {item}"));
        }
        ui.separator();
        ui.colored_label(
            YELLOW,
            "The Forge host contract is real; Ember editor components are migrated only when their actual native implementations are ready.",
        );
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Forge Settings / Services");
        ui.label(format!("Version: {VERSION}"));
        ui.label(format!("Active project root: {}", self.root.display()));
        ui.label(format!("Project UI state: {}", ui_state_path(&self.root).display()));
        ui.horizontal(|ui| {
            if ui.button("Toolchain Doctor").clicked() {
                match doctor_project(&self.root) {
                    Ok(report) => self.append(&format!(
                        "[{}] Toolchain doctor: {} tool(s), required_ready={}",
                        if report.required_ready { "PASS" } else { "FAIL" },
                        report.statuses.len(),
                        report.required_ready
                    )),
                    Err(error) => self.append(&format!("[FAIL] Toolchain doctor: {error}")),
                }
            }
            if ui.button("Native Diagnostics").clicked() {
                let report = collect_diagnostics(&self.root);
                self.append(&format!(
                    "[{}] Native diagnostics: contract={} git={} state={} operations={}",
                    if report.overall_ready { "PASS" } else { "WARN" },
                    report.project_contract_present,
                    report.git.is_some(),
                    report.forge_state_ready,
                    report.operation_receipts
                ));
            }
        });
        let mut write_diagnostics = false;
        if let Some(store) = self.forge_state.as_ref() {
            let snapshot = store.snapshot();
            ui.label(format!("Machine state root: {}", store.root().display()));
            ui.separator();
            ui.label(egui::RichText::new("Services").strong());
            if snapshot.services.is_empty() {
                ui.label(egui::RichText::new("No persisted service records yet.").color(MUTED));
            }
            for service in &snapshot.services {
                ui.label(format!("{} · {:?} · {}", service.label, service.state, service.detail));
            }
            ui.separator();
            ui.label(egui::RichText::new("Takeover certification").strong());
            for check in &snapshot.takeover.checks {
                let color = match check.status {
                    TakeoverStatus::Verified | TakeoverStatus::NotApplicable => GREEN,
                    TakeoverStatus::Candidate => CYAN,
                    TakeoverStatus::Missing => YELLOW,
                    TakeoverStatus::Blocked => RED,
                };
                ui.colored_label(color, format!("{:?} · {}", check.status, check.label));
                if !check.evidence.is_empty() {
                    ui.label(egui::RichText::new(&check.evidence).color(MUTED).size(9.0));
                }
            }
            ui.label(format!(
                "Takeover ready: {}",
                snapshot.takeover.ready_for_takeover()
            ));
            if ui.button("Write machine-state diagnostics snapshot").clicked() {
                write_diagnostics = true;
            }
        } else {
            ui.colored_label(RED, "Forge machine state is unavailable");
            ui.label(&self.forge_state_error);
        }
        if write_diagnostics {
            if let Some(store) = self.forge_state.as_mut() {
                match store.write_diagnostics_snapshot() {
                    Ok(path) => self.append(&format!("[PASS] Forge diagnostics state: {}", path.display())),
                    Err(error) => self.append(&format!("[FAIL] Forge diagnostics state: {error}")),
                }
            }
        }
        ui.separator();
        ui.label(
            egui::RichText::new(
                "Forge settings own universal services, fleet, source control, Artifact Central and shell state. Cortex provider/model settings remain inside the Cortex workspace.",
            )
            .color(MUTED),
        );
    }

    fn central_tab(&mut self, ui: &mut egui::Ui) {
        match self.tab {
            AppTab::Projects => self.projects_tab(ui),
            AppTab::Workspace => self.workspace(ui),
            AppTab::Vault => self.vault_tab(ui),
            AppTab::SourceControl => self.source_control_tab(ui),
            AppTab::Ide => self.ide_tab(ui),
            AppTab::Ember => self.ember_tab(ui),
            AppTab::Cortex => self.cortex_workspace(ui),
            AppTab::Settings => self.settings_tab(ui),
        }
    }

    fn save_ui_state(&self) {
        let state = PersistedUiState {
            tab: self.tab,
            workspace_page: self.workspace_page,
            cortex_page: self.cortex_page,
            cortex_mode: self.cortex_mode.label().to_ascii_lowercase(),
        };
        let path = ui_state_path(&self.root);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&state) {
            let _ = fs::write(path, bytes);
        }
    }
}

impl Drop for ForgeApp {
    fn drop(&mut self) {
        self.save_ui_state();
    }
}

impl eframe::App for ForgeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_visuals(egui::Visuals::dark());
        self.drain_operation_events(ctx);
        self.drain_cortex_events(ctx);
        self.header(ctx);
        self.app_rail(ctx);
        self.health_rail(ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.visuals_mut().panel_fill = BG;
            self.central_tab(ui);
        });
    }
}

fn monospace_block(ui: &mut egui::Ui, title: &str, body: &str) {
    ui.label(egui::RichText::new(title).strong());
    if body.trim().is_empty() {
        ui.label(egui::RichText::new("No data").color(MUTED));
    } else {
        egui::ScrollArea::vertical()
            .max_height(155.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new(body).monospace().size(10.0));
            });
    }
    ui.add_space(4.0);
}

fn connection_badge(ui: &mut egui::Ui, state: CortexConnectionState) {
    let (label, color) = match state {
        CortexConnectionState::Ready => ("CORTEX READY", GREEN),
        CortexConnectionState::Connecting => ("CORTEX CONNECTING", CYAN),
        CortexConnectionState::Disconnected => ("CORTEX OFFLINE", YELLOW),
        CortexConnectionState::Error => ("CORTEX ATTENTION", RED),
    };
    ui.colored_label(color, label);
}

fn action_button(ui: &mut egui::Ui, label: &str, disabled: bool) -> bool {
    ui.add_enabled(!disabled, egui::Button::new(label).fill(PANEL_2))
        .clicked()
}

fn status_row(ui: &mut egui::Ui, label: &str, ok: bool) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.colored_label(if ok { GREEN } else { RED }, if ok { "PASS" } else { "FAIL" });
        });
    });
}

fn state_color(state: OperationState) -> egui::Color32 {
    match state {
        OperationState::Succeeded => GREEN,
        OperationState::Queued | OperationState::Running => CYAN,
        OperationState::Cancelled | OperationState::Interrupted => YELLOW,
        OperationState::Failed | OperationState::StartFailed => RED,
    }
}

fn semantic_color(text: &str) -> egui::Color32 {
    if text.contains("[FAIL]") || text.contains(" FAIL") {
        RED
    } else if text.contains("[WARN]") || text.contains(" WARN") {
        YELLOW
    } else if text.contains("[PASS]") || text.contains(" PASS") {
        GREEN
    } else if text.contains("[INFO]") {
        MUTED
    } else {
        TEXT
    }
}

fn draw_health_gauge(ui: &mut egui::Ui, score: f32) {
    use std::f32::consts::PI;
    let score = score.clamp(0.0, 1.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(145.0, 82.0), egui::Sense::hover());
    let center = egui::pos2(rect.center().x, rect.bottom() - 5.0);
    let radius = 57.0;
    let painter = ui.painter();
    let arc = |fraction: f32| {
        let steps = 40;
        (0..=steps)
            .map(|index| {
                let t = index as f32 / steps as f32;
                let angle = PI - PI * t * fraction;
                egui::pos2(
                    center.x + radius * angle.cos(),
                    center.y - radius * angle.sin(),
                )
            })
            .collect::<Vec<_>>()
    };
    painter.add(egui::Shape::line(
        arc(1.0),
        egui::Stroke::new(9.0, BORDER),
    ));
    let color = if score > 0.9 {
        GREEN
    } else if score > 0.65 {
        YELLOW
    } else {
        RED
    };
    painter.add(egui::Shape::line(
        arc(score),
        egui::Stroke::new(9.0, color),
    ));
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 23.0),
        egui::Align2::CENTER_CENTER,
        format!("{}", (score * 100.0).round() as u32),
        egui::FontId::proportional(24.0),
        TEXT,
    );
}

fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * KIB;
    const GIB: f64 = 1024.0 * MIB;
    let value = bytes as f64;
    if value >= GIB {
        format!("{:.1} GiB", value / GIB)
    } else if value >= MIB {
        format!("{:.1} MiB", value / MIB)
    } else if value >= KIB {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn parse_cortex_mode(value: &str) -> CortexRequestKind {
    match value {
        "inspect" => CortexRequestKind::Inspect,
        "plan" => CortexRequestKind::Plan,
        "apply" => CortexRequestKind::Apply,
        "repair" => CortexRequestKind::Repair,
        _ => CortexRequestKind::Chat,
    }
}

fn ui_state_path(root: &Path) -> PathBuf {
    root.join("artifacts")
        .join("forge-rust")
        .join("ui")
        .join("forge-ui-state.json")
}

fn load_ui_state(root: &Path) -> PersistedUiState {
    let path = ui_state_path(root);
    fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<PersistedUiState>(&bytes).ok())
        .unwrap_or_default()
}

fn parse_root() -> PathBuf {
    let mut args = env::args_os().skip(1);
    let mut root = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    while let Some(arg) = args.next() {
        if arg == "--root" {
            if let Some(value) = args.next() {
                root = PathBuf::from(value);
            }
        }
    }
    root
}

fn main() -> eframe::Result<()> {
    let root = parse_root();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("Forge — Rust {VERSION}"))
            .with_inner_size([1660.0, 940.0])
            .with_min_inner_size([1180.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Forge",
        options,
        Box::new(move |_creation_context| Ok(Box::new(ForgeApp::new(root.clone())))),
    )
}
