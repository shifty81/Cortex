//! Global standalone Cortex workspace registry.
//!
//! The registry lives outside individual repositories and remembers arbitrary
//! attached workspaces. It is deliberately project-agnostic: Open2D, Rust,
//! Node, Python, CMake and plain folders all enter through `cortex_workspace`.

use cortex_project::{CapabilityId, ProjectAuthority, ProjectContract, ProjectId};
use cortex_workspace::{discover_cortex_home, Workspace, WorkspaceProfile};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const REGISTRY_SCHEMA_VERSION: u32 = 3;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisteredWorkspace {
    pub id: String,
    #[serde(default)]
    pub project_id: Option<ProjectId>,
    #[serde(default)]
    pub authority: ProjectAuthority,
    #[serde(default)]
    pub capabilities: BTreeSet<CapabilityId>,
    #[serde(default)]
    pub contract_path: Option<PathBuf>,
    pub name: String,
    pub root: PathBuf,
    #[serde(default)]
    pub known_paths: Vec<PathBuf>,
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub state_root: PathBuf,
    pub profile: WorkspaceProfile,
    pub attached_unix_ms: u128,
    pub last_opened_unix_ms: u128,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRegistryState {
    pub schema_version: u32,
    pub active_workspace_id: Option<String>,
    pub workspaces: Vec<RegisteredWorkspace>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CortexStorageVolumeIdentity {
    #[serde(default)]
    pub volume_guid: Option<String>,
    #[serde(default)]
    pub serial_number: Option<String>,
    #[serde(default)]
    pub filesystem: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub drive_type: Option<String>,
    #[serde(default)]
    pub mount_root: Option<PathBuf>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub free_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CortexLibraryRoot {
    pub schema_version: u32,
    pub root: PathBuf,
    pub configured_unix_ms: u128,
    #[serde(default)]
    pub volume: Option<CortexStorageVolumeIdentity>,
    #[serde(default)]
    pub offsite_backup_root: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CortexLibraryLayout {
    pub schema_version: u32,
    pub root: PathBuf,
    pub projects: PathBuf,
    pub vault: PathBuf,
    pub vault_assets: PathBuf,
    pub vault_logic: PathBuf,
    pub vault_templates: PathBuf,
    pub vault_code: PathBuf,
    pub vault_knowledge: PathBuf,
    pub vault_generated: PathBuf,
    pub vault_provenance: PathBuf,
    pub models: PathBuf,
    pub models_chat: PathBuf,
    pub models_coding: PathBuf,
    pub models_reasoning: PathBuf,
    pub models_vision: PathBuf,
    pub models_embeddings: PathBuf,
    pub models_imported: PathBuf,
    pub models_image_generation: PathBuf,
    pub image_checkpoints: PathBuf,
    pub image_diffusion_models: PathBuf,
    pub image_vae: PathBuf,
    pub image_lora: PathBuf,
    pub image_controlnet: PathBuf,
    pub image_upscalers: PathBuf,
    pub image_embeddings: PathBuf,
    pub image_workflows: PathBuf,
    pub image_imported: PathBuf,
    pub intake: PathBuf,
    pub intake_unassigned: PathBuf,
    pub shared: PathBuf,
    pub artifacts: PathBuf,
    pub backups: PathBuf,
    pub recovery: PathBuf,
    pub recycle_bin: PathBuf,
    pub exports: PathBuf,
    pub git: PathBuf,
    pub git_mirrors: PathBuf,
    pub git_recovery: PathBuf,
    pub cortex_data: PathBuf,
    pub cortex_search: PathBuf,
    pub cortex_memory: PathBuf,
    pub cortex_tasks: PathBuf,
    pub cortex_logs: PathBuf,
    pub internal: PathBuf,
    pub migration_staging: PathBuf,
}

impl CortexLibraryLayout {
    fn from_root(root: PathBuf) -> Self {
        let intake = root.join("Intake");
        let vault = root.join("Vault");
        let git = root.join("Git");
        let cortex_data = root.join(".cortex").join("data");
        Self {
            schema_version: 2,
            projects: root.join("Source"),
            vault_assets: vault.join("Assets"),
            vault_logic: vault.join("Logic"),
            vault_templates: vault.join("Templates"),
            vault_code: vault.join("Code"),
            vault_knowledge: vault.join("Knowledge"),
            vault_generated: vault.join("Generated"),
            vault_provenance: vault.join("Provenance"),
            vault,
            models: root.join("Models"),
            models_chat: root.join("Models").join("Chat"),
            models_coding: root.join("Models").join("Coding"),
            models_reasoning: root.join("Models").join("Reasoning"),
            models_vision: root.join("Models").join("Vision"),
            models_embeddings: root.join("Models").join("Embeddings"),
            models_imported: root.join("Models").join("Imported"),
            models_image_generation: root.join("Models").join("ImageGeneration"),
            image_checkpoints: root
                .join("Models")
                .join("ImageGeneration")
                .join("Checkpoints"),
            image_diffusion_models: root
                .join("Models")
                .join("ImageGeneration")
                .join("DiffusionModels"),
            image_vae: root.join("Models").join("ImageGeneration").join("VAE"),
            image_lora: root.join("Models").join("ImageGeneration").join("LoRA"),
            image_controlnet: root
                .join("Models")
                .join("ImageGeneration")
                .join("ControlNet"),
            image_upscalers: root
                .join("Models")
                .join("ImageGeneration")
                .join("Upscalers"),
            image_embeddings: root
                .join("Models")
                .join("ImageGeneration")
                .join("Embeddings"),
            image_workflows: root
                .join("Models")
                .join("ImageGeneration")
                .join("Workflows"),
            image_imported: root.join("Models").join("ImageGeneration").join("Imported"),
            intake_unassigned: intake.join("_Unassigned"),
            shared: root.join("Shared"),
            artifacts: root.join("Artifacts"),
            backups: root.join("Backups"),
            recovery: root.join("Recovery"),
            recycle_bin: root.join("RecycleBin"),
            exports: root.join("Exports"),
            git_mirrors: git.join("Mirrors"),
            git_recovery: git.join("Recovery"),
            git,
            cortex_search: cortex_data.join("Search"),
            cortex_memory: cortex_data.join("Memory"),
            cortex_tasks: cortex_data.join("Tasks"),
            cortex_logs: cortex_data.join("Logs"),
            cortex_data,
            internal: root.join(".cortex"),
            migration_staging: root.join(".cortex").join("MigrationStaging"),
            intake,
            root,
        }
    }

    fn ensure_directories(&self) -> Result<(), String> {
        for directory in [
            &self.root,
            &self.projects,
            &self.vault,
            &self.vault_assets,
            &self.vault_logic,
            &self.vault_templates,
            &self.vault_code,
            &self.vault_knowledge,
            &self.vault_generated,
            &self.vault_provenance,
            &self.models,
            &self.models_chat,
            &self.models_coding,
            &self.models_reasoning,
            &self.models_vision,
            &self.models_embeddings,
            &self.models_imported,
            &self.models_image_generation,
            &self.image_checkpoints,
            &self.image_diffusion_models,
            &self.image_vae,
            &self.image_lora,
            &self.image_controlnet,
            &self.image_upscalers,
            &self.image_embeddings,
            &self.image_workflows,
            &self.image_imported,
            &self.intake,
            &self.intake_unassigned,
            &self.shared,
            &self.artifacts,
            &self.backups,
            &self.recovery,
            &self.recycle_bin,
            &self.exports,
            &self.git,
            &self.git_mirrors,
            &self.git_recovery,
            &self.cortex_data,
            &self.cortex_search,
            &self.cortex_memory,
            &self.cortex_tasks,
            &self.cortex_logs,
            &self.internal,
            &self.migration_staging,
        ] {
            fs::create_dir_all(directory).map_err(|error| {
                format!(
                    "failed to provision Cortex library directory {}: {error}",
                    directory.display()
                )
            })?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryCandidate {
    pub root: PathBuf,
    pub name: String,
    pub kind: String,
    pub confidence: u8,
    pub markers: Vec<String>,
    pub languages: Vec<String>,
    pub already_registered: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryScanReport {
    pub schema_version: u32,
    pub root: PathBuf,
    pub scanned_directories: u64,
    pub ignored_directories: u64,
    pub candidates: Vec<LibraryCandidate>,
    pub generated_unix_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum CatalogFileClass {
    ProjectSource,
    ProjectContent,
    Documentation,
    ArchiveBackup,
    GeneratedBuild,
    DependencyVendor,
    ToolchainInstalled,
    ModelData,
    Media,
    PersonalNonProject,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectNodeKind {
    Workspace,
    Project,
    Component,
    Client,
    Server,
    Editor,
    Launcher,
    Tool,
    Plugin,
    Example,
    TestFixture,
    ContentPack,
    SharedLibrary,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectGraphNode {
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    pub parent_id: Option<String>,
    pub kind: ProjectNodeKind,
    pub confidence: u8,
    pub markers: Vec<String>,
    pub languages: Vec<String>,
    pub already_registered: bool,
    pub tree_signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectGraphEdgeKind {
    Contains,
    RelatedCopy,
    SharedRepository,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectGraphEdge {
    pub from: String,
    pub to: String,
    pub kind: ProjectGraphEdgeKind,
    pub confidence: u8,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DuplicateFileGroup {
    pub fingerprint: String,
    pub bytes_each: u64,
    pub paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectLineageCandidate {
    pub normalized_name: String,
    pub project_ids: Vec<String>,
    pub confidence: u8,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogClassSummary {
    pub class: CatalogFileClass,
    pub files: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DriveCatalogReport {
    pub schema_version: u32,
    pub scan_root: PathBuf,
    #[serde(default)]
    pub scan_roots: Vec<PathBuf>,
    pub catalog_file: PathBuf,
    pub scanned_directories: u64,
    #[serde(default)]
    pub ignored_directories: u64,
    pub scanned_files: u64,
    pub scanned_bytes: u64,
    pub skipped_cycles_or_external_links: u64,
    pub project_nodes: Vec<ProjectGraphNode>,
    pub project_edges: Vec<ProjectGraphEdge>,
    pub lineage_candidates: Vec<ProjectLineageCandidate>,
    pub duplicate_groups: Vec<DuplicateFileGroup>,
    pub class_summary: Vec<CatalogClassSummary>,
    pub truncated: bool,
    pub cancelled: bool,
    pub generated_unix_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DriveCatalogFileRecord {
    pub path: PathBuf,
    pub relative_path: PathBuf,
    pub bytes: u64,
    pub modified_unix_ms: u128,
    pub class: CatalogFileClass,
    pub project_id: Option<String>,
    pub content_fingerprint: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MigrationActionKind {
    RegisterInPlace,
    CreateManagedProject,
    CopyVerifiedTree,
    RebindWorkspace,
    ProbationOriginal,
    InitializeGit,
    ImportRecoveredVersion,
    MoveToVault,
    MovePath,
    UpdateReference,
    QuarantineOriginal,
    VerifyProject,
    DeleteOriginal,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationAction {
    pub id: String,
    pub kind: MigrationActionKind,
    pub description: String,
    pub source: Option<PathBuf>,
    pub target: Option<PathBuf>,
    pub enabled: bool,
    pub destructive: bool,
    pub requires_approval: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MigrationPlanStatus {
    Draft,
    Approved,
    Applying,
    PromotedPendingValidation,
    Certified,
    RolledBack,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationApplyResult {
    pub schema_version: u32,
    pub plan_id: String,
    pub source: PathBuf,
    pub target: PathBuf,
    pub workspace_id: String,
    pub copied_files: u64,
    pub copied_bytes: u64,
    pub verified_files: u64,
    pub source_preserved: bool,
    pub active_workspace_preserved: bool,
    pub status: MigrationPlanStatus,
    pub applied_unix_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationPlan {
    pub schema_version: u32,
    pub id: String,
    pub title: String,
    pub project_ids: Vec<String>,
    pub status: MigrationPlanStatus,
    pub actions: Vec<MigrationAction>,
    pub estimated_move_bytes: u64,
    pub generated_unix_ms: u128,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntakeKind {
    HandoffPatch,
    SourceRollup,
    DebugBundle,
    Image,
    Document,
    Log,
    Archive,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CortexIntakeItem {
    pub path: PathBuf,
    pub relative_path: PathBuf,
    pub project_id: Option<String>,
    pub kind: IntakeKind,
    pub bytes: u64,
    pub modified_unix_ms: u128,
    pub stable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CortexIntakeReport {
    pub schema_version: u32,
    pub root: PathBuf,
    pub items: Vec<CortexIntakeItem>,
    pub stable_items: usize,
    pub unassigned_items: usize,
    pub generated_unix_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IntakeTag {
    schema_version: u32,
    project_id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct ServicePortAssignments {
    schema_version: u32,
    assignments: BTreeMap<String, u16>,
}

#[derive(Clone, Copy, Debug, Default)]
struct MigrationCopyStats {
    files: u64,
    bytes: u64,
    verified_files: u64,
}

#[derive(Clone, Debug)]
pub struct WorkspaceRegistry {
    home: PathBuf,
    registry_path: PathBuf,
}

impl WorkspaceRegistry {
    pub fn open_default() -> Result<Self, String> {
        Self::open(cortex_home()?)
    }

    pub fn open(home: impl Into<PathBuf>) -> Result<Self, String> {
        let home = home.into();
        let registry_root = home.join("registry");
        fs::create_dir_all(&registry_root).map_err(|error| {
            format!(
                "failed to create Cortex registry directory {}: {error}",
                registry_root.display()
            )
        })?;
        Ok(Self {
            home,
            registry_path: registry_root.join("workspaces.json"),
        })
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn registry_path(&self) -> &Path {
        &self.registry_path
    }

    pub fn ensure_default_library_root(&self) -> Result<Option<CortexLibraryRoot>, String> {
        if let Some(configured) = configured_vault_root_from_environment() {
            self.provision_library_root(&configured)?;
            return self.library_root();
        }

        if let Some(existing) = self.library_root()? {
            let layout = CortexLibraryLayout::from_root(existing.root.clone());
            layout.ensure_directories()?;
            return Ok(Some(existing));
        }

        #[cfg(windows)]
        {
            let drive = PathBuf::from(r"D:\");
            if drive.is_dir() {
                let preferred = drive.join("Cortex");
                self.provision_library_root(&preferred)?;
                return self.library_root();
            }
        }

        Ok(None)
    }

    /// Resolve the storage authority for a concrete workspace before desktop
    /// services, model hosts, or project tools are opened. Explicit machine
    /// configuration wins; portable source distributions can then opt into the
    /// repository drive root. Existing managed state is still protected by
    /// `set_library_root` and will not be silently migrated.
    pub fn ensure_library_root_for_workspace(
        &self,
        workspace_root: impl AsRef<Path>,
    ) -> Result<Option<CortexLibraryRoot>, String> {
        if let Some(configured) = configured_vault_root_from_environment() {
            self.provision_library_root(&configured)?;
            return self.library_root();
        }

        if let Some(portable) = portable_drive_root_for_workspace(workspace_root.as_ref())? {
            self.provision_library_root(&portable)?;
            return self.library_root();
        }

        self.ensure_default_library_root()
    }

    pub fn provision_library_root(
        &self,
        root: impl AsRef<Path>,
    ) -> Result<CortexLibraryLayout, String> {
        fs::create_dir_all(root.as_ref()).map_err(|error| {
            format!(
                "failed to create Cortex library root {}: {error}",
                root.as_ref().display()
            )
        })?;
        let canonical = fs::canonicalize(root.as_ref()).map_err(|error| {
            format!(
                "failed to resolve Cortex library root {}: {error}",
                root.as_ref().display()
            )
        })?;
        let layout = CortexLibraryLayout::from_root(canonical.clone());
        layout.ensure_directories()?;
        self.set_library_root(&canonical)?;
        Ok(layout)
    }

    pub fn library_layout(&self) -> Result<Option<CortexLibraryLayout>, String> {
        let Some(record) = self.library_root()? else {
            return Ok(None);
        };
        let layout = CortexLibraryLayout::from_root(record.root);
        layout.ensure_directories()?;
        Ok(Some(layout))
    }

    pub fn project_intake_root(&self, workspace_id: &str) -> Result<PathBuf, String> {
        if workspace_id.trim().is_empty()
            || workspace_id.contains('/')
            || workspace_id.contains('\\')
            || workspace_id.contains("..")
        {
            return Err("invalid Cortex workspace id for intake routing".into());
        }
        let layout = self
            .library_layout()?
            .ok_or_else(|| "Cortex library root is not configured".to_string())?;
        let directory = layout.intake.join(workspace_id);
        fs::create_dir_all(&directory).map_err(|error| {
            format!(
                "failed to create Cortex project intake directory {}: {error}",
                directory.display()
            )
        })?;
        Ok(directory)
    }

    pub fn library_root(&self) -> Result<Option<CortexLibraryRoot>, String> {
        let path = self.home.join("registry").join("library.json");
        if !path.is_file() {
            return Ok(None);
        }
        let record: CortexLibraryRoot = serde_json::from_slice(
            &fs::read(&path).map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("invalid Cortex library record {}: {error}", path.display()))?;
        if !matches!(record.schema_version, 1 | 2) {
            return Err(format!(
                "unsupported Cortex library schema {}",
                record.schema_version
            ));
        }
        Ok(Some(record))
    }

    pub fn set_library_root(&self, root: impl AsRef<Path>) -> Result<CortexLibraryRoot, String> {
        let canonical = fs::canonicalize(root.as_ref()).map_err(|error| {
            format!(
                "failed to resolve Cortex library root {}: {error}",
                root.as_ref().display()
            )
        })?;
        if !canonical.is_dir() {
            return Err(format!(
                "Cortex library root must be a directory: {}",
                canonical.display()
            ));
        }

        if let Some(existing) = self.library_root()? {
            if existing.root != canonical && storage_authority_has_live_state(&existing.root) {
                return Err(format!(
                    "Cortex storage authority already contains live managed state at {}. Changing the configured root is not a storage migration; use the dedicated authority-migration workflow instead.",
                    existing.root.display()
                ));
            }
        }

        let previous_offsite = self
            .library_root()?
            .and_then(|record| record.offsite_backup_root);
        let record = CortexLibraryRoot {
            schema_version: 2,
            volume: storage_volume_identity_for(&canonical).ok(),
            root: canonical,
            configured_unix_ms: unix_millis(),
            offsite_backup_root: previous_offsite,
        };
        self.save_library_root_record(&record)?;
        Ok(record)
    }

    fn save_library_root_record(&self, record: &CortexLibraryRoot) -> Result<(), String> {
        let path = self.home.join("registry").join("library.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("failed to save Cortex library {}: {error}", path.display()))
    }

    pub fn offsite_backup_root(&self) -> Result<Option<PathBuf>, String> {
        Ok(self
            .library_root()?
            .and_then(|record| record.offsite_backup_root))
    }

    pub fn set_offsite_backup_root(
        &self,
        root: impl AsRef<Path>,
    ) -> Result<CortexLibraryRoot, String> {
        let mut record = self
            .library_root()?
            .ok_or_else(|| "Cortex storage authority is not configured".to_string())?;
        fs::create_dir_all(root.as_ref()).map_err(|error| {
            format!(
                "failed to create Cortex offsite backup root {}: {error}",
                root.as_ref().display()
            )
        })?;
        let canonical = fs::canonicalize(root.as_ref()).map_err(|error| {
            format!(
                "failed to resolve Cortex offsite backup root {}: {error}",
                root.as_ref().display()
            )
        })?;
        if path_starts_with(&canonical, &record.root) || path_starts_with(&record.root, &canonical)
        {
            return Err(
                "Cortex offsite backup root must be outside the live Cortex storage authority"
                    .into(),
            );
        }
        record.schema_version = 2;
        record.offsite_backup_root = Some(canonical);
        if record.volume.is_none() {
            record.volume = storage_volume_identity_for(&record.root).ok();
        }
        self.save_library_root_record(&record)?;
        Ok(record)
    }

    pub fn clear_offsite_backup_root(&self) -> Result<CortexLibraryRoot, String> {
        let mut record = self
            .library_root()?
            .ok_or_else(|| "Cortex storage authority is not configured".to_string())?;
        record.schema_version = 2;
        record.offsite_backup_root = None;
        self.save_library_root_record(&record)?;
        Ok(record)
    }

    pub fn storage_volumes(
        &self,
        include_removable: bool,
    ) -> Result<Vec<CortexStorageVolumeIdentity>, String> {
        available_storage_volumes(include_removable)
    }

    pub fn clear_library_root(&self) -> Result<bool, String> {
        let path = self.home.join("registry").join("library.json");
        if !path.exists() {
            return Ok(false);
        }
        fs::remove_file(&path).map_err(|error| error.to_string())?;
        Ok(true)
    }

    pub fn path_is_in_library(&self, path: impl AsRef<Path>) -> Result<bool, String> {
        let Some(library) = self.library_root()? else {
            return Ok(false);
        };
        let canonical = fs::canonicalize(path.as_ref()).map_err(|error| error.to_string())?;
        Ok(path_starts_with(&canonical, &library.root))
    }

    pub fn internal_git_dir_for(&self, workspace_id: &str) -> Result<PathBuf, String> {
        let layout = self
            .library_layout()?
            .ok_or_else(|| "Cortex Vault root is required for internal Git recovery".to_string())?;
        let git_root = layout.git_recovery.join("projects");
        fs::create_dir_all(&git_root).map_err(|error| error.to_string())?;
        let directory = git_root.join(format!("{workspace_id}.git"));
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        Ok(directory)
    }

    pub fn service_port_for(&self, workspace_id: &str, preferred: u16) -> Result<u16, String> {
        if preferred == 0 {
            return Err("Cortex service port cannot be zero".into());
        }

        let path = self.home.join("registry").join("service_ports.json");
        let mut state = if path.is_file() {
            serde_json::from_slice::<ServicePortAssignments>(
                &fs::read(&path).map_err(|error| error.to_string())?,
            )
            .map_err(|error| format!("invalid Cortex service-port registry: {error}"))?
        } else {
            ServicePortAssignments {
                schema_version: 1,
                assignments: BTreeMap::new(),
            }
        };

        if let Some(port) = state.assignments.get(workspace_id).copied() {
            return Ok(port);
        }

        let used = state.assignments.values().copied().collect::<BTreeSet<_>>();
        let start_offset = (fnv_bytes(workspace_id.as_bytes()) % 192) as u16;
        let mut chosen = None;
        for offset in 0..192_u16 {
            let relative = (start_offset + offset) % 192;
            let Some(candidate) = preferred.checked_add(relative) else {
                continue;
            };
            if !used.contains(&candidate) {
                chosen = Some(candidate);
                break;
            }
        }

        let port = chosen.ok_or_else(|| {
            format!(
                "no free Cortex service port available in {}..{}",
                preferred,
                preferred.saturating_add(191)
            )
        })?;
        state.schema_version = 1;
        state.assignments.insert(workspace_id.to_string(), port);
        fs::write(
            &path,
            serde_json::to_vec_pretty(&state).map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("failed to save Cortex service-port registry: {error}"))?;
        Ok(port)
    }

    pub fn inspect_candidate(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<Option<LibraryCandidate>, String> {
        let canonical = fs::canonicalize(path.as_ref()).map_err(|error| error.to_string())?;
        if !canonical.is_dir() || is_ignored_directory(&canonical) {
            return Ok(None);
        }
        classify_candidate(&canonical, &self.state()?.workspaces)
    }

    pub fn write_intake_tag(
        &self,
        item: impl AsRef<Path>,
        project_id: &str,
    ) -> Result<PathBuf, String> {
        if !self
            .state()?
            .workspaces
            .iter()
            .any(|workspace| workspace.id == project_id)
        {
            return Err(format!("unknown Cortex project id: {project_id}"));
        }
        let sidecar = intake_tag_path(item.as_ref());
        let tag = IntakeTag {
            schema_version: 1,
            project_id: project_id.to_string(),
        };
        fs::write(
            &sidecar,
            serde_json::to_vec_pretty(&tag).map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("failed to write intake tag {}: {error}", sidecar.display()))?;
        Ok(sidecar)
    }

    pub fn scan_intake(
        &self,
        stable_age_ms: u128,
        max_files: usize,
    ) -> Result<CortexIntakeReport, String> {
        let layout = self
            .library_layout()?
            .ok_or_else(|| "Cortex library root is not configured".to_string())?;
        let state = self.state()?;
        let known_ids = state
            .workspaces
            .iter()
            .map(|workspace| workspace.id.clone())
            .collect::<BTreeSet<_>>();
        let now = unix_millis();
        let mut items = Vec::new();
        let mut stack = vec![(layout.intake.clone(), 0_u8)];

        while let Some((directory, depth)) = stack.pop() {
            if items.len() >= max_files.max(1) || depth > 2 {
                continue;
            }
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                if items.len() >= max_files.max(1) {
                    break;
                }
                let path = entry.path();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    stack.push((path, depth + 1));
                    continue;
                }
                if !kind.is_file() || is_intake_tag(&path) {
                    continue;
                }
                let metadata = entry.metadata().map_err(|error| error.to_string())?;
                let modified_unix_ms = metadata
                    .modified()
                    .ok()
                    .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                    .map(|value| value.as_millis())
                    .unwrap_or_default();
                let stable = now.saturating_sub(modified_unix_ms) >= stable_age_ms;
                let relative_path = path
                    .strip_prefix(&layout.intake)
                    .unwrap_or(&path)
                    .to_path_buf();

                let mut project_id = relative_path
                    .components()
                    .next()
                    .and_then(|component| component.as_os_str().to_str())
                    .filter(|value| *value != "_Unassigned")
                    .filter(|value| known_ids.contains(*value))
                    .map(ToOwned::to_owned);

                let sidecar = intake_tag_path(&path);
                if sidecar.is_file() {
                    if let Ok(tag) = serde_json::from_slice::<IntakeTag>(
                        &fs::read(&sidecar).map_err(|error| error.to_string())?,
                    ) {
                        if tag.schema_version == 1 && known_ids.contains(&tag.project_id) {
                            project_id = Some(tag.project_id);
                        }
                    }
                }

                items.push(CortexIntakeItem {
                    path,
                    relative_path,
                    project_id,
                    kind: classify_intake_kind(entry.file_name().to_string_lossy().as_ref()),
                    bytes: metadata.len(),
                    modified_unix_ms,
                    stable,
                });
            }
        }

        items.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let stable_items = items.iter().filter(|item| item.stable).count();
        let unassigned_items = items
            .iter()
            .filter(|item| item.project_id.is_none())
            .count();
        Ok(CortexIntakeReport {
            schema_version: 1,
            root: layout.intake,
            items,
            stable_items,
            unassigned_items,
            generated_unix_ms: now,
        })
    }

    #[cfg(windows)]
    pub fn create_desktop_intake_shortcut(&self) -> Result<PathBuf, String> {
        use std::process::Command;

        let layout = self
            .library_layout()?
            .ok_or_else(|| "Cortex library root is not configured".to_string())?;
        let user_profile = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .ok_or_else(|| "USERPROFILE is unavailable".to_string())?;
        let shortcut = user_profile.join("Desktop").join("Cortex Intake.lnk");
        let script = r#"$shell=New-Object -ComObject WScript.Shell; $s=$shell.CreateShortcut($args[0]); $s.TargetPath=$args[1]; $s.WorkingDirectory=$args[1]; $s.Description='Cortex Intake'; $s.Save()"#;
        let status = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .arg(shortcut.as_os_str())
            .arg(layout.intake.as_os_str())
            .status()
            .map_err(|error| format!("failed to launch Windows shortcut creation: {error}"))?;
        if !status.success() || !shortcut.is_file() {
            return Err("Windows failed to create the Cortex Intake shortcut".into());
        }
        Ok(shortcut)
    }

    pub fn scan_library(&self, max_directories: usize) -> Result<LibraryScanReport, String> {
        let library = self
            .library_root()?
            .ok_or_else(|| "Cortex library root is not configured".to_string())?;
        let registered = self.state()?.workspaces;
        let mut candidates = Vec::new();
        let mut scanned = 0_u64;
        let mut ignored = 0_u64;
        let mut stack = fs::read_dir(&library.root)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .file_type()
                    .ok()
                    .filter(|kind| kind.is_dir())
                    .map(|_| entry.path())
            })
            .collect::<Vec<_>>();
        stack.sort();
        stack.reverse();

        while let Some(directory) = stack.pop() {
            if scanned as usize >= max_directories.max(1) {
                break;
            }
            if is_ignored_directory(&directory) {
                ignored += 1;
                continue;
            }
            scanned += 1;

            if let Some(candidate) = classify_candidate(&directory, &registered)? {
                let strong_boundary = candidate.confidence >= 80;
                candidates.push(candidate);
                if strong_boundary {
                    continue;
                }
            }

            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            let mut children = entries
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    entry
                        .file_type()
                        .ok()
                        .filter(|kind| kind.is_dir())
                        .map(|_| entry.path())
                })
                .collect::<Vec<_>>();
            children.sort();
            children.reverse();
            stack.extend(children);
        }

        candidates.sort_by(|left, right| {
            right
                .confidence
                .cmp(&left.confidence)
                .then_with(|| left.root.cmp(&right.root))
        });
        let report = LibraryScanReport {
            schema_version: 1,
            root: library.root,
            scanned_directories: scanned,
            ignored_directories: ignored,
            candidates,
            generated_unix_ms: unix_millis(),
        };
        let catalog_root = self.home.join("catalog");
        fs::create_dir_all(&catalog_root).map_err(|error| error.to_string())?;
        fs::write(
            catalog_root.join("library_scan.json"),
            serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        Ok(report)
    }

    /// Read-only full storage catalog. This intentionally performs no moves,
    /// deletions, Git initialization, deduplication, or project registration.
    /// Every discovered file is streamed to JSONL so large drives do not need
    /// to keep the whole catalog resident in memory.
    pub fn scan_drive_catalog(
        &self,
        scan_root: impl AsRef<Path>,
        max_directories: usize,
        max_files: usize,
    ) -> Result<DriveCatalogReport, String> {
        self.scan_drive_catalog_with_cancel(scan_root, max_directories, max_files, || false)
    }

    pub fn scan_drive_catalog_with_cancel<F>(
        &self,
        scan_root: impl AsRef<Path>,
        max_directories: usize,
        max_files: usize,
        mut should_cancel: F,
    ) -> Result<DriveCatalogReport, String>
    where
        F: FnMut() -> bool,
    {
        let scan_root = fs::canonicalize(scan_root.as_ref()).map_err(|error| {
            format!(
                "failed to resolve drive catalog root {}: {error}",
                scan_root.as_ref().display()
            )
        })?;
        if !scan_root.is_dir() {
            return Err(format!(
                "drive catalog root is not a directory: {}",
                scan_root.display()
            ));
        }

        let catalog_root = self.home.join("catalog");
        fs::create_dir_all(&catalog_root).map_err(|error| error.to_string())?;
        let catalog_file = catalog_root.join("drive_catalog_files.jsonl");
        let file = fs::File::create(&catalog_file)
            .map_err(|error| format!("failed to create {}: {error}", catalog_file.display()))?;
        let mut writer = BufWriter::new(file);
        let registered = self.state()?.workspaces;
        let mut stack = vec![(scan_root.clone(), None::<String>)];
        let mut visited = BTreeSet::<PathBuf>::new();
        let mut nodes = Vec::<ProjectGraphNode>::new();
        let mut edges = Vec::<ProjectGraphEdge>::new();
        let mut directory_count = 0_u64;
        let mut ignored_directories = 0_u64;
        let mut file_count = 0_u64;
        let mut scanned_bytes = 0_u64;
        let mut skipped_links = 0_u64;
        let mut class_counts = BTreeMap::<CatalogFileClass, (u64, u64)>::new();
        let mut duplicate_map = BTreeMap::<String, (u64, Vec<PathBuf>)>::new();
        let mut truncated = false;
        let mut cancelled = false;

        while let Some((directory, inherited_project)) = stack.pop() {
            if should_cancel() {
                cancelled = true;
                break;
            }
            if directory_count as usize >= max_directories.max(1)
                || file_count as usize >= max_files.max(1)
            {
                truncated = true;
                break;
            }
            let Ok(canonical_directory) = fs::canonicalize(&directory) else {
                continue;
            };
            if !path_starts_with(&canonical_directory, &scan_root)
                || !visited.insert(canonical_directory.clone())
            {
                skipped_links += 1;
                continue;
            }
            directory_count += 1;

            let mut active_project = inherited_project;
            if let Some(candidate) = classify_candidate(&canonical_directory, &registered)? {
                let node_id = project_node_id(&canonical_directory);
                let kind = infer_project_node_kind(
                    &canonical_directory,
                    active_project.as_deref(),
                    &candidate.markers,
                );
                let signature = project_tree_signature(&canonical_directory, 768);
                if let Some(parent) = active_project.clone() {
                    edges.push(ProjectGraphEdge {
                        from: parent.clone(),
                        to: node_id.clone(),
                        kind: ProjectGraphEdgeKind::Contains,
                        confidence: 100,
                        reason: "nested project/component boundary".into(),
                    });
                }
                nodes.push(ProjectGraphNode {
                    id: node_id.clone(),
                    name: candidate.name,
                    root: candidate.root,
                    parent_id: active_project.clone(),
                    kind,
                    confidence: candidate.confidence,
                    markers: candidate.markers,
                    languages: candidate.languages,
                    already_registered: candidate.already_registered,
                    tree_signature: signature,
                });
                active_project = Some(node_id);
            }

            let Ok(entries) = fs::read_dir(&canonical_directory) else {
                continue;
            };
            let mut child_directories = Vec::new();
            for entry in entries.filter_map(Result::ok) {
                if should_cancel() {
                    cancelled = true;
                    break;
                }
                if file_count as usize >= max_files.max(1) {
                    truncated = true;
                    break;
                }
                let path = entry.path();
                if path == catalog_file || path == catalog_root.join("drive_catalog.json") {
                    continue;
                }
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_symlink() {
                    skipped_links += 1;
                    continue;
                }
                if file_type.is_dir() {
                    if is_ignored_directory(&path) {
                        ignored_directories = ignored_directories.saturating_add(1);
                    } else {
                        child_directories.push(path);
                    }
                    continue;
                }
                if !file_type.is_file() {
                    continue;
                }
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                file_count += 1;
                scanned_bytes = scanned_bytes.saturating_add(metadata.len());
                let class = classify_catalog_file(&path, active_project.is_some());
                let counter = class_counts.entry(class.clone()).or_default();
                counter.0 += 1;
                counter.1 = counter.1.saturating_add(metadata.len());
                let modified_unix_ms = metadata
                    .modified()
                    .ok()
                    .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                    .map(|value| value.as_millis())
                    .unwrap_or_default();
                let content_fingerprint = catalog_content_fingerprint(&path, metadata.len());
                if let Some(fingerprint) = &content_fingerprint {
                    let group = duplicate_map
                        .entry(fingerprint.clone())
                        .or_insert_with(|| (metadata.len(), Vec::new()));
                    if group.1.len() < 64 {
                        group.1.push(path.clone());
                    }
                }
                let record = DriveCatalogFileRecord {
                    relative_path: path.strip_prefix(&scan_root).unwrap_or(&path).to_path_buf(),
                    path,
                    bytes: metadata.len(),
                    modified_unix_ms,
                    class,
                    project_id: active_project.clone(),
                    content_fingerprint,
                };
                serde_json::to_writer(&mut writer, &record).map_err(|error| error.to_string())?;
                writer.write_all(b"\n").map_err(|error| error.to_string())?;
            }
            child_directories.sort();
            child_directories.reverse();
            for child in child_directories {
                stack.push((child, active_project.clone()));
            }
        }
        writer.flush().map_err(|error| error.to_string())?;

        add_related_project_edges(&nodes, &mut edges);
        let lineage_candidates = build_lineage_candidates(&nodes);
        let mut duplicate_groups = duplicate_map
            .into_iter()
            .filter_map(|(fingerprint, (bytes_each, paths))| {
                (paths.len() > 1).then_some(DuplicateFileGroup {
                    fingerprint,
                    bytes_each,
                    paths,
                })
            })
            .collect::<Vec<_>>();
        duplicate_groups.sort_by_key(|group| {
            Reverse(group.bytes_each.saturating_mul(group.paths.len() as u64))
        });
        duplicate_groups.truncate(512);

        let class_summary = class_counts
            .into_iter()
            .map(|(class, (files, bytes))| CatalogClassSummary {
                class,
                files,
                bytes,
            })
            .collect::<Vec<_>>();
        let report = DriveCatalogReport {
            schema_version: 2,
            scan_roots: vec![scan_root.clone()],
            scan_root,
            catalog_file,
            scanned_directories: directory_count,
            ignored_directories,
            scanned_files: file_count,
            scanned_bytes,
            skipped_cycles_or_external_links: skipped_links,
            project_nodes: nodes,
            project_edges: edges,
            lineage_candidates,
            duplicate_groups,
            class_summary,
            truncated,
            cancelled,
            generated_unix_ms: unix_millis(),
        };
        fs::write(
            catalog_root.join("drive_catalog.json"),
            serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        Ok(report)
    }

    pub fn scan_machine_catalog(
        &self,
        include_removable: bool,
        max_directories: usize,
        max_files: usize,
    ) -> Result<DriveCatalogReport, String> {
        self.scan_machine_catalog_with_cancel(
            include_removable,
            max_directories,
            max_files,
            &mut || false,
        )
    }

    pub fn scan_machine_catalog_with_cancel(
        &self,
        include_removable: bool,
        max_directories: usize,
        max_files: usize,
        should_cancel: &mut dyn FnMut() -> bool,
    ) -> Result<DriveCatalogReport, String> {
        let roots = available_storage_volumes(include_removable)?
            .into_iter()
            .filter_map(|volume| volume.mount_root)
            .filter_map(|root| fs::canonicalize(root).ok())
            .filter(|root| root.is_dir())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if roots.is_empty() {
            return Err("no eligible local storage volumes were found for whole-PC scan".into());
        }

        let catalog_root = self.home.join("catalog");
        fs::create_dir_all(&catalog_root).map_err(|error| error.to_string())?;
        let combined_file = catalog_root.join("machine_catalog_files.jsonl");
        let mut combined_writer =
            BufWriter::new(fs::File::create(&combined_file).map_err(|error| {
                format!("failed to create {}: {error}", combined_file.display())
            })?);

        let mut directory_count = 0_u64;
        let mut ignored_directories = 0_u64;
        let mut file_count = 0_u64;
        let mut scanned_bytes = 0_u64;
        let mut skipped_links = 0_u64;
        let mut nodes = Vec::<ProjectGraphNode>::new();
        let mut edges = Vec::<ProjectGraphEdge>::new();
        let mut class_counts = BTreeMap::<CatalogFileClass, (u64, u64)>::new();
        let mut duplicate_map = BTreeMap::<String, (u64, Vec<PathBuf>)>::new();
        let mut truncated = false;
        let mut cancelled = false;

        for root in &roots {
            if should_cancel() {
                cancelled = true;
                break;
            }
            let remaining_directories = max_directories
                .max(1)
                .saturating_sub(directory_count as usize);
            let remaining_files = max_files.max(1).saturating_sub(file_count as usize);
            if remaining_directories == 0 || remaining_files == 0 {
                truncated = true;
                break;
            }
            let report = self.scan_drive_catalog_with_cancel(
                root,
                remaining_directories,
                remaining_files,
                &mut *should_cancel,
            )?;
            directory_count = directory_count.saturating_add(report.scanned_directories);
            ignored_directories = ignored_directories.saturating_add(report.ignored_directories);
            file_count = file_count.saturating_add(report.scanned_files);
            scanned_bytes = scanned_bytes.saturating_add(report.scanned_bytes);
            skipped_links = skipped_links.saturating_add(report.skipped_cycles_or_external_links);
            nodes.extend(report.project_nodes);
            edges.extend(
                report
                    .project_edges
                    .into_iter()
                    .filter(|edge| edge.kind == ProjectGraphEdgeKind::Contains),
            );
            for summary in report.class_summary {
                let counter = class_counts.entry(summary.class).or_default();
                counter.0 = counter.0.saturating_add(summary.files);
                counter.1 = counter.1.saturating_add(summary.bytes);
            }

            let file = fs::File::open(&report.catalog_file).map_err(|error| {
                format!("failed to read {}: {error}", report.catalog_file.display())
            })?;
            for line in BufReader::new(file).lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                combined_writer
                    .write_all(line.as_bytes())
                    .and_then(|_| combined_writer.write_all(b"\n"))
                    .map_err(|error| error.to_string())?;
                if let Ok(record) = serde_json::from_str::<DriveCatalogFileRecord>(&line) {
                    if let Some(fingerprint) = record.content_fingerprint {
                        let group = duplicate_map
                            .entry(fingerprint)
                            .or_insert_with(|| (record.bytes, Vec::new()));
                        if group.1.len() < 64 {
                            group.1.push(record.path);
                        }
                    }
                }
            }
            truncated |= report.truncated;
            cancelled |= report.cancelled;
            if truncated || cancelled {
                break;
            }
        }
        combined_writer.flush().map_err(|error| error.to_string())?;

        add_related_project_edges(&nodes, &mut edges);
        let lineage_candidates = build_lineage_candidates(&nodes);
        let mut duplicate_groups = duplicate_map
            .into_iter()
            .filter_map(|(fingerprint, (bytes_each, paths))| {
                (paths.len() > 1).then_some(DuplicateFileGroup {
                    fingerprint,
                    bytes_each,
                    paths,
                })
            })
            .collect::<Vec<_>>();
        duplicate_groups.sort_by_key(|group| {
            Reverse(group.bytes_each.saturating_mul(group.paths.len() as u64))
        });
        duplicate_groups.truncate(512);

        let class_summary = class_counts
            .into_iter()
            .map(|(class, (files, bytes))| CatalogClassSummary {
                class,
                files,
                bytes,
            })
            .collect::<Vec<_>>();
        let scan_root = roots.first().cloned().unwrap_or_default();
        let report = DriveCatalogReport {
            schema_version: 2,
            scan_root,
            scan_roots: roots,
            catalog_file: combined_file,
            scanned_directories: directory_count,
            ignored_directories,
            scanned_files: file_count,
            scanned_bytes,
            skipped_cycles_or_external_links: skipped_links,
            project_nodes: nodes,
            project_edges: edges,
            lineage_candidates,
            duplicate_groups,
            class_summary,
            truncated,
            cancelled,
            generated_unix_ms: unix_millis(),
        };
        fs::write(
            catalog_root.join("drive_catalog.json"),
            serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        Ok(report)
    }

    pub fn scan_configured_storage(
        &self,
        max_directories: usize,
        max_files: usize,
    ) -> Result<DriveCatalogReport, String> {
        let library = self
            .library_root()?
            .ok_or_else(|| "Cortex library root is not configured".to_string())?;
        let root = volume_root_for(&library.root);
        self.scan_drive_catalog(root, max_directories, max_files)
    }

    pub fn scan_configured_storage_with_cancel(
        &self,
        max_directories: usize,
        max_files: usize,
        should_cancel: &mut dyn FnMut() -> bool,
    ) -> Result<DriveCatalogReport, String> {
        let library = self
            .library_root()?
            .ok_or_else(|| "Cortex library root is not configured".to_string())?;
        let root = volume_root_for(&library.root);
        self.scan_drive_catalog_with_cancel(root, max_directories, max_files, should_cancel)
    }

    pub fn last_drive_catalog(&self) -> Result<Option<DriveCatalogReport>, String> {
        let path = self.home.join("catalog").join("drive_catalog.json");
        if !path.is_file() {
            return Ok(None);
        }
        serde_json::from_slice(&fs::read(&path).map_err(|error| error.to_string())?)
            .map(Some)
            .map_err(|error| format!("invalid drive catalog {}: {error}", path.display()))
    }

    /// Creates a dry-run migration proposal only. Applying, moving, deleting,
    /// Git initialization, and deduplication remain separate approval-gated work.
    /// Creates a dry-run migration proposal only. Applying is a separate,
    /// approval-gated copy/verify/promote operation and never deletes the source.
    pub fn propose_managed_migration(&self, project_id: &str) -> Result<MigrationPlan, String> {
        let report = self
            .last_drive_catalog()?
            .ok_or_else(|| "scan storage before proposing a migration".to_string())?;
        let node = report
            .project_nodes
            .iter()
            .find(|node| node.id == project_id)
            .ok_or_else(|| format!("catalog project not found: {project_id}"))?;
        let layout = self
            .library_layout()?
            .ok_or_else(|| "Cortex storage authority is not configured".to_string())?;
        let safe_name = safe_managed_name(&node.name);
        let target = layout.projects.join(&safe_name);
        let stamp = unix_millis();
        let plan_id = format!(
            "migration-{stamp}-{:016x}",
            fnv_bytes(project_id.as_bytes())
        );
        let estimated_move_bytes = tree_size_without_links(&node.root)?;
        let actions = vec![
            MigrationAction {
                id: "catalog-identity".into(),
                kind: MigrationActionKind::RegisterInPlace,
                description: "Preserve the current catalog/project identity before migration. Registration is metadata only and does not move source files.".into(),
                source: Some(node.root.clone()),
                target: None,
                enabled: true,
                destructive: false,
                requires_approval: false,
            },
            MigrationAction {
                id: "verified-copy".into(),
                kind: MigrationActionKind::CopyVerifiedTree,
                description: "Copy the complete project tree into destination-side MigrationStaging, reject symlinks, and verify every regular file byte-for-byte before promotion.".into(),
                source: Some(node.root.clone()),
                target: Some(target.clone()),
                enabled: true,
                destructive: false,
                requires_approval: true,
            },
            MigrationAction {
                id: "promote".into(),
                kind: MigrationActionKind::CreateManagedProject,
                description: "Promote the verified staging tree into the Cortex-managed Projects root. Existing destinations are never merged or overwritten.".into(),
                source: Some(node.root.clone()),
                target: Some(target.clone()),
                enabled: true,
                destructive: false,
                requires_approval: true,
            },
            MigrationAction {
                id: "rebind".into(),
                kind: MigrationActionKind::RebindWorkspace,
                description: "Rebind Cortex project identity to the promoted path while preserving historical-path metadata, per-project state, repository history, Forgejo/Vault associations, and the previously active workspace.".into(),
                source: Some(node.root.clone()),
                target: Some(target.clone()),
                enabled: true,
                destructive: false,
                requires_approval: true,
            },
            MigrationAction {
                id: "validate".into(),
                kind: MigrationActionKind::VerifyProject,
                description: "The promoted project remains pending validation until its detected project quality gate succeeds from the new location.".into(),
                source: Some(target.clone()),
                target: None,
                enabled: true,
                destructive: false,
                requires_approval: true,
            },
            MigrationAction {
                id: "probation-original".into(),
                kind: MigrationActionKind::ProbationOriginal,
                description: "Keep the original tree untouched during probation. A later explicit cleanup may move it to Cortex Recycle Bin only after the promoted project is validated.".into(),
                source: Some(node.root.clone()),
                target: Some(layout.recycle_bin.join("Projects").join(&plan_id)),
                enabled: false,
                destructive: false,
                requires_approval: true,
            },
        ];
        let plan = MigrationPlan {
            schema_version: 2,
            id: plan_id.clone(),
            title: format!("Migrate {} into Cortex-managed storage", node.name),
            project_ids: vec![project_id.to_string()],
            status: MigrationPlanStatus::Draft,
            actions,
            estimated_move_bytes,
            generated_unix_ms: stamp,
            note: "DRY RUN ONLY: no file has been moved, deleted, deduplicated, reinitialized, or rebound. Existing repository history will be copied intact; Forgejo remains the user-facing repository authority.".into(),
        };
        self.save_migration_plan(&plan)?;
        Ok(plan)
    }

    fn save_migration_plan(&self, plan: &MigrationPlan) -> Result<(), String> {
        let plans = self.home.join("catalog").join("migration_plans");
        fs::create_dir_all(&plans).map_err(|error| error.to_string())?;
        fs::write(
            plans.join(format!("{}.json", plan.id)),
            serde_json::to_vec_pretty(plan).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())
    }

    pub fn migration_plan(&self, id: &str) -> Result<MigrationPlan, String> {
        let path = self
            .home
            .join("catalog")
            .join("migration_plans")
            .join(format!("{id}.json"));
        if !path.is_file() {
            return Err(format!("Cortex migration plan not found: {id}"));
        }
        serde_json::from_slice(&fs::read(&path).map_err(|error| error.to_string())?)
            .map_err(|error| format!("invalid Cortex migration plan {}: {error}", path.display()))
    }

    pub fn apply_managed_migration(&self, plan_id: &str) -> Result<MigrationApplyResult, String> {
        let mut plan = self.migration_plan(plan_id)?;
        if !matches!(
            plan.status,
            MigrationPlanStatus::Draft | MigrationPlanStatus::Approved
        ) {
            return Err(format!(
                "migration plan {} cannot be applied from status {:?}",
                plan.id, plan.status
            ));
        }
        let copy_action = plan
            .actions
            .iter()
            .find(|action| action.kind == MigrationActionKind::CopyVerifiedTree)
            .ok_or_else(|| "migration plan is missing verified-copy action".to_string())?;
        let source = copy_action
            .source
            .clone()
            .ok_or_else(|| "migration plan is missing source path".to_string())?;
        let target = copy_action
            .target
            .clone()
            .ok_or_else(|| "migration plan is missing target path".to_string())?;
        let source = fs::canonicalize(&source).map_err(|error| {
            format!(
                "failed to resolve migration source {}: {error}",
                source.display()
            )
        })?;
        if !source.is_dir() {
            return Err(format!(
                "migration source is not a directory: {}",
                source.display()
            ));
        }
        if self
            .active()?
            .is_some_and(|workspace| workspace.root == source)
        {
            return Err(
                "Cortex will not migrate the currently active Desktop workspace. Switch to another project first."
                    .into(),
            );
        }
        if target.exists() {
            return Err(format!(
                "migration destination already exists; Cortex will not merge or overwrite it: {}",
                target.display()
            ));
        }
        if path_starts_with(&target, &source) || path_starts_with(&source, &target) {
            return Err("migration source and destination overlap".into());
        }

        let layout = self
            .library_layout()?
            .ok_or_else(|| "Cortex storage authority is not configured".to_string())?;
        if !path_starts_with(&target, &layout.projects) {
            return Err(format!(
                "migration destination is outside the managed Projects root: {}",
                target.display()
            ));
        }
        let staging_root = layout.migration_staging.join(&plan.id);
        if staging_root.exists() {
            return Err(format!(
                "migration staging already exists and requires review before retry: {}",
                staging_root.display()
            ));
        }
        fs::create_dir_all(&staging_root).map_err(|error| error.to_string())?;
        let staged_project = staging_root.join(
            target
                .file_name()
                .ok_or_else(|| "migration destination has no project name".to_string())?,
        );

        plan.status = MigrationPlanStatus::Applying;
        self.save_migration_plan(&plan)?;

        let previous_active = self.active()?.map(|workspace| workspace.id);
        let copy = match copy_tree_verified(&source, &staged_project) {
            Ok(copy) => copy,
            Err(error) => {
                plan.status = MigrationPlanStatus::Failed;
                plan.note = format!(
                    "Migration staging failed. The original source remains authoritative and untouched. Error: {error}"
                );
                let _ = self.save_migration_plan(&plan);
                return Err(error);
            }
        };

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::rename(&staged_project, &target).map_err(|error| {
            format!(
                "verified migration staging could not be promoted to {}: {error}",
                target.display()
            )
        })?;

        let rebind_result = if let Some(existing) = self.resolve_for_root(&source)? {
            self.rebind_workspace_root(&existing.id, &target)
        } else {
            Workspace::open(&target)
                .map_err(|error| error.to_string())
                .and_then(|workspace| {
                    self.attach_preserving_active(&workspace, previous_active.as_deref())
                })
        };
        let record = match rebind_result {
            Ok(record) => record,
            Err(error) => {
                let rollback = fs::rename(&target, &staged_project);
                plan.status = MigrationPlanStatus::Failed;
                plan.note = if rollback.is_ok() {
                    format!(
                        "Verified files were promoted but Cortex project identity rebinding failed. The destination was returned to MigrationStaging and the original source remains authoritative. Error: {error}"
                    )
                } else {
                    format!(
                        "Verified files were promoted but Cortex project identity rebinding failed, and the promoted destination could not be returned to MigrationStaging. The original source remains authoritative; review both locations before retrying. Error: {error}"
                    )
                };
                let _ = self.save_migration_plan(&plan);
                return Err(plan.note.clone());
            }
        };
        let _ = fs::remove_dir_all(&staging_root);

        if let Some(previous) = previous_active.as_deref() {
            if previous != record.id {
                let _ = self.set_active(previous)?;
            }
        } else {
            self.restore_active_workspace(None)?;
        }

        plan.status = MigrationPlanStatus::PromotedPendingValidation;
        plan.note = format!(
            "Verified copy promoted to {}. Original source remains untouched at {}. Project identity is rebound to the promoted path, but validation is still required before cleanup/probation can advance.",
            target.display(),
            source.display()
        );
        self.save_migration_plan(&plan)?;

        Ok(MigrationApplyResult {
            schema_version: 1,
            plan_id: plan.id,
            source,
            target,
            workspace_id: record.id,
            copied_files: copy.files,
            copied_bytes: copy.bytes,
            verified_files: copy.verified_files,
            source_preserved: true,
            active_workspace_preserved: self.active()?.map(|item| item.id) == previous_active,
            status: MigrationPlanStatus::PromotedPendingValidation,
            applied_unix_ms: unix_millis(),
        })
    }

    pub fn list_migration_plans(&self) -> Result<Vec<MigrationPlan>, String> {
        let root = self.home.join("catalog").join("migration_plans");
        if !root.is_dir() {
            return Ok(Vec::new());
        }
        let mut plans = fs::read_dir(root)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().and_then(|v| v.to_str()) == Some("json"))
            .filter_map(|entry| fs::read(entry.path()).ok())
            .filter_map(|bytes| serde_json::from_slice::<MigrationPlan>(&bytes).ok())
            .collect::<Vec<_>>();
        plans.sort_by_key(|plan| Reverse(plan.generated_unix_ms));
        Ok(plans)
    }

    pub fn last_library_scan(&self) -> Result<Option<LibraryScanReport>, String> {
        let path = self.home.join("catalog").join("library_scan.json");
        if !path.is_file() {
            return Ok(None);
        }
        serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    /// Register Cortex itself before arbitrary managed projects so the native
    /// GUI/CLI always knows its own system project.  This is best-effort for
    /// installed builds that do not carry a source checkout beside the binary.
    pub fn ensure_cortex_self_registered(&self) -> Result<Option<RegisteredWorkspace>, String> {
        let Some(root) = discover_cortex_project_root() else {
            return Ok(None);
        };
        let previous_active = self.state()?.active_workspace_id;
        let workspace = Workspace::open(&root).map_err(|error| error.to_string())?;
        let record = self.attach_preserving_active(&workspace, previous_active.as_deref())?;
        Ok(Some(record))
    }

    pub fn ensure_cortex_root_registered(
        &self,
        root: impl AsRef<Path>,
    ) -> Result<RegisteredWorkspace, String> {
        let contract = ProjectContract::load_required(root.as_ref())?;
        if !contract.project.id.is_cortex() {
            return Err(format!(
                "Cortex self-registration requires project id `cortex`, found `{}`",
                contract.project.id
            ));
        }
        let workspace = Workspace::open(root.as_ref()).map_err(|error| error.to_string())?;
        self.attach(&workspace)
    }

    pub fn attach_candidate(&self, path: impl AsRef<Path>) -> Result<RegisteredWorkspace, String> {
        let workspace = Workspace::open(path.as_ref()).map_err(|error| error.to_string())?;
        self.attach(&workspace)
    }

    pub fn state(&self) -> Result<WorkspaceRegistryState, String> {
        if !self.registry_path.is_file() {
            return Ok(WorkspaceRegistryState {
                schema_version: REGISTRY_SCHEMA_VERSION,
                ..WorkspaceRegistryState::default()
            });
        }
        let bytes = fs::read(&self.registry_path)
            .map_err(|error| format!("failed to read {}: {error}", self.registry_path.display()))?;
        let mut state: WorkspaceRegistryState =
            serde_json::from_slice(&bytes).map_err(|error| {
                format!(
                    "invalid Cortex workspace registry {}: {error}",
                    self.registry_path.display()
                )
            })?;
        if matches!(state.schema_version, 1 | 2) {
            for workspace in &mut state.workspaces {
                if workspace.known_paths.is_empty() {
                    workspace.known_paths.push(workspace.root.clone());
                }
                if workspace.fingerprint.is_empty() {
                    workspace.fingerprint = legacy_record_fingerprint(workspace);
                }
                if workspace.state_root.as_os_str().is_empty() {
                    workspace.state_root = self.home.join("workspaces").join(&workspace.id);
                }
                if let Ok(Some(contract)) = ProjectContract::load_optional(&workspace.root) {
                    workspace.project_id = Some(contract.project.id.clone());
                    workspace.authority = contract.project.authority;
                    workspace.capabilities = contract.capabilities();
                    workspace.contract_path = Some(workspace.root.join("project.control.json"));
                    workspace.name = contract.project.name.clone();
                }
            }
            state.schema_version = REGISTRY_SCHEMA_VERSION;
            self.save(&state)?;
        } else if state.schema_version != REGISTRY_SCHEMA_VERSION {
            return Err(format!(
                "unsupported Cortex workspace registry schema {} (expected {})",
                state.schema_version, REGISTRY_SCHEMA_VERSION
            ));
        }

        #[cfg(windows)]
        if rebase_portable_workspace_paths(&mut state, &self.home) {
            self.save(&state)?;
        }

        state
            .workspaces
            .sort_by_key(|workspace| Reverse(workspace.last_opened_unix_ms));
        Ok(state)
    }

    pub fn list(&self) -> Result<Vec<RegisteredWorkspace>, String> {
        let _ = self.prune_generated_workspace_records()?;
        Ok(self.state()?.workspaces)
    }

    pub fn prune_generated_workspace_records(&self) -> Result<usize, String> {
        let mut state = self.state()?;
        let before = state.workspaces.len();
        state
            .workspaces
            .retain(|workspace| !is_generated_workspace_root(&workspace.root));
        let removed = before.saturating_sub(state.workspaces.len());
        if removed == 0 {
            return Ok(0);
        }

        if state.active_workspace_id.as_deref().is_some_and(|active| {
            !state
                .workspaces
                .iter()
                .any(|workspace| workspace.id == active)
        }) {
            state.active_workspace_id = state
                .workspaces
                .first()
                .map(|workspace| workspace.id.clone());
        }
        self.save(&state)?;
        Ok(removed)
    }

    pub fn active(&self) -> Result<Option<RegisteredWorkspace>, String> {
        let state = self.state()?;
        let Some(active_id) = state.active_workspace_id else {
            return Ok(None);
        };
        Ok(state
            .workspaces
            .into_iter()
            .find(|workspace| workspace.id == active_id))
    }

    pub fn attach(&self, workspace: &Workspace) -> Result<RegisteredWorkspace, String> {
        let mut state = self.state()?;
        let now = unix_millis();
        let fingerprint = workspace_fingerprint(workspace);
        let canonical_root = workspace.root().to_path_buf();
        let contract = ProjectContract::load_optional(&canonical_root)?;
        let project_id = contract.as_ref().map(|value| value.project.id.clone());
        let authority = contract
            .as_ref()
            .map(|value| value.project.authority)
            .unwrap_or(ProjectAuthority::Unknown);
        let capabilities = contract
            .as_ref()
            .map(ProjectContract::capabilities)
            .unwrap_or_default();
        let contract_path = contract
            .as_ref()
            .map(|_| canonical_root.join("project.control.json"));
        let display_name = contract
            .as_ref()
            .map(|value| value.project.name.clone())
            .unwrap_or_else(|| workspace.profile().name.clone());

        let match_index = state.workspaces.iter().position(|entry| {
            if entry.root == canonical_root {
                return true;
            }

            // A known historical path is only reusable when the record's current
            // root no longer exists. If both folders exist, they are distinct live
            // projects and must receive distinct WorkspaceIds/state roots.
            !entry.root.exists() && entry.known_paths.iter().any(|path| path == &canonical_root)
        });

        let record = if let Some(index) = match_index {
            let existing = &mut state.workspaces[index];
            existing.name = display_name.clone();
            existing.project_id = project_id.clone();
            existing.authority = authority;
            existing.capabilities = capabilities.clone();
            existing.contract_path = contract_path.clone();
            if existing.root != canonical_root
                && !existing
                    .known_paths
                    .iter()
                    .any(|path| path == &existing.root)
            {
                existing.known_paths.push(existing.root.clone());
            }
            if !existing
                .known_paths
                .iter()
                .any(|path| path == &canonical_root)
            {
                existing.known_paths.push(canonical_root.clone());
            }
            existing.root = canonical_root;
            existing.profile = workspace.profile().clone();
            existing.fingerprint = fingerprint;
            if existing.state_root.as_os_str().is_empty() {
                existing.state_root = self.home.join("workspaces").join(&existing.id);
            }
            existing.last_opened_unix_ms = now;
            existing.clone()
        } else {
            let id = new_workspace_id(&fingerprint);
            let state_root = self.home.join("workspaces").join(&id);
            let record = RegisteredWorkspace {
                id: id.clone(),
                project_id: project_id.clone(),
                authority,
                capabilities: capabilities.clone(),
                contract_path: contract_path.clone(),
                name: display_name.clone(),
                root: canonical_root.clone(),
                known_paths: vec![canonical_root],
                fingerprint,
                state_root,
                profile: workspace.profile().clone(),
                attached_unix_ms: now,
                last_opened_unix_ms: now,
            };
            state.workspaces.push(record.clone());
            record
        };

        fs::create_dir_all(&record.state_root).map_err(|error| error.to_string())?;
        workspace
            .migrate_legacy_state_to(&record.state_root)
            .map_err(|error| error.to_string())?;

        state.active_workspace_id = Some(record.id.clone());
        state
            .workspaces
            .sort_by_key(|workspace| Reverse(workspace.last_opened_unix_ms));
        self.save(&state)?;
        Ok(record)
    }

    pub fn attach_preserving_active(
        &self,
        workspace: &Workspace,
        previous_active: Option<&str>,
    ) -> Result<RegisteredWorkspace, String> {
        let record = self.attach(workspace)?;
        self.restore_active_workspace(previous_active)?;
        Ok(record)
    }

    pub fn restore_active_workspace(&self, workspace_id: Option<&str>) -> Result<(), String> {
        let mut state = self.state()?;
        match workspace_id {
            Some(id) => {
                if !state.workspaces.iter().any(|workspace| workspace.id == id) {
                    return Err(format!(
                        "cannot restore unknown Cortex active workspace: {id}"
                    ));
                }
                state.active_workspace_id = Some(id.to_string());
            }
            None => state.active_workspace_id = None,
        }
        self.save(&state)
    }

    pub fn rebind_workspace_root(
        &self,
        id: &str,
        new_root: impl AsRef<Path>,
    ) -> Result<RegisteredWorkspace, String> {
        let canonical = fs::canonicalize(new_root.as_ref()).map_err(|error| {
            format!(
                "failed to resolve promoted Cortex workspace {}: {error}",
                new_root.as_ref().display()
            )
        })?;
        let workspace = Workspace::open(&canonical).map_err(|error| error.to_string())?;
        let mut state = self.state()?;
        let record = state
            .workspaces
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or_else(|| format!("registered Cortex workspace not found: {id}"))?;
        if record.root != canonical && !record.known_paths.iter().any(|path| path == &record.root) {
            record.known_paths.push(record.root.clone());
        }
        if !record.known_paths.iter().any(|path| path == &canonical) {
            record.known_paths.push(canonical.clone());
        }
        record.root = canonical.clone();
        record.profile = workspace.profile().clone();
        record.fingerprint = workspace_fingerprint(&workspace);
        if let Some(contract) = ProjectContract::load_optional(&canonical)? {
            record.name = contract.project.name.clone();
            record.project_id = Some(contract.project.id.clone());
            record.authority = contract.project.authority;
            record.capabilities = contract.capabilities();
            record.contract_path = Some(canonical.join("project.control.json"));
        } else {
            record.name = workspace.profile().name.clone();
            record.project_id = None;
            record.authority = ProjectAuthority::Unknown;
            record.capabilities.clear();
            record.contract_path = None;
        }
        record.last_opened_unix_ms = unix_millis();
        let result = record.clone();
        self.save(&state)?;
        Ok(result)
    }

    pub fn open_workspace(&self, record: &RegisteredWorkspace) -> Result<Workspace, String> {
        Workspace::open_with_state_root(&record.root, &record.state_root)
            .map_err(|error| error.to_string())
    }

    pub fn resolve_for_root(&self, root: &Path) -> Result<Option<RegisteredWorkspace>, String> {
        let canonical = fs::canonicalize(root).map_err(|error| error.to_string())?;
        Ok(self.state()?.workspaces.into_iter().find(|entry| {
            entry.root == canonical
                || entry
                    .known_paths
                    .iter()
                    .any(|path| path == &canonical && !path.exists())
        }))
    }

    pub fn set_active(&self, id: &str) -> Result<RegisteredWorkspace, String> {
        let mut state = self.state()?;
        let now = unix_millis();
        let workspace = state
            .workspaces
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or_else(|| format!("registered Cortex workspace not found: {id}"))?;
        workspace.last_opened_unix_ms = now;
        let result = workspace.clone();
        state.active_workspace_id = Some(id.to_string());
        state
            .workspaces
            .sort_by_key(|workspace| Reverse(workspace.last_opened_unix_ms));
        self.save(&state)?;
        Ok(result)
    }

    pub fn detach(&self, id: &str) -> Result<bool, String> {
        let mut state = self.state()?;
        let before = state.workspaces.len();
        state.workspaces.retain(|workspace| workspace.id != id);
        let removed = state.workspaces.len() != before;
        if state.active_workspace_id.as_deref() == Some(id) {
            state.active_workspace_id = state
                .workspaces
                .first()
                .map(|workspace| workspace.id.clone());
        }
        if removed {
            self.save(&state)?;
        }
        Ok(removed)
    }

    fn save(&self, state: &WorkspaceRegistryState) -> Result<(), String> {
        let mut normalized = state.clone();
        normalized.schema_version = REGISTRY_SCHEMA_VERSION;
        let bytes = serde_json::to_vec_pretty(&normalized).map_err(|error| error.to_string())?;
        let temp = self.registry_path.with_extension("json.tmp");
        fs::write(&temp, bytes)
            .map_err(|error| format!("failed to write {}: {error}", temp.display()))?;
        if self.registry_path.exists() {
            fs::remove_file(&self.registry_path).map_err(|error| {
                format!(
                    "failed to replace Cortex workspace registry {}: {error}",
                    self.registry_path.display()
                )
            })?;
        }
        fs::rename(&temp, &self.registry_path).map_err(|error| {
            format!(
                "failed to publish Cortex workspace registry {}: {error}",
                self.registry_path.display()
            )
        })
    }
}

pub fn cortex_home() -> Result<PathBuf, String> {
    discover_cortex_home().map_err(|error| error.to_string())
}

fn discover_cortex_project_root() -> Option<PathBuf> {
    let mut starts = Vec::new();
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            starts.push(parent.to_path_buf());
        }
    }
    if let Ok(current) = std::env::current_dir() {
        starts.push(current);
    }
    for start in starts {
        for ancestor in start.ancestors() {
            let Ok(Some(contract)) = ProjectContract::load_optional(ancestor) else {
                continue;
            };
            if contract.project.id.is_cortex() {
                return fs::canonicalize(ancestor).ok();
            }
        }
    }
    None
}

fn is_generated_workspace_root(path: &Path) -> bool {
    let parts = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(|part| part.to_ascii_lowercase())
        .collect::<Vec<_>>();

    parts.windows(2).any(|pair| {
        matches!(pair[0].as_str(), "target" | "build" | "builds")
            && matches!(pair[1].as_str(), "debug" | "release" | "relwithdebinfo")
    }) || parts.last().is_some_and(|last| {
        matches!(
            last.as_str(),
            "target" | "node_modules" | ".cache" | "deriveddatacache" | "intermediate"
        )
    })
}

fn is_ignored_directory(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        name.to_ascii_lowercase().as_str(),
        "windows"
            | "windows.old"
            | "program files"
            | "program files (x86)"
            | "programdata"
            | "system volume information"
            | "$recycle.bin"
            | "recovery"
            | "appdata"
            | ".git"
            | ".cortex"
            | ".idea"
            | ".vs"
            | ".gradle"
            | ".cargo"
            | ".rustup"
            | ".nuget"
            | ".npm"
            | ".pnpm-store"
            | ".yarn"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".pytest_cache"
            | ".mypy_cache"
            | ".ruff_cache"
            | "target"
            | "debug"
            | "release"
            | "build"
            | "builds"
            | "bin"
            | "obj"
            | "node_modules"
            | "dist"
            | "out"
            | "cache"
            | ".cache"
            | "intermediate"
            | "saved"
            | "deriveddatacache"
    )
}

fn classify_candidate(
    root: &Path,
    registered: &[RegisteredWorkspace],
) -> Result<Option<LibraryCandidate>, String> {
    let marker_specs = [
        ("Cargo.toml", "Rust / Cargo"),
        ("CMakeLists.txt", "C/C++ / CMake"),
        ("package.json", "Node / JavaScript"),
        ("pyproject.toml", "Python"),
        ("go.mod", "Go"),
        ("pom.xml", "Java / Maven"),
        ("build.gradle", "Java / Gradle"),
        ("build.gradle.kts", "Kotlin / Gradle"),
        ("project.godot", "Godot"),
    ];
    let mut markers = marker_specs
        .iter()
        .filter(|(marker, _)| root.join(marker).is_file())
        .map(|(marker, _)| (*marker).to_string())
        .collect::<Vec<_>>();

    if root.join(".git").exists() {
        markers.push(".git".into());
    }
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.filter_map(Result::ok).take(256) {
            let name = entry.file_name().to_string_lossy().to_string();
            let lower = name.to_ascii_lowercase();
            if lower.ends_with(".sln")
                || lower.ends_with(".csproj")
                || lower.ends_with(".vcxproj")
                || lower.ends_with(".uproject")
            {
                markers.push(name);
            }
        }
    }
    markers.sort();
    markers.dedup();

    let source_signal = root.join("src").is_dir()
        || root.join("Source").is_dir()
        || root.join("app").is_dir()
        || root.join("crates").is_dir();
    if markers.is_empty() && !source_signal {
        return Ok(None);
    }

    let manifest_count = markers
        .iter()
        .filter(|marker| marker.as_str() != ".git")
        .count();
    let has_git = markers.iter().any(|marker| marker == ".git");
    let confidence = if manifest_count >= 2 {
        100
    } else if manifest_count == 1 && has_git {
        95
    } else if manifest_count == 1 {
        88
    } else if has_git && source_signal {
        72
    } else {
        55
    };

    let kind = if root.join("Cargo.toml").is_file() {
        "Rust / Cargo"
    } else if root.join("CMakeLists.txt").is_file() {
        "C/C++ / CMake"
    } else if root.join("project.godot").is_file() {
        "Godot"
    } else if markers
        .iter()
        .any(|marker| marker.to_ascii_lowercase().ends_with(".uproject"))
    {
        "Unreal"
    } else if markers.iter().any(|marker| {
        marker.to_ascii_lowercase().ends_with(".sln")
            || marker.to_ascii_lowercase().ends_with(".csproj")
    }) {
        ".NET / Visual Studio"
    } else if root.join("package.json").is_file() {
        "Node / JavaScript"
    } else if root.join("pyproject.toml").is_file() {
        "Python"
    } else {
        "Source / repository"
    };

    let languages = detect_languages(root, 600);
    let canonical = fs::canonicalize(root).map_err(|error| error.to_string())?;
    let already_registered = registered
        .iter()
        .any(|workspace| workspace.root == canonical);
    let name = canonical
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("Project")
        .to_string();
    Ok(Some(LibraryCandidate {
        root: canonical,
        name,
        kind: kind.into(),
        confidence,
        markers,
        languages,
        already_registered,
    }))
}

fn detect_languages(root: &Path, max_files: usize) -> Vec<String> {
    let mut languages = BTreeSet::new();
    let mut seen = 0_usize;
    let mut stack = vec![(root.to_path_buf(), 0_u8)];
    while let Some((directory, depth)) = stack.pop() {
        if depth > 3 || seen >= max_files {
            continue;
        }
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            if seen >= max_files {
                break;
            }
            let path = entry.path();
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                if !is_ignored_directory(&path) {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            seen += 1;
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let language = match extension.as_str() {
                "rs" => Some("Rust"),
                "c" => Some("C"),
                "cc" | "cpp" | "cxx" | "h" | "hpp" => Some("C++"),
                "cs" => Some("C#"),
                "java" => Some("Java"),
                "kt" | "kts" => Some("Kotlin"),
                "js" | "jsx" => Some("JavaScript"),
                "ts" | "tsx" => Some("TypeScript"),
                "py" => Some("Python"),
                "ps1" => Some("PowerShell"),
                "sh" | "bash" => Some("Shell"),
                "gd" => Some("GDScript"),
                "lua" => Some("Lua"),
                "toml" => Some("TOML"),
                "json" => Some("JSON"),
                "wgsl" => Some("WGSL"),
                "glsl" | "vert" | "frag" => Some("GLSL"),
                _ => None,
            };
            if let Some(language) = language {
                languages.insert(language.to_string());
            }
        }
    }
    languages.into_iter().collect()
}

fn project_node_id(root: &Path) -> String {
    format!("project-{:016x}", fnv_bytes(identity_text(root).as_bytes()))
}

fn infer_project_node_kind(
    root: &Path,
    parent_id: Option<&str>,
    markers: &[String],
) -> ProjectNodeKind {
    let name = root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name.contains("launcher") {
        return ProjectNodeKind::Launcher;
    }
    if name.contains("server") {
        return ProjectNodeKind::Server;
    }
    if name.contains("client") {
        return ProjectNodeKind::Client;
    }
    if name.contains("editor") || name == "ember" {
        return ProjectNodeKind::Editor;
    }
    if name.contains("plugin") || name.contains("extension") {
        return ProjectNodeKind::Plugin;
    }
    if matches!(name.as_str(), "tools" | "tool" | "scripts") {
        return ProjectNodeKind::Tool;
    }
    if matches!(name.as_str(), "examples" | "example" | "samples" | "sample") {
        return ProjectNodeKind::Example;
    }
    if matches!(name.as_str(), "tests" | "test" | "fixtures" | "fixture") {
        return ProjectNodeKind::TestFixture;
    }
    if name.contains("content") || name.contains("asset_pack") || name.contains("assets-pack") {
        return ProjectNodeKind::ContentPack;
    }
    if root.join("Cargo.toml").is_file()
        && fs::read_to_string(root.join("Cargo.toml"))
            .map(|text| text.contains("[workspace]"))
            .unwrap_or(false)
    {
        return ProjectNodeKind::Workspace;
    }
    if markers.iter().any(|marker| marker.ends_with(".sln")) {
        return ProjectNodeKind::Workspace;
    }
    if parent_id.is_some() {
        ProjectNodeKind::Component
    } else {
        ProjectNodeKind::Project
    }
}

fn project_tree_signature(root: &Path, max_files: usize) -> String {
    let mut material = Vec::<u8>::new();
    let mut stack = vec![(root.to_path_buf(), 0_u8)];
    let mut seen = 0_usize;
    while let Some((directory, depth)) = stack.pop() {
        if depth > 5 || seen >= max_files {
            continue;
        }
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            if seen >= max_files {
                break;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if !is_ignored_directory(&path) {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            seen += 1;
            if let Ok(relative) = path.strip_prefix(root) {
                material
                    .extend_from_slice(relative.to_string_lossy().replace('\\', "/").as_bytes());
            }
            if let Ok(metadata) = entry.metadata() {
                material.extend_from_slice(&metadata.len().to_le_bytes());
            }
        }
    }
    format!("tree-{:016x}", fnv_bytes(&material))
}

fn normalized_project_family_name(name: &str) -> String {
    let mut lower = name.to_ascii_lowercase();
    for token in [
        " backup", "-backup", "_backup", " copy", "-copy", "_copy", " old", "-old", "_old",
        " final", "-final", "_final",
    ] {
        lower = lower.replace(token, "");
    }
    let parts = lower
        .split(['-', '_', ' '])
        .filter(|part| !part.is_empty())
        .filter(|part| !part.chars().all(|ch| ch.is_ascii_digit()))
        .filter(|part| !matches!(*part, "v1" | "v2" | "v3" | "v4" | "new" | "latest"))
        .collect::<Vec<_>>();
    if parts.is_empty() {
        lower
    } else {
        parts.join("-")
    }
}

fn build_lineage_candidates(nodes: &[ProjectGraphNode]) -> Vec<ProjectLineageCandidate> {
    let mut families = BTreeMap::<String, Vec<&ProjectGraphNode>>::new();
    for node in nodes {
        families
            .entry(normalized_project_family_name(&node.name))
            .or_default()
            .push(node);
    }
    let mut out = Vec::new();
    for (normalized_name, family) in families {
        if family.len() < 2 {
            continue;
        }
        let exact_signature = family
            .iter()
            .map(|node| node.tree_signature.as_str())
            .collect::<BTreeSet<_>>()
            .len()
            == 1;
        out.push(ProjectLineageCandidate {
            normalized_name,
            project_ids: family.iter().map(|node| node.id.clone()).collect(),
            confidence: if exact_signature { 100 } else { 72 },
            reason: if exact_signature {
                "same normalized project family and sampled tree signature".into()
            } else {
                "same normalized project family name; requires lineage review before merge".into()
            },
        });
    }
    out.sort_by_key(|candidate| Reverse(candidate.confidence));
    out
}

fn add_related_project_edges(nodes: &[ProjectGraphNode], edges: &mut Vec<ProjectGraphEdge>) {
    for candidate in build_lineage_candidates(nodes) {
        if candidate.project_ids.len() < 2 {
            continue;
        }
        let anchor = candidate.project_ids[0].clone();
        for other in candidate.project_ids.iter().skip(1) {
            edges.push(ProjectGraphEdge {
                from: anchor.clone(),
                to: other.clone(),
                kind: ProjectGraphEdgeKind::RelatedCopy,
                confidence: candidate.confidence,
                reason: candidate.reason.clone(),
            });
        }
    }
}

fn classify_catalog_file(path: &Path, inside_project: bool) -> CatalogFileClass {
    let lower_parts = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(|part| part.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if lower_parts.iter().any(|part| {
        matches!(
            part.as_str(),
            "target"
                | "build"
                | "builds"
                | "bin"
                | "obj"
                | "dist"
                | "out"
                | "deriveddatacache"
                | "intermediate"
                | ".cache"
        )
    }) {
        return CatalogFileClass::GeneratedBuild;
    }
    if lower_parts.iter().any(|part| {
        matches!(
            part.as_str(),
            "node_modules" | "vendor" | "third_party" | "packages" | ".gradle" | ".nuget"
        )
    }) {
        return CatalogFileClass::DependencyVendor;
    }
    if lower_parts.iter().any(|part| {
        matches!(
            part.as_str(),
            "steamapps" | "windowsapps" | "program files" | "msys64" | "sdk" | "sdks"
        )
    }) {
        return CatalogFileClass::ToolchainInstalled;
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz"
    ) {
        return CatalogFileClass::ArchiveBackup;
    }
    if matches!(
        extension.as_str(),
        "gguf" | "safetensors" | "ckpt" | "pt" | "pth" | "onnx"
    ) {
        return CatalogFileClass::ModelData;
    }
    if matches!(
        extension.as_str(),
        "md" | "txt" | "pdf" | "doc" | "docx" | "rtf"
    ) {
        return CatalogFileClass::Documentation;
    }
    if matches!(
        extension.as_str(),
        "rs" | "c"
            | "cc"
            | "cpp"
            | "cxx"
            | "h"
            | "hpp"
            | "cs"
            | "java"
            | "kt"
            | "kts"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "py"
            | "lua"
            | "gd"
            | "ps1"
            | "sh"
            | "toml"
            | "json"
            | "yaml"
            | "yml"
            | "xml"
            | "wgsl"
            | "glsl"
            | "vert"
            | "frag"
    ) {
        return CatalogFileClass::ProjectSource;
    }
    if matches!(
        extension.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "webp"
            | "gif"
            | "bmp"
            | "tga"
            | "psd"
            | "ase"
            | "aseprite"
            | "blend"
            | "glb"
            | "gltf"
            | "fbx"
            | "obj"
            | "wav"
            | "ogg"
            | "mp3"
            | "flac"
            | "mid"
            | "midi"
    ) {
        return if inside_project {
            CatalogFileClass::ProjectContent
        } else {
            CatalogFileClass::Media
        };
    }
    if inside_project {
        CatalogFileClass::ProjectContent
    } else {
        CatalogFileClass::Unknown
    }
}

fn catalog_content_fingerprint(path: &Path, bytes: u64) -> Option<String> {
    // Fast first-pass fingerprint. Very large files are scheduled for later
    // content-addressed hashing instead of being synchronously read during scan.
    if bytes > 16 * 1024 * 1024 {
        return None;
    }
    let data = fs::read(path).ok()?;
    let mut material = Vec::with_capacity(data.len().min(1024 * 1024) + 16);
    material.extend_from_slice(&bytes.to_le_bytes());
    if data.len() <= 1024 * 1024 {
        material.extend_from_slice(&data);
    } else {
        material.extend_from_slice(&data[..512 * 1024]);
        material.extend_from_slice(&data[data.len() - 512 * 1024..]);
    }
    Some(format!("fast-{:016x}", fnv_bytes(&material)))
}

#[cfg(windows)]
fn powershell_json(script: &str) -> Result<serde_json::Value, String> {
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("failed to query Windows storage identity: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "Windows storage identity query failed".into()
        } else {
            format!("Windows storage identity query failed: {stderr}")
        });
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return Err("Windows storage identity query returned no data".into());
    }
    serde_json::from_str(&text)
        .map_err(|error| format!("invalid Windows storage identity JSON: {error}; output={text}"))
}

#[cfg(windows)]
fn volume_identity_from_value(value: &serde_json::Value) -> Option<CortexStorageVolumeIdentity> {
    let mount_root = value
        .get("mount_root")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let volume_guid = value
        .get("volume_guid")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if mount_root.is_none() && volume_guid.is_none() {
        return None;
    }
    Some(CortexStorageVolumeIdentity {
        volume_guid,
        serial_number: value
            .get("serial_number")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        filesystem: value
            .get("filesystem")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        label: value
            .get("label")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        drive_type: value
            .get("drive_type")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        mount_root,
        size_bytes: value.get("size_bytes").and_then(serde_json::Value::as_u64),
        free_bytes: value.get("free_bytes").and_then(serde_json::Value::as_u64),
    })
}

#[cfg(windows)]
fn storage_volume_identity_for(path: &Path) -> Result<CortexStorageVolumeIdentity, String> {
    let escaped = path.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$ErrorActionPreference='Stop'; \
         $v=Get-Volume -FilePath '{escaped}'; \
         $serial=$null; \
         if ($null -ne $v.DriveLetter) {{ \
           $device=([string]$v.DriveLetter)+':'; \
           $logical=Get-CimInstance Win32_LogicalDisk -Filter (\\\"DeviceID='\\\"+$device+\\\"'\\\") -ErrorAction SilentlyContinue; \
           if ($null -ne $logical) {{ $serial=[string]$logical.VolumeSerialNumber }} \
         }}; \
         $mount=if ($null -ne $v.DriveLetter) {{ ([string]$v.DriveLetter)+':\\\\' }} else {{ [string]$v.Path }}; \
         [pscustomobject]@{{ \
           volume_guid=[string]$v.UniqueId; \
           serial_number=$serial; \
           filesystem=[string]$v.FileSystem; \
           label=[string]$v.FileSystemLabel; \
           drive_type=[string]$v.DriveType; \
           mount_root=$mount; \
           size_bytes=[uint64]$v.Size; \
           free_bytes=[uint64]$v.SizeRemaining \
         }} | ConvertTo-Json -Compress"
    );
    let value = powershell_json(&script)?;
    volume_identity_from_value(&value).ok_or_else(|| {
        format!(
            "Windows did not return a usable storage identity for {}",
            path.display()
        )
    })
}

#[cfg(not(windows))]
fn storage_volume_identity_for(_path: &Path) -> Result<CortexStorageVolumeIdentity, String> {
    Ok(CortexStorageVolumeIdentity {
        mount_root: Some(PathBuf::from("/")),
        filesystem: None,
        label: None,
        drive_type: Some("fixed".into()),
        volume_guid: None,
        serial_number: None,
        size_bytes: None,
        free_bytes: None,
    })
}

#[cfg(windows)]
fn available_storage_volumes(
    include_removable: bool,
) -> Result<Vec<CortexStorageVolumeIdentity>, String> {
    let removable = if include_removable {
        " -or $_.DriveType.ToString() -eq 'Removable'"
    } else {
        ""
    };
    let script = format!(
        "$ErrorActionPreference='Stop'; \
         $items=@(Get-Volume | Where-Object {{ $_.DriveType.ToString() -eq 'Fixed'{removable} }} | ForEach-Object {{ \
           $v=$_; \
           $serial=$null; \
           if ($null -ne $v.DriveLetter) {{ \
             $device=([string]$v.DriveLetter)+':'; \
             $logical=Get-CimInstance Win32_LogicalDisk -Filter (\\\"DeviceID='\\\"+$device+\\\"'\\\") -ErrorAction SilentlyContinue; \
             if ($null -ne $logical) {{ $serial=[string]$logical.VolumeSerialNumber }} \
           }}; \
           $mount=if ($null -ne $v.DriveLetter) {{ ([string]$v.DriveLetter)+':\\\\' }} else {{ [string]$v.Path }}; \
           [pscustomobject]@{{ \
             volume_guid=[string]$v.UniqueId; \
             serial_number=$serial; \
             filesystem=[string]$v.FileSystem; \
             label=[string]$v.FileSystemLabel; \
             drive_type=[string]$v.DriveType; \
             mount_root=$mount; \
             size_bytes=[uint64]$v.Size; \
             free_bytes=[uint64]$v.SizeRemaining \
           }} \
         }}); $items | ConvertTo-Json -Compress"
    );
    let value = powershell_json(&script)?;
    let values = match value {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Null => Vec::new(),
        other => vec![other],
    };
    let mut volumes = values
        .iter()
        .filter_map(volume_identity_from_value)
        .filter(|volume| volume.mount_root.as_ref().is_some_and(|root| root.is_dir()))
        .collect::<Vec<_>>();
    volumes.sort_by(|left, right| left.mount_root.cmp(&right.mount_root));
    volumes.dedup_by(|left, right| {
        left.volume_guid == right.volume_guid && left.mount_root == right.mount_root
    });
    Ok(volumes)
}

#[cfg(not(windows))]
fn available_storage_volumes(
    _include_removable: bool,
) -> Result<Vec<CortexStorageVolumeIdentity>, String> {
    Ok(vec![storage_volume_identity_for(Path::new("/"))?])
}

#[cfg(windows)]
fn volume_root_for(path: &Path) -> PathBuf {
    if let Some(root) = storage_volume_identity_for(path)
        .ok()
        .and_then(|identity| identity.mount_root)
        .filter(|root| root.is_dir())
    {
        return root;
    }
    use std::path::Component;
    let mut components = path.components();
    match components.next() {
        Some(Component::Prefix(prefix)) => {
            let mut root = PathBuf::from(prefix.as_os_str());
            root.push("\\");
            root
        }
        Some(Component::RootDir) => PathBuf::from("\\"),
        _ => path.parent().unwrap_or(path).to_path_buf(),
    }
}

#[cfg(not(windows))]
fn volume_root_for(path: &Path) -> PathBuf {
    path.parent().unwrap_or(path).to_path_buf()
}

fn storage_authority_has_live_state(root: &Path) -> bool {
    let layout = CortexLibraryLayout::from_root(root.to_path_buf());
    [
        root.join(".cortex"),
        root.join("Cortex"),
        layout.projects,
        layout.vault,
        layout.models,
        layout.git,
        layout.backups,
        layout.recovery,
    ]
    .into_iter()
    .any(|path| {
        fs::read_dir(path)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
    })
}

fn tree_size_without_links(root: &Path) -> Result<u64, String> {
    let mut total = 0_u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("failed to read {}: {error}", directory.display()))?
            .filter_map(Result::ok)
        {
            let file_type = entry.file_type().map_err(|error| error.to_string())?;
            if file_type.is_symlink() {
                return Err(format!(
                    "migration planning found a symbolic link that requires explicit handling: {}",
                    entry.path().display()
                ));
            }
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                total = total
                    .saturating_add(entry.metadata().map_err(|error| error.to_string())?.len());
            }
        }
    }
    Ok(total)
}

fn copy_tree_verified(source: &Path, target: &Path) -> Result<MigrationCopyStats, String> {
    if target.exists() {
        return Err(format!(
            "migration staging target already exists: {}",
            target.display()
        ));
    }
    fs::create_dir_all(target).map_err(|error| error.to_string())?;
    let mut stats = MigrationCopyStats::default();
    let mut stack = vec![(source.to_path_buf(), target.to_path_buf())];
    while let Some((source_dir, target_dir)) = stack.pop() {
        for entry in fs::read_dir(&source_dir)
            .map_err(|error| format!("failed to read {}: {error}", source_dir.display()))?
            .filter_map(Result::ok)
        {
            let file_type = entry.file_type().map_err(|error| error.to_string())?;
            let source_path = entry.path();
            let target_path = target_dir.join(entry.file_name());
            if file_type.is_symlink() {
                return Err(format!(
                    "Cortex project migration refuses symbolic links until an explicit link policy is approved: {}",
                    source_path.display()
                ));
            }
            if file_type.is_dir() {
                fs::create_dir_all(&target_path).map_err(|error| {
                    format!(
                        "failed to create migration directory {}: {error}",
                        target_path.display()
                    )
                })?;
                stack.push((source_path, target_path));
                continue;
            }
            if !file_type.is_file() {
                return Err(format!(
                    "Cortex project migration encountered an unsupported filesystem object: {}",
                    source_path.display()
                ));
            }
            let bytes = entry.metadata().map_err(|error| error.to_string())?.len();
            fs::copy(&source_path, &target_path).map_err(|error| {
                format!(
                    "failed to copy {} to {}: {error}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
            stats.files = stats.files.saturating_add(1);
            stats.bytes = stats.bytes.saturating_add(bytes);
            if !files_equal(&source_path, &target_path)? {
                return Err(format!(
                    "byte verification failed for migrated file {}",
                    source_path.display()
                ));
            }
            stats.verified_files = stats.verified_files.saturating_add(1);
        }
    }
    Ok(stats)
}

fn files_equal(left: &Path, right: &Path) -> Result<bool, String> {
    let left_meta = fs::metadata(left).map_err(|error| error.to_string())?;
    let right_meta = fs::metadata(right).map_err(|error| error.to_string())?;
    if left_meta.len() != right_meta.len() {
        return Ok(false);
    }
    let mut left_reader = BufReader::new(fs::File::open(left).map_err(|error| error.to_string())?);
    let mut right_reader =
        BufReader::new(fs::File::open(right).map_err(|error| error.to_string())?);
    let mut left_buffer = [0_u8; 64 * 1024];
    let mut right_buffer = [0_u8; 64 * 1024];
    loop {
        let left_read = left_reader
            .read(&mut left_buffer)
            .map_err(|error| error.to_string())?;
        let right_read = right_reader
            .read(&mut right_buffer)
            .map_err(|error| error.to_string())?;
        if left_read != right_read {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
        if left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
    }
}

fn safe_managed_name(name: &str) -> String {
    let value = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let value = value.trim_matches('_');
    if value.is_empty() {
        "Project".into()
    } else {
        value.to_string()
    }
}

fn intake_tag_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.cortex-intake.json", path.to_string_lossy()))
}

fn is_intake_tag(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.ends_with(".cortex-intake.json"))
}

fn classify_intake_kind(name: &str) -> IntakeKind {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".zip") && (lower.contains("handoff") || lower.contains("patch")) {
        IntakeKind::HandoffPatch
    } else if lower.ends_with(".zip") && (lower.contains("source") || lower.contains("rollup")) {
        IntakeKind::SourceRollup
    } else if lower.ends_with(".zip") && lower.contains("debug") {
        IntakeKind::DebugBundle
    } else if lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".webp")
    {
        IntakeKind::Image
    } else if lower.ends_with(".md")
        || lower.ends_with(".txt")
        || lower.ends_with(".pdf")
        || lower.ends_with(".docx")
    {
        IntakeKind::Document
    } else if lower.ends_with(".log") {
        IntakeKind::Log
    } else if lower.ends_with(".zip")
        || lower.ends_with(".7z")
        || lower.ends_with(".tar")
        || lower.ends_with(".gz")
    {
        IntakeKind::Archive
    } else {
        IntakeKind::Unknown
    }
}

fn workspace_fingerprint(workspace: &Workspace) -> String {
    let candidates = [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "CMakeLists.txt",
        "go.mod",
        ".git/config",
    ];
    let mut material = Vec::<u8>::new();
    for relative in candidates {
        let path = workspace.root().join(relative);
        if let Ok(bytes) = fs::read(&path) {
            material.extend_from_slice(relative.as_bytes());
            material.extend_from_slice(&bytes[..bytes.len().min(128 * 1024)]);
        }
    }
    if material.is_empty() {
        material.extend_from_slice(format!("{:?}", workspace.profile().kinds).as_bytes());
        material.extend_from_slice(format!("{:?}", workspace.profile().build_systems).as_bytes());
    }
    format!("fp-{:016x}", fnv_bytes(&material))
}

fn legacy_record_fingerprint(record: &RegisteredWorkspace) -> String {
    let material = format!(
        "{}|{:?}|{:?}",
        record.name, record.profile.kinds, record.profile.build_systems
    );
    format!("fp-{:016x}", fnv_bytes(material.as_bytes()))
}

fn new_workspace_id(fingerprint: &str) -> String {
    let material = format!("{fingerprint}|{}|{}", unix_millis(), std::process::id());
    format!("ws-{:016x}", fnv_bytes(material.as_bytes()))
}

fn fnv_bytes(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn workspace_id(root: &Path) -> String {
    let text = identity_text(root);
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("ws-{hash:016x}")
}

#[cfg(windows)]
fn identity_text(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

#[cfg(not(windows))]
fn identity_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(windows)]
fn path_starts_with(path: &Path, root: &Path) -> bool {
    let path = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let root = root
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    path == root || path.starts_with(&(root.trim_end_matches('/').to_string() + "/"))
}

#[cfg(not(windows))]
fn path_starts_with(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

#[cfg(windows)]
fn windows_path_relative_to_drive(path: &Path) -> Option<PathBuf> {
    use std::path::Component;

    let mut saw_prefix = false;
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) => saw_prefix = true,
            Component::RootDir | Component::CurDir => {}
            Component::Normal(value) => relative.push(value),
            Component::ParentDir => return None,
        }
    }
    if saw_prefix && !relative.as_os_str().is_empty() {
        Some(relative)
    } else {
        None
    }
}

#[cfg(windows)]
fn rebase_portable_workspace_paths(state: &mut WorkspaceRegistryState, home: &Path) -> bool {
    let Some(vault_root) = configured_vault_root_from_environment() else {
        return false;
    };
    let mut changed = false;
    for workspace in &mut state.workspaces {
        let expected_state_root = home.join("workspaces").join(&workspace.id);
        if workspace.state_root != expected_state_root {
            workspace.state_root = expected_state_root;
            changed = true;
        }

        if workspace.root.exists() {
            continue;
        }
        let Some(relative) = windows_path_relative_to_drive(&workspace.root) else {
            continue;
        };
        let candidate = vault_root.join(relative);
        if !candidate.is_dir() {
            continue;
        }

        let previous = workspace.root.clone();
        if !workspace.known_paths.iter().any(|path| path == &previous) {
            workspace.known_paths.push(previous);
        }
        workspace.root = candidate.clone();
        if let Ok(reopened) = Workspace::open(&candidate) {
            workspace.profile = reopened.profile().clone();
            workspace.fingerprint = workspace_fingerprint(&reopened);
        } else {
            workspace.profile.root = candidate.clone();
        }
        workspace.contract_path = if candidate.join("project.control.json").is_file() {
            Some(candidate.join("project.control.json"))
        } else {
            None
        };
        changed = true;
    }
    changed
}

fn configured_vault_root_from_environment() -> Option<PathBuf> {
    ["CORTEX_VAULT_ROOT", "PCC_VAULT_ROOT"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .find(|path| !path.as_os_str().is_empty())
}

fn portable_drive_root_policy_enabled(workspace_root: &Path) -> Result<bool, String> {
    let path = workspace_root
        .join("config")
        .join("cortex")
        .join("portable_drive_root_vault.v1.json");
    if !path.is_file() {
        return Ok(false);
    }
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).map_err(|error| {
        format!(
            "failed to read Cortex portable Vault policy {}: {error}",
            path.display()
        )
    })?)
    .map_err(|error| {
        format!(
            "invalid Cortex portable Vault policy {}: {error}",
            path.display()
        )
    })?;
    let schema = value
        .get("schema")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if schema != "cortex.portable_drive_root_vault.v1" {
        return Err(format!(
            "unsupported Cortex portable Vault policy schema in {}: {schema}",
            path.display()
        ));
    }
    let enabled = value
        .get("enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !enabled {
        return Ok(false);
    }
    let mode = value
        .get("mode")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if mode != "repository_drive_root" {
        return Err(format!(
            "unsupported Cortex portable Vault policy mode in {}: {mode}",
            path.display()
        ));
    }
    Ok(true)
}

fn portable_drive_root_for_workspace(workspace_root: &Path) -> Result<Option<PathBuf>, String> {
    if !portable_drive_root_policy_enabled(workspace_root)? {
        return Ok(None);
    }

    #[cfg(windows)]
    {
        windows_drive_root(workspace_root)
            .map(Some)
            .ok_or_else(|| {
                format!(
                    "portable drive-root Vault policy is enabled but no Windows drive root could be derived from {}",
                    workspace_root.display()
                )
            })
    }

    #[cfg(not(windows))]
    {
        Ok(None)
    }
}

#[cfg(windows)]
fn windows_drive_root(path: &Path) -> Option<PathBuf> {
    use std::path::{Component, Prefix};

    for component in path.components() {
        if let Component::Prefix(prefix) = component {
            return match prefix.kind() {
                Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                    Some(PathBuf::from(format!("{}:\\", char::from(letter))))
                }
                _ => None,
            };
        }
    }
    None
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_root(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "cortex-registry-test-{}-{}-{label}",
            std::process::id(),
            unix_millis()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn portable_drive_root_policy_is_explicit_and_non_mutating() {
        let base = temporary_root("portable-policy");
        let policy_dir = base.join("config").join("cortex");
        fs::create_dir_all(&policy_dir).unwrap();
        fs::write(
            policy_dir.join("portable_drive_root_vault.v1.json"),
            r#"{
  "schema": "cortex.portable_drive_root_vault.v1",
  "enabled": true,
  "mode": "repository_drive_root"
}"#,
        )
        .unwrap();
        assert!(portable_drive_root_policy_enabled(&base).unwrap());
        assert!(!base.join("registry").exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn portable_drive_root_policy_rejects_unknown_mode() {
        let base = temporary_root("portable-policy-bad-mode");
        let policy_dir = base.join("config").join("cortex");
        fs::create_dir_all(&policy_dir).unwrap();
        fs::write(
            policy_dir.join("portable_drive_root_vault.v1.json"),
            r#"{
  "schema": "cortex.portable_drive_root_vault.v1",
  "enabled": true,
  "mode": "some_other_mode"
}"#,
        )
        .unwrap();
        assert!(portable_drive_root_policy_enabled(&base).is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_root_handles_normal_and_verbatim_drive_paths() {
        assert_eq!(
            windows_drive_root(Path::new(r"E:\Cortex")),
            Some(PathBuf::from(r"E:\"))
        );
        assert_eq!(
            windows_drive_root(Path::new(r"\\?\E:\Cortex")),
            Some(PathBuf::from(r"E:\"))
        );
    }

    #[test]
    fn library_provisioning_creates_cortex_ecosystem_layout() {
        let base = temporary_root("library-provision");
        let home = base.join("home");
        let library = base.join("Cortex");

        let registry = WorkspaceRegistry::open(&home).unwrap();
        let layout = registry.provision_library_root(&library).unwrap();

        assert!(layout.projects.is_dir());
        assert!(layout.vault.is_dir());
        assert!(layout.vault_assets.is_dir());
        assert!(layout.vault_logic.is_dir());
        assert!(layout.vault_templates.is_dir());
        assert!(layout.vault_code.is_dir());
        assert!(layout.vault_knowledge.is_dir());
        assert!(layout.vault_generated.is_dir());
        assert!(layout.vault_provenance.is_dir());
        assert!(layout.git.is_dir());
        assert!(layout.git_mirrors.is_dir());
        assert!(layout.git_recovery.is_dir());
        assert!(layout.cortex_data.is_dir());
        assert!(layout.cortex_search.is_dir());
        assert!(layout.cortex_memory.is_dir());
        assert!(layout.cortex_tasks.is_dir());
        assert!(layout.cortex_logs.is_dir());
        assert!(layout.models.is_dir());
        assert!(layout.models_chat.is_dir());
        assert!(layout.models_coding.is_dir());
        assert!(layout.models_reasoning.is_dir());
        assert!(layout.models_vision.is_dir());
        assert!(layout.models_embeddings.is_dir());
        assert!(layout.models_imported.is_dir());
        assert!(layout.intake.is_dir());
        assert!(layout.intake_unassigned.is_dir());
        assert!(layout.shared.is_dir());
        assert!(layout.artifacts.is_dir());
        assert!(layout.backups.is_dir());
        assert!(layout.exports.is_dir());
        assert!(layout.internal.is_dir());
        assert!(layout.models_image_generation.is_dir());
        assert!(layout.image_workflows.is_dir());
        assert!(layout.image_imported.is_dir());
        assert_eq!(
            registry.library_root().unwrap().unwrap().root,
            fs::canonicalize(&library).unwrap()
        );

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn project_intake_root_is_scoped_by_workspace_id() {
        let base = temporary_root("project-intake");
        let home = base.join("home");
        let library = base.join("Cortex");
        let registry = WorkspaceRegistry::open(&home).unwrap();
        registry.provision_library_root(&library).unwrap();

        let intake = registry.project_intake_root("ws-1234").unwrap();
        assert!(intake.is_dir());
        assert!(intake.ends_with(Path::new("Intake").join("ws-1234")));
        assert!(registry.project_intake_root("../escape").is_err());

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn intake_classification_and_stability_are_project_scoped() {
        let base = temporary_root("intake-scan");
        let home = base.join("home");
        let library = base.join("Cortex");
        let project = library.join("Projects").join("demo");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname='demo'\nversion='0.1.0'\n",
        )
        .unwrap();

        let registry = WorkspaceRegistry::open(&home).unwrap();
        registry.provision_library_root(&library).unwrap();
        let record = registry.attach_candidate(&project).unwrap();
        let intake = registry.project_intake_root(&record.id).unwrap();
        let patch = intake.join("Demo_Handoff_R1.zip");
        fs::write(&patch, b"placeholder").unwrap();

        let report = registry.scan_intake(0, 100).unwrap();
        let item = report.items.iter().find(|item| item.path == patch).unwrap();
        assert_eq!(item.project_id.as_deref(), Some(record.id.as_str()));
        assert_eq!(item.kind, IntakeKind::HandoffPatch);
        assert!(item.stable);

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn intake_sidecar_can_assign_an_unassigned_item() {
        let base = temporary_root("intake-tag");
        let home = base.join("home");
        let library = base.join("Cortex");
        let project = library.join("Projects").join("demo");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname='demo'\nversion='0.1.0'\n",
        )
        .unwrap();

        let registry = WorkspaceRegistry::open(&home).unwrap();
        let layout = registry.provision_library_root(&library).unwrap();
        let record = registry.attach_candidate(&project).unwrap();
        let item = layout.intake_unassigned.join("notes.md");
        fs::write(&item, b"notes").unwrap();
        registry.write_intake_tag(&item, &record.id).unwrap();

        let report = registry.scan_intake(0, 100).unwrap();
        assert_eq!(
            report
                .items
                .iter()
                .find(|entry| entry.path == item)
                .unwrap()
                .project_id
                .as_deref(),
            Some(record.id.as_str())
        );

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn drive_catalog_preserves_nested_project_graph_and_streams_files() {
        let base = temporary_root("drive-catalog");
        let home = base.join("home");
        let library = base.join("CortexLibrary");
        let workspace = base.join("Workspace");
        let child = workspace.join("tools").join("launcher");
        fs::create_dir_all(workspace.join("src")).unwrap();
        fs::create_dir_all(child.join("src")).unwrap();
        fs::write(
            workspace.join("Cargo.toml"),
            "[workspace]\nmembers=[\"tools/launcher\"]\n",
        )
        .unwrap();
        fs::write(workspace.join("src").join("lib.rs"), "pub fn root() {}\n").unwrap();
        fs::write(
            child.join("Cargo.toml"),
            "[package]\nname=\"launcher\"\nversion=\"0.1.0\"\n",
        )
        .unwrap();
        fs::write(child.join("src").join("main.rs"), "fn main() {}\n").unwrap();

        let registry = WorkspaceRegistry::open(&home).unwrap();
        registry.provision_library_root(&library).unwrap();
        let report = registry.scan_drive_catalog(&base, 1000, 10_000).unwrap();
        let workspace_node = report
            .project_nodes
            .iter()
            .find(|node| node.root == fs::canonicalize(&workspace).unwrap())
            .unwrap();
        let child_node = report
            .project_nodes
            .iter()
            .find(|node| node.root == fs::canonicalize(&child).unwrap())
            .unwrap();
        assert_eq!(workspace_node.kind, ProjectNodeKind::Workspace);
        assert_eq!(
            child_node.parent_id.as_deref(),
            Some(workspace_node.id.as_str())
        );
        assert_eq!(child_node.kind, ProjectNodeKind::Launcher);
        assert!(report.catalog_file.is_file());
        assert!(report.scanned_files >= 4);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn duplicate_project_names_become_lineage_candidates_not_auto_merges() {
        let base = temporary_root("lineage");
        let home = base.join("home");
        let library = base.join("CortexLibrary");
        for name in ["Game", "Game-backup"] {
            let root = base.join(name);
            fs::create_dir_all(root.join("src")).unwrap();
            fs::write(
                root.join("Cargo.toml"),
                "[package]\nname=\"game\"\nversion=\"0.1.0\"\n",
            )
            .unwrap();
            fs::write(root.join("src/lib.rs"), "pub fn same() {}\n").unwrap();
        }
        let registry = WorkspaceRegistry::open(&home).unwrap();
        registry.provision_library_root(&library).unwrap();
        let report = registry.scan_drive_catalog(&base, 1000, 10_000).unwrap();
        assert!(report
            .lineage_candidates
            .iter()
            .any(|candidate| candidate.project_ids.len() >= 2));
        assert_eq!(
            registry.list().unwrap().len(),
            0,
            "cataloging must not auto-register projects"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn migration_plan_is_dry_run_and_never_moves_source() {
        let base = temporary_root("migration-plan");
        let home = base.join("home");
        let library = base.join("CortexLibrary");
        let project = base.join("Demo");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname=\"demo\"\nversion=\"0.1.0\"\n",
        )
        .unwrap();
        fs::write(project.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();
        let registry = WorkspaceRegistry::open(&home).unwrap();
        registry.provision_library_root(&library).unwrap();
        let report = registry.scan_drive_catalog(&base, 1000, 10_000).unwrap();
        let node = report
            .project_nodes
            .iter()
            .find(|node| node.name == "Demo")
            .unwrap();
        let plan = registry.propose_managed_migration(&node.id).unwrap();
        assert_eq!(plan.status, MigrationPlanStatus::Draft);
        assert!(project.join("src/lib.rs").is_file());
        assert!(!library.join("Projects").join("Demo").exists());
        assert!(plan.note.contains("DRY RUN ONLY"));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn drive_catalog_can_cancel_without_mutating_source() {
        let base = temporary_root("catalog-cancel");
        let home = base.join("home");
        let project = base.join("Demo");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname=\"demo\"\nversion=\"0.1.0\"\n",
        )
        .unwrap();
        fs::write(project.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();
        let registry = WorkspaceRegistry::open(&home).unwrap();
        let mut calls = 0_u32;
        let report = registry
            .scan_drive_catalog_with_cancel(&base, 1000, 10_000, || {
                calls += 1;
                calls > 2
            })
            .unwrap();
        assert!(report.cancelled);
        assert!(project.join("src/lib.rs").is_file());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn generated_workspace_roots_are_precise() {
        assert!(is_generated_workspace_root(Path::new(
            r"C:\code\demo\target\debug"
        )));
        assert!(is_generated_workspace_root(Path::new(
            r"C:\code\demo\build\release"
        )));
        assert!(!is_generated_workspace_root(Path::new(r"D:\debug")));
        assert!(!is_generated_workspace_root(Path::new(
            r"D:\Projects\debug"
        )));
    }

    #[test]
    fn library_scan_detects_projects_and_ignores_build_output() {
        let base = temporary_root("library-scan");
        let home = base.join("home");
        let library = base.join("library");
        let project = library.join("demo");
        let ignored = library.join("target").join("debug");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::create_dir_all(&ignored).unwrap();
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(project.join("src").join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(ignored.join("Cargo.toml"), "[package]\nname='wrong'\n").unwrap();

        let registry = WorkspaceRegistry::open(&home).unwrap();
        registry.set_library_root(&library).unwrap();
        let report = registry.scan_library(1_000).unwrap();
        assert!(report
            .candidates
            .iter()
            .any(|candidate| candidate.root == fs::canonicalize(&project).unwrap()));
        assert!(!report
            .candidates
            .iter()
            .any(|candidate| candidate.root == fs::canonicalize(&ignored).unwrap()));

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn identical_live_project_copies_get_distinct_workspace_state() {
        let base = temporary_root("registry-copy-isolation");
        let project_a = base.join("project-a");
        let project_b = base.join("project-b");
        let home = base.join("home");
        fs::create_dir_all(&project_a).unwrap();
        fs::create_dir_all(&project_b).unwrap();

        let manifest = "[package]\nname = \"same-project\"\nversion = \"0.1.0\"\n";
        fs::write(project_a.join("Cargo.toml"), manifest).unwrap();
        fs::write(project_b.join("Cargo.toml"), manifest).unwrap();

        let registry = WorkspaceRegistry::open(&home).unwrap();
        let a = registry
            .attach(&Workspace::open(&project_a).unwrap())
            .unwrap();
        let b = registry
            .attach(&Workspace::open(&project_b).unwrap())
            .unwrap();

        assert_ne!(a.id, b.id);
        assert_ne!(a.state_root, b.state_root);
        assert_eq!(registry.list().unwrap().len(), 2);

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn managed_migration_copies_verifies_rebinds_and_preserves_original() {
        let base = temporary_root("migration-apply");
        let home = base.join("home");
        let library = base.join("CortexStore");
        let project = base.join("Demo");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname=\"demo\"\nversion=\"0.1.0\"\n",
        )
        .unwrap();
        fs::write(project.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();

        let registry = WorkspaceRegistry::open(&home).unwrap();
        registry.provision_library_root(&library).unwrap();
        let report = registry.scan_drive_catalog(&base, 10_000, 100_000).unwrap();
        let node = report
            .project_nodes
            .iter()
            .find(|node| node.name == "Demo")
            .unwrap();
        let plan = registry.propose_managed_migration(&node.id).unwrap();
        let result = registry.apply_managed_migration(&plan.id).unwrap();

        assert!(result.source_preserved);
        assert_eq!(result.copied_files, result.verified_files);
        assert!(project.join("src/lib.rs").is_file());
        assert!(result.target.join("src/lib.rs").is_file());
        assert!(registry.resolve_for_root(&project).unwrap().is_none());
        assert_eq!(
            fs::read(project.join("src/lib.rs")).unwrap(),
            fs::read(result.target.join("src/lib.rs")).unwrap()
        );
        let registered = registry
            .resolve_for_root(&result.target)
            .unwrap()
            .expect("promoted project should be registered");
        assert_eq!(registered.id, result.workspace_id);
        assert_eq!(
            registry.migration_plan(&plan.id).unwrap().status,
            MigrationPlanStatus::PromotedPendingValidation
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn generated_and_system_directories_are_pruned_during_catalog() {
        for name in [
            "Windows",
            "Program Files",
            "ProgramData",
            "$Recycle.Bin",
            "node_modules",
            ".cargo",
            ".venv",
            "__pycache__",
        ] {
            assert!(is_ignored_directory(Path::new(name)), "{name}");
        }
        assert!(!is_ignored_directory(Path::new("Projects")));
        assert!(!is_ignored_directory(Path::new("Havenwild")));
    }

    #[test]
    fn library_root_round_trip_and_path_authority() {
        let base = temporary_root("library");
        let project = base.join("project");
        let home = base.join("home");
        fs::create_dir_all(&project).unwrap();

        let registry = WorkspaceRegistry::open(&home).unwrap();
        let library = registry.set_library_root(&base).unwrap();
        assert_eq!(library.root, fs::canonicalize(&base).unwrap());
        assert!(registry.path_is_in_library(&project).unwrap());
        assert!(!registry
            .path_is_in_library(std::env::temp_dir())
            .unwrap_or(false));
        assert!(registry.clear_library_root().unwrap());
        assert!(registry.library_root().unwrap().is_none());

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn service_ports_are_stable_and_distinct_per_workspace() {
        let base = temporary_root("ports");
        let home = base.join("home");
        let registry = WorkspaceRegistry::open(&home).unwrap();

        let first = registry.service_port_for("ws-one", 7337).unwrap();
        let first_again = registry.service_port_for("ws-one", 7337).unwrap();
        let second = registry.service_port_for("ws-two", 7337).unwrap();

        assert_eq!(first, first_again);
        assert_ne!(first, second);
        assert!((7337..=7528).contains(&first));
        assert!((7337..=7528).contains(&second));

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn attaches_lists_activates_and_detaches_workspace() {
        let base = temporary_root("registry");
        let project = base.join("project");
        let home = base.join("home");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("Cargo.toml"), "[workspace]\n").unwrap();
        let workspace = Workspace::open(&project).unwrap();
        let registry = WorkspaceRegistry::open(&home).unwrap();

        let attached = registry.attach(&workspace).unwrap();
        assert_eq!(registry.list().unwrap().len(), 1);
        assert_eq!(registry.active().unwrap().unwrap().id, attached.id);
        assert!(attached.state_root.starts_with(&home));
        assert!(attached
            .known_paths
            .iter()
            .any(|path| path == workspace.root()));
        assert_eq!(registry.set_active(&attached.id).unwrap().id, attached.id);
        assert!(registry.detach(&attached.id).unwrap());
        assert!(registry.list().unwrap().is_empty());

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn workspace_id_is_stable() {
        let path = Path::new("example/workspace");
        assert_eq!(workspace_id(path), workspace_id(path));
    }

    #[test]
    fn drive_root_layout_keeps_source_and_state_separate() {
        let base = temporary_root("portable-layout");
        let layout = CortexLibraryLayout::from_root(base.clone());
        assert_eq!(layout.projects, base.join("Source"));
        assert_eq!(layout.models, base.join("Models"));
        assert_eq!(layout.cortex_data, base.join(".cortex").join("data"));
        assert_ne!(layout.cortex_data, base.join("Cortex"));
        fs::remove_dir_all(base).ok();
    }
}
