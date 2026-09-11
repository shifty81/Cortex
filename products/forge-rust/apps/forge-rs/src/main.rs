use eframe::egui;
use forge_core::ProjectSession;
use forge_process::{spawn, OperationEvent};
use std::env;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

const VERSION: &str = "0.1.0-FR01";
const BG: egui::Color32 = egui::Color32::from_rgb(9, 11, 14);
const PANEL_2: egui::Color32 = egui::Color32::from_rgb(23, 28, 34);
const BORDER: egui::Color32 = egui::Color32::from_rgb(40, 49, 58);
const TEXT: egui::Color32 = egui::Color32::from_rgb(237, 242, 245);
const MUTED: egui::Color32 = egui::Color32::from_rgb(146, 154, 163);
const CYAN: egui::Color32 = egui::Color32::from_rgb(0, 217, 255);
const GREEN: egui::Color32 = egui::Color32::from_rgb(67, 240, 113);
const YELLOW: egui::Color32 = egui::Color32::from_rgb(255, 212, 74);
const RED: egui::Color32 = egui::Color32::from_rgb(255, 93, 104);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppTab {
    Projects,
    Workspace,
    Vault,
    SourceControl,
    Ide,
    Cortex,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspacePage {
    Dashboard,
    BuildRun,
    Updates,
    SourceControl,
    Diagnostics,
    Tooling,
    Advanced,
}

#[derive(Debug, Clone)]
struct ConsoleLine {
    text: String,
    color: egui::Color32,
}

struct ForgeApp {
    session: Result<ProjectSession, String>,
    tab: AppTab,
    workspace_page: WorkspacePage,
    console: Vec<ConsoleLine>,
    operation_tx: Sender<OperationEvent>,
    operation_rx: Receiver<OperationEvent>,
    busy: bool,
    active_pid: Option<u32>,
    health_score: f32,
}

impl ForgeApp {
    fn new(root: PathBuf) -> Self {
        let (operation_tx, operation_rx) = mpsc::channel();
        let session = ProjectSession::load(&root).map_err(|error| error.to_string());
        let mut app = Self {
            session,
            tab: AppTab::Workspace,
            workspace_page: WorkspacePage::Updates,
            console: Vec::new(),
            operation_tx,
            operation_rx,
            busy: false,
            active_pid: None,
            health_score: 0.0,
        };
        app.append("[INFO] Forge Rust parallel application lane started.");
        if let Ok(session) = &app.session {
            app.append(&format!("[PASS] Active project: {}", session.contract.display_name()));
            if session.provider_ready() {
                app.append("[PASS] Project-native provider ready.");
            } else {
                app.append("[WARN] No project-native provider resolved; direct contract commands only.");
            }
        } else if let Err(error) = &app.session {
            app.append(&format!("[FAIL] {error}"));
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
        self.health_score = match &self.session {
            Ok(session) if session.provider_ready() => 1.0,
            Ok(_) => 0.72,
            Err(_) => 0.2,
        };
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.operation_rx.try_recv() {
            match event {
                OperationEvent::Started { label, pid } => {
                    self.busy = true;
                    self.active_pid = Some(pid);
                    self.append(&format!("[INFO] START {label} (pid {pid})"));
                }
                OperationEvent::Output { stderr, line } => {
                    if stderr && !line.contains("[PASS]") && !line.contains("[WARN]") {
                        self.console.push(ConsoleLine { text: line, color: MUTED });
                    } else {
                        self.append(&line);
                    }
                }
                OperationEvent::Finished { label, success, code, elapsed_ms } => {
                    self.busy = false;
                    self.active_pid = None;
                    let token = if success { "PASS" } else { "FAIL" };
                    self.append(&format!(
                        "[{token}] END {label} ({elapsed_ms} ms, exit {})",
                        code.map_or_else(|| "?".to_owned(), |value| value.to_string())
                    ));
                }
                OperationEvent::FailedToStart { label, error } => {
                    self.busy = false;
                    self.active_pid = None;
                    self.append(&format!("[FAIL] START {label}: {error}"));
                }
            }
            ctx.request_repaint();
        }
    }

    fn start_operation(&mut self, operation: &str) {
        if self.busy {
            self.append("[WARN] An operation is already running.");
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
            Ok(spec) => spawn(spec, self.operation_tx.clone()),
            Err(error) => self.append(&format!("[FAIL] {error}")),
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
                        self.append("[PASS] Rust Forge project state refreshed.");
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
            .exact_width(118.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("FORGE WORKSPACES").color(MUTED).size(10.0));
                ui.add_space(8.0);
                for (tab, label) in [
                    (AppTab::Projects, "Projects"),
                    (AppTab::Workspace, "Workspace"),
                    (AppTab::Vault, "Vault"),
                    (AppTab::SourceControl, "Source Control"),
                    (AppTab::Ide, "IDE"),
                    (AppTab::Cortex, "Cortex"),
                    (AppTab::Settings, "Settings"),
                ] {
                    let selected = self.tab == tab;
                    if ui.selectable_label(selected, label).clicked() {
                        self.tab = tab;
                    }
                    ui.add_space(4.0);
                }
            });
    }

    fn health_rail(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("health_rail")
            .exact_width(175.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("FORGE HEALTH").color(MUTED).size(10.0));
                ui.add_space(8.0);
                draw_health_gauge(ui, self.health_score);
                ui.separator();
                status_row(ui, "Contract", self.session.is_ok());
                status_row(
                    ui,
                    "Provider",
                    self.session.as_ref().is_ok_and(ProjectSession::provider_ready),
                );
                status_row(ui, "Rust Shell", true);
                status_row(ui, "Console", true);
                ui.separator();
                if self.busy {
                    ui.colored_label(YELLOW, "Operation running");
                    if let Some(pid) = self.active_pid {
                        ui.label(egui::RichText::new(format!("PID {pid}")).color(MUTED));
                    }
                } else {
                    ui.colored_label(GREEN, "Ready");
                }
                ui.add_space(8.0);
                ui.label(egui::RichText::new("PARITY").color(MUTED).size(10.0));
                ui.label(egui::RichText::new("FR01 vertical slice").color(TEXT));
                ui.label(egui::RichText::new("ForgePY remains production authority").color(MUTED));
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
        });
        ui.separator();
    }

    fn workspace(&mut self, ui: &mut egui::Ui) {
        self.quick_actions(ui);
        ui.columns(2, |columns| {
            columns[0].set_min_width(330.0);
            columns[0].vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_min_width(125.0);
                        ui.label(egui::RichText::new("PROJECT OPERATIONS").color(MUTED).size(10.0));
                        for (page, label) in [
                            (WorkspacePage::Dashboard, "Dashboard"),
                            (WorkspacePage::BuildRun, "Build & Run"),
                            (WorkspacePage::Updates, "Updates"),
                            (WorkspacePage::SourceControl, "Source Control"),
                            (WorkspacePage::Diagnostics, "Diagnostics"),
                            (WorkspacePage::Tooling, "Tooling"),
                            (WorkspacePage::Advanced, "Advanced Commands"),
                        ] {
                            if ui.selectable_label(self.workspace_page == page, label).clicked() {
                                self.workspace_page = page;
                            }
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_min_width(220.0);
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
                }
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
            WorkspacePage::Updates => {
                ui.heading("Updates");
                ui.label(egui::RichText::new(
                    "FR01 reads the project update contract but does not yet implement ForgePY patch intake."
                ).color(MUTED));
                if action_button(ui, "Patch Status", self.busy) {
                    self.start_operation("patch.status");
                }
                if action_button(ui, "Apply Validated Project Queue", self.busy) {
                    self.start_operation("patch.apply");
                }
            }
            WorkspacePage::SourceControl => {
                ui.heading("Source Control");
                if action_button(ui, "Git Status", self.busy) {
                    self.start_operation("git.status");
                }
                if action_button(ui, "Commit + Push GREEN", self.busy) {
                    self.start_operation("git.commit-push-green");
                }
                ui.label(egui::RichText::new(
                    "ForgePY Internal Git parity is scheduled after the project-operation spine is certified."
                ).color(MUTED));
            }
            WorkspacePage::Diagnostics => {
                ui.heading("Diagnostics");
                if action_button(ui, "Create Debug Bundle", self.busy) {
                    self.start_operation("diagnostics.bundle");
                }
                if action_button(ui, "Doctor Status", self.busy) {
                    self.start_operation("doctor.status");
                }
            }
            WorkspacePage::Tooling => {
                ui.heading("Tooling");
                ui.label(egui::RichText::new(
                    "Tool inventory, Blender, dependency doctor, and cross-project build queue are parity passes, not FR01 stubs."
                ).color(MUTED));
            }
            WorkspacePage::Advanced => {
                ui.heading("Advanced Commands");
                let commands: Vec<(String, String)> = self.session.as_ref().map_or_else(
                    |_| Vec::new(),
                    |session| session.contract.commands.iter().map(|command| {
                        (command.key.clone(), if command.label.is_empty() { command.key.clone() } else { command.label.clone() })
                    }).collect(),
                );
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for (key, label) in commands {
                        if action_button(ui, &label, self.busy) {
                            self.start_operation(&key);
                        }
                    }
                });
            }
        }
    }

    fn console_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.colored_label(CYAN, egui::RichText::new("PROJECT CONSOLE").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Clear").clicked() {
                    self.console.clear();
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
                    ui.label(egui::RichText::new(&line.text).monospace().color(line.color));
                }
            });
    }

    fn non_workspace(&mut self, ui: &mut egui::Ui) {
        match self.tab {
            AppTab::Projects => {
                ui.heading("Projects");
                ui.label("FR01 starts with the active project. Fleet registry/discovery parity follows.");
            }
            AppTab::Vault => {
                ui.heading("Vault");
                ui.label("Artifact Central, drive catalog, lineage, and intake move here in parity passes.");
            }
            AppTab::SourceControl => {
                ui.heading("Source Control");
                ui.label("GitHub + Forge Internal Git parity is a dedicated subsystem pass.");
                if action_button(ui, "Project Git Status", self.busy) {
                    self.start_operation("git.status");
                }
            }
            AppTab::Ide => {
                ui.heading("IDE");
                ui.label("Monaco will be hosted in a separate Rust WebView window, preserving the proven ForgePY isolation model.");
            }
            AppTab::Cortex => {
                ui.heading("Cortex");
                ui.colored_label(YELLOW, "Cortex bridge boundary exists; chat/runtime is not connected in FR01.");
                ui.label("No fake assistant responses are generated by this scaffold.");
            }
            AppTab::Settings => {
                ui.heading("Settings");
                ui.label("Persistent settings/service/model pages arrive after the operation spine is GREEN.");
            }
            AppTab::Workspace => {}
        }
    }
}

impl eframe::App for ForgeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_visuals(egui::Visuals::dark());
        self.drain_events(ctx);
        self.header(ctx);
        self.app_rail(ctx);
        self.health_rail(ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.visuals_mut().panel_fill = BG;
            if self.tab == AppTab::Workspace {
                self.workspace(ui);
            } else {
                self.non_workspace(ui);
            }
        });
    }
}

fn action_button(ui: &mut egui::Ui, label: &str, disabled: bool) -> bool {
    ui.add_enabled(!disabled, egui::Button::new(label).fill(PANEL_2)).clicked()
}

fn status_row(ui: &mut egui::Ui, label: &str, ok: bool) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.colored_label(if ok { GREEN } else { RED }, if ok { "PASS" } else { "FAIL" });
        });
    });
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
                egui::pos2(center.x + radius * angle.cos(), center.y - radius * angle.sin())
            })
            .collect::<Vec<_>>()
    };
    painter.add(egui::Shape::line(arc(1.0), egui::Stroke::new(9.0, BORDER)));
    let color = if score > 0.9 { GREEN } else if score > 0.65 { YELLOW } else { RED };
    painter.add(egui::Shape::line(arc(score), egui::Stroke::new(9.0, color)));
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 23.0),
        egui::Align2::CENTER_CENTER,
        format!("{}", (score * 100.0).round() as u32),
        egui::FontId::proportional(24.0),
        TEXT,
    );
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
            .with_inner_size([1600.0, 900.0])
            .with_min_inner_size([1100.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Forge",
        options,
        Box::new(move |_creation_context| Ok(Box::new(ForgeApp::new(root.clone())))),
    )
}
