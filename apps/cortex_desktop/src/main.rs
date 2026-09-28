use cortex_desktop_core::DesktopController;
use cortex_desktop_native::{
    DesktopChatBlock, DesktopConfigSnapshot, DesktopFileEntry, DesktopFileListing, DesktopHost,
    DesktopLibraryView, DesktopSnapshot, DesktopWorkbenchDocument, DesktopWorker,
};
use cortex_registry::WorkspaceRegistry;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("Cortex Desktop failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let certify = args
        .iter()
        .any(|argument| argument.to_string_lossy() == "--certify");
    let workspace = resolve_startup_workspace(&args)?;

    bind_portable_desktop_environment(&workspace)?;
    let controller = DesktopController::open(&workspace)?;

    if certify {
        let report = controller.certify();
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
        if report.ready {
            println!("Cortex Desktop certification: READY");
            return Ok(());
        }
        return Err("Cortex Desktop certification failed".into());
    }

    // Provider/service startup is lazy. Opening Cortex must never wait on
    // LM Studio or network/provider health before showing the desktop shell.
    cortex_desktop_native::run(HostAdapter { controller })
}

fn resolve_startup_workspace(args: &[OsString]) -> Result<PathBuf, String> {
    if let Some(explicit) = args
        .iter()
        .find(|argument| !argument.to_string_lossy().starts_with("--"))
    {
        return canonical_directory(PathBuf::from(explicit.as_os_str()));
    }

    for key in ["CORTEX_WORKSPACE", "OPEN2D_CORTEX_WORKSPACE"] {
        if let Some(value) = std::env::var_os(key).filter(|value| !value.is_empty()) {
            return canonical_directory(PathBuf::from(value));
        }
    }

    // When cortex_desktop.exe is launched directly from target/.../debug or
    // target/.../release, Explorer commonly makes that build-output directory
    // the process working directory. Walk upward to the actual project root
    // instead of registering the target directory as a generic workspace.
    if let Ok(current) = std::env::current_dir() {
        if let Some(root) = discover_workspace_ancestor(&current) {
            return Ok(root);
        }
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            if let Some(root) = discover_workspace_ancestor(parent) {
                return Ok(root);
            }
        }
    }

    // Installed/relocated Cortex builds may have no project marker near the
    // executable. In that case reopen the user's last active registered
    // workspace before falling back to an arbitrary current directory.
    if let Ok(registry) = WorkspaceRegistry::open_default() {
        if let Ok(Some(active)) = registry.active() {
            if active.root.is_dir() {
                return canonical_directory(active.root);
            }
        }
        if let Ok(workspaces) = registry.list() {
            if let Some(known) = workspaces.into_iter().find(|entry| entry.root.is_dir()) {
                return canonical_directory(known.root);
            }
        }
    }

    canonical_directory(std::env::current_dir().map_err(|error| error.to_string())?)
}

/// Direct exe launching must inherit the same marked-volume authority as PCC.
/// Only Cortex's own checkout on that volume may bind portable storage here;
/// user-supplied environment overrides are never overwritten.
fn portable_desktop_environment(
    workspace: &Path,
) -> Result<Option<Vec<(&'static str, PathBuf)>>, String> {
    let Some(volume) = workspace.parent() else {
        return Ok(None);
    };
    let marker_path = volume.join(".cortex-volume.json");
    if !marker_path.is_file() {
        return Ok(None);
    }
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(&marker_path).map_err(|error| error.to_string())?)
            .map_err(|error| {
                format!(
                    "invalid portable volume marker {}: {error}",
                    marker_path.display()
                )
            })?;
    if marker.get("schema").and_then(serde_json::Value::as_str) != Some("cortex.volume.v1")
        || marker
            .get("volumeId")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err(format!(
            "invalid Cortex portable volume marker: {}",
            marker_path.display()
        ));
    }
    let layout_path = workspace
        .join("config")
        .join("cortex")
        .join("volume_layout.v2.json");
    if !layout_path.is_file() {
        return Ok(None);
    }
    let layout: serde_json::Value =
        serde_json::from_slice(&fs::read(&layout_path).map_err(|error| error.to_string())?)
            .map_err(|error| {
                format!(
                    "invalid portable volume layout {}: {error}",
                    layout_path.display()
                )
            })?;
    if layout.get("schema").and_then(serde_json::Value::as_str) != Some("cortex.volume_layout.v2") {
        return Err(format!(
            "invalid portable volume layout schema: {}",
            layout_path.display()
        ));
    }
    let authorities = layout
        .get("authorities")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            format!(
                "missing portable layout authorities: {}",
                layout_path.display()
            )
        })?;
    let authority = |key: &str| -> Result<PathBuf, String> {
        use std::path::Component;
        let value = authorities
            .get(key)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("missing portable layout authority: {key}"))?;
        let relative = Path::new(value);
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(format!("unsafe portable layout authority: {key}"));
        }
        Ok(volume.join(relative))
    };
    if fs::canonicalize(authority("cortex")?).map_err(|error| error.to_string())? != workspace {
        return Ok(None); // Another registered project on this volume is not Cortex's checkout.
    }
    Ok(Some(vec![
        ("CORTEX_VAULT_ROOT", volume.to_path_buf()),
        ("PCC_VAULT_ROOT", volume.to_path_buf()),
        ("CORTEX_HOME", authority("state")?.join("home")),
        ("CORTEX_RUNTIME_ROOT", workspace.to_path_buf()),
        ("CORTEX_PORTABLE_VOLUME_ROOT", volume.to_path_buf()),
        ("CORTEX_PROJECTS_ROOT", authority("projects")?),
        ("CORTEX_MODELS_ROOT", authority("models")?),
        ("CORTEX_LOCAL_GIT_ROOT", authority("git")?),
    ]))
}

fn bind_portable_desktop_environment(workspace: &Path) -> Result<(), String> {
    if let Some(values) = portable_desktop_environment(workspace)? {
        for (name, value) in values {
            if std::env::var_os(name).is_none_or(|existing| existing.is_empty()) {
                std::env::set_var(name, value);
            }
        }
        if std::env::var_os("CORTEX_STATE_MODE").is_none() {
            std::env::set_var("CORTEX_STATE_MODE", "portable");
        }
    }
    Ok(())
}

fn canonical_directory(path: PathBuf) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(&path).map_err(|error| {
        format!(
            "failed to resolve Cortex workspace {}: {error}",
            path.display()
        )
    })?;
    if canonical.is_dir() {
        Ok(canonical)
    } else {
        Err(format!(
            "Cortex workspace is not a directory: {}",
            canonical.display()
        ))
    }
}

fn discover_workspace_ancestor(start: &Path) -> Option<PathBuf> {
    let canonical = fs::canonicalize(start).ok()?;
    let mut fallback = None;
    for ancestor in canonical.ancestors() {
        if is_strong_workspace_root(ancestor) {
            return Some(ancestor.to_path_buf());
        }
        if fallback.is_none() && is_workspace_marker(ancestor) {
            fallback = Some(ancestor.to_path_buf());
        }
    }
    fallback
}

fn is_strong_workspace_root(path: &Path) -> bool {
    if path.join(".git").exists()
        || path
            .join("config")
            .join("architecture")
            .join("foundry_dependency_policy.json")
            .is_file()
    {
        return true;
    }
    fs::read_to_string(path.join("Cargo.toml"))
        .map(|manifest| manifest.contains("[workspace]"))
        .unwrap_or(false)
}

fn is_workspace_marker(path: &Path) -> bool {
    path.join("Cargo.toml").is_file()
        || path.join("package.json").is_file()
        || path.join("pyproject.toml").is_file()
        || path.join("CMakeLists.txt").is_file()
}

struct HostAdapter {
    controller: DesktopController,
}

struct WorkerAdapter {
    controller: DesktopController,
}

impl DesktopWorker for WorkerAdapter {
    fn send_message_stream(
        &mut self,
        text: &str,
        on_event: &mut dyn FnMut(cortex_desktop_native::CortexStreamEvent),
    ) -> Result<(), String> {
        self.controller.send_chat_stream(text, on_event)
    }

    fn run_agent(&mut self, mode: &str, prompt: &str) -> Result<(), String> {
        self.controller.run_agent(mode, prompt).map(|_| ())
    }

    // O2D-R051N9M9A5_SAFE_AGENT_STREAM_TELEMETRY
    fn run_agent_stream(
        &mut self,
        mode: &str,
        prompt: &str,
        on_event: &mut dyn FnMut(cortex_desktop_native::CortexStreamEvent),
    ) -> Result<(), String> {
        self.controller
            .run_agent_stream(mode, prompt, on_event)
            .map(|_| ())
    }
    fn show_provider_status(&mut self) -> Result<(), String> {
        self.controller.show_provider_status()
    }

    fn refresh_repository_provider(&mut self) -> Result<(), String> {
        self.controller.refresh_repository_provider()
    }

    fn refresh_repository_vault_audit(&mut self) -> Result<(), String> {
        self.controller.refresh_repository_vault_audit()
    }

    fn create_repository_safety_checkpoint(&mut self) -> Result<(), String> {
        self.controller.create_repository_safety_checkpoint()
    }

    fn push_repository_local(&mut self) -> Result<(), String> {
        self.controller.push_repository_local()
    }

    fn run_build_check(&mut self) -> Result<(), String> {
        self.controller.run_build_check()
    }

    fn rebuild_context_index(&mut self) -> Result<(), String> {
        self.controller
            .rebuild_context_index(50_000, 512 * 1024)
            .map(|_| ())
    }

    fn ingest_paths(&mut self, paths: &[String]) -> Result<(), String> {
        self.controller.ingest_paths(paths)
    }

    fn scan_library(&mut self) -> Result<(), String> {
        self.controller.scan_library().map(|_| ())
    }

    fn scan_storage_catalog_preview(
        &mut self,
        should_cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), String> {
        self.controller
            .scan_storage_catalog_preview(should_cancel)
            .map(|_| ())
    }

    fn scan_machine_catalog_preview(
        &mut self,
        include_removable: bool,
        should_cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), String> {
        self.controller
            .scan_machine_catalog_preview(include_removable, should_cancel)
            .map(|_| ())
    }

    fn redo_message(&mut self, message_id: &str, instructions: &str) -> Result<(), String> {
        self.controller.redo_response(message_id, instructions)
    }

    fn persist_state(&mut self) -> Result<(), String> {
        self.controller.persist_background_state()
    }
}

impl DesktopHost for HostAdapter {
    fn snapshot(&mut self) -> DesktopSnapshot {
        let view = self.controller.view();
        DesktopSnapshot {
            workspace_title: view.workspace_title,
            workspaces: view.workspaces,
            active_workspace: view.active_workspace,
            conversations: view.conversations,
            active_conversation: view.active_conversation,
            conversation_title: view.conversation_title,
            transcript: view.transcript,
            chat_blocks: view
                .chat_blocks
                .into_iter()
                .map(|block| DesktopChatBlock {
                    message_id: block.message_id,
                    role: block.role,
                    label: block.label,
                    text: block.text,
                    kind: block.kind,
                    path: block.path,
                    language: block.language,
                    status: block.status,
                    created_unix_ms: block.created_unix_ms,
                    feedback_score: block.feedback_score,
                    revision: block.revision,
                })
                .collect(),
            library: DesktopLibraryView {
                summary: view.library.summary,
                projects: view.library.projects,
                lineage: view.library.lineage,
                storage: view.library.storage,
                inbox: view.library.inbox,
                plans: view.library.plans,
                recovery: view.library.recovery,
                scan_ready: view.library.scan_ready,
                truncated: view.library.truncated,
            },
            info: view.info,
            activity: view.activity,
            jobs: view.jobs,
            build: view.build,
            git: view.git,
            vault_activity: view.vault_activity,
            system_activity: view.system_activity,
            native_model_log_root: view.native_model_log_root,
            notifications: view.notifications,
            status: view.status,
            provider_recovery_pending: self.controller.provider_recovery_pending(),
        }
    }

    fn attach_workspace(&mut self, path: &str) -> Result<(), String> {
        self.controller.attach_workspace(path)
    }

    fn set_library_root(&mut self, path: &str) -> Result<(), String> {
        self.controller.set_library_root(path).map(|_| ())
    }

    fn config_snapshot(&mut self) -> Result<DesktopConfigSnapshot, String> {
        let settings = self.controller.settings().clone();
        let storage = self.controller.storage_authority_record()?;
        let storage_identity = storage
            .as_ref()
            .and_then(|record| record.volume.as_ref())
            .map(|volume| {
                format!(
                    "{}{}{}",
                    volume
                        .mount_root
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "unknown mount".into()),
                    volume
                        .filesystem
                        .as_ref()
                        .map(|value| format!(" · {value}"))
                        .unwrap_or_default(),
                    volume
                        .free_bytes
                        .map(|value| format!(" · {value} bytes free"))
                        .unwrap_or_default()
                )
            })
            .unwrap_or_else(|| {
                "Volume identity will be captured when the storage authority is saved.".into()
            });
        Ok(DesktopConfigSnapshot {
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
            storage_identity,
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

    fn set_config_value(&mut self, key: &str, value: &str) -> Result<(), String> {
        if key == "library_root" {
            self.controller.set_library_root(value).map(|_| ())
        } else if key == "offsite_backup_root" {
            if value.trim().is_empty() {
                let _ = self
                    .controller
                    .storage_authority_record()?
                    .ok_or_else(|| "Cortex storage authority is not configured".to_string())?;
                self.controller
                    .registry_clear_offsite_backup_root()
                    .map(|_| ())
            } else {
                self.controller.set_offsite_backup_root(value).map(|_| ())
            }
        } else {
            self.controller.update_setting(key, value)
        }
    }

    fn scan_library(&mut self) -> Result<(), String> {
        self.controller.scan_library().map(|_| ())
    }

    fn rate_message(&mut self, message_id: &str, score: i8) -> Result<(), String> {
        self.controller.rate_response(message_id, score)
    }

    fn record_error_message(&mut self, context: &str, error: &str) -> Result<(), String> {
        self.controller.record_desktop_error(context, error)
    }

    fn switch_workspace(&mut self, index: usize) -> Result<(), String> {
        self.controller.switch_workspace(index)
    }

    fn new_conversation(&mut self) -> Result<(), String> {
        self.controller.new_conversation()
    }

    fn archive_conversation(&mut self) -> Result<(), String> {
        self.controller.archive_current_conversation()
    }

    fn select_conversation(&mut self, index: usize) -> Result<(), String> {
        self.controller.select_conversation(index)
    }

    fn export_conversation(&mut self, index: usize, format: &str) -> Result<String, String> {
        self.controller
            .export_conversation(index, format)
            .map(|path| path.display().to_string())
    }

    fn export_project_conversations(&mut self, format: &str) -> Result<String, String> {
        self.controller
            .export_project_conversations(format)
            .map(|path| path.display().to_string())
    }

    fn cancel_pending_request(&mut self) -> Result<bool, String> {
        self.controller.cancel_pending_provider_request()
    }

    fn cancel_active_request(&mut self) -> Result<bool, String> {
        self.controller.cancel_active_request()
    }

    fn send_message(&mut self, text: &str) -> Result<(), String> {
        self.controller.send_chat(text)
    }

    fn send_message_stream(
        &mut self,
        text: &str,
        on_event: &mut dyn FnMut(cortex_desktop_native::CortexStreamEvent),
    ) -> Result<(), String> {
        self.controller.send_chat_stream(text, on_event)
    }

    fn show_files(&mut self, query: &str) -> Result<(), String> {
        self.controller.show_files(query)
    }

    fn open_workbench_file(
        &mut self,
        relative_path: &str,
    ) -> Result<DesktopWorkbenchDocument, String> {
        let document = self.controller.open_workbench_file(relative_path)?;
        Ok(DesktopWorkbenchDocument {
            path: document.path,
            language: document.language,
            content: document.content,
            bytes: document.bytes,
            truncated: document.truncated,
        })
    }

    fn browse_files(
        &mut self,
        library_scope: bool,
        relative_directory: &str,
    ) -> Result<DesktopFileListing, String> {
        let listing = self
            .controller
            .browse_files(library_scope, relative_directory)?;
        Ok(DesktopFileListing {
            scope: listing.scope,
            root: listing.root,
            current: listing.current,
            entries: listing
                .entries
                .into_iter()
                .map(|entry| DesktopFileEntry {
                    name: entry.name,
                    relative_path: entry.relative_path,
                    is_directory: entry.is_directory,
                    bytes: entry.bytes,
                })
                .collect(),
        })
    }

    fn show_changes(&mut self) -> Result<(), String> {
        self.controller.show_changes()
    }

    fn search_vault(&mut self, query: &str) -> Result<(), String> {
        self.controller.search_vault(query)
    }

    fn show_artifacts(&mut self) -> Result<(), String> {
        self.controller.show_artifacts()
    }

    fn show_tasks(&mut self) -> Result<(), String> {
        self.controller.show_tasks()
    }

    fn show_settings(&mut self) -> Result<(), String> {
        self.controller.show_settings()
    }

    fn show_provider_status(&mut self) -> Result<(), String> {
        self.controller.show_provider_status()
    }

    fn probe_provider(&mut self) -> Result<(), String> {
        let _ = self.controller.probe_provider_quiet();
        Ok(())
    }

    fn run_build_check(&mut self) -> Result<(), String> {
        self.controller.run_build_check()
    }

    fn rebuild_context_index(&mut self) -> Result<(), String> {
        self.controller
            .rebuild_context_index(50_000, 512 * 1024)
            .map(|_| ())
    }

    fn show_context_index(&mut self) -> Result<(), String> {
        self.controller.show_context_index()
    }

    fn run_agent(&mut self, mode: &str, prompt: &str) -> Result<(), String> {
        self.controller.run_agent(mode, prompt).map(|_| ())
    }

    fn spawn_worker(&mut self) -> Result<Box<dyn DesktopWorker>, String> {
        Ok(Box::new(WorkerAdapter {
            controller: self.controller.fork_for_background()?,
        }))
    }

    fn refresh(&mut self) -> Result<(), String> {
        self.controller.sync_pending_state()?;
        self.controller.refresh_overview()
    }
}
