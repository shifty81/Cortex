//! Generic workspace-safe source access, detection and reversible Cortex transactions.
//!
//! This crate has no Open2D requirement. Open2D is detected as one optional
//! workspace capability/adaptor alongside Git, Rust, Node, Python and CMake.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct Workspace {
    root: PathBuf,
    profile: WorkspaceProfile,
    state_root: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceProfile {
    pub root: PathBuf,
    pub name: String,
    pub kinds: Vec<WorkspaceKind>,
    pub build_systems: Vec<String>,
    pub adapters: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceInventory {
    pub schema_version: u32,
    pub root: PathBuf,
    pub name: String,
    pub scanned_unix_ms: u128,
    pub file_count: usize,
    pub total_bytes: u64,
    pub textual_files: usize,
    pub extension_counts: BTreeMap<String, usize>,
    pub manifests: Vec<PathBuf>,
    pub top_level_entries: Vec<PathBuf>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoundedTextRead {
    pub text: String,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub total_bytes: usize,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspacePathClass {
    AuthoritativeSource,
    Config,
    Asset,
    Generated,
    BuildOutput,
    Vendor,
    DependencyCache,
    Secret,
    InternalState,
}

impl WorkspacePathClass {
    pub fn autonomous_mutation_allowed(self) -> bool {
        matches!(
            self,
            Self::AuthoritativeSource | Self::Config | Self::Asset | Self::Generated
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogicalPathResolution {
    pub requested: PathBuf,
    pub relative: PathBuf,
    pub absolute: PathBuf,
    pub class: WorkspacePathClass,
    pub rewritten: bool,
    pub duplicate_tree_detected: bool,
    pub nested_project_detected: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceKind {
    Generic,
    Git,
    Rust,
    Node,
    Python,
    Cmake,
    Open2d,
}

impl Workspace {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let root = fs::canonicalize(root).map_err(WorkspaceError::Io)?;
        if !root.is_dir() {
            return Err(WorkspaceError::Invalid(format!(
                "workspace root is not a directory: {}",
                root.display()
            )));
        }
        let profile = detect_profile(&root);
        let state_root = default_global_state_root(&root)?;
        Ok(Self {
            root,
            profile,
            state_root,
        })
    }

    pub fn open_with_state_root(
        root: impl AsRef<Path>,
        state_root: impl Into<PathBuf>,
    ) -> Result<Self, WorkspaceError> {
        let root = fs::canonicalize(root).map_err(WorkspaceError::Io)?;
        if !root.is_dir() {
            return Err(WorkspaceError::Invalid(format!(
                "workspace root is not a directory: {}",
                root.display()
            )));
        }
        let profile = detect_profile(&root);
        Ok(Self {
            root,
            profile,
            state_root: state_root.into(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn profile(&self) -> &WorkspaceProfile {
        &self.profile
    }

    pub fn is_open2d(&self) -> bool {
        self.profile.kinds.contains(&WorkspaceKind::Open2d)
    }

    /// Cortex-owned durable state. The default is global/user-local and does
    /// not modify the attached repository. Registry-backed callers should
    /// reopen with `open_with_state_root` so a persistent WorkspaceId owns it.
    pub fn cortex_state_dir(&self) -> PathBuf {
        self.state_root.clone()
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.state_root.join("sessions")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.state_root.join("logs")
    }

    pub fn legacy_state_dir(&self) -> PathBuf {
        if self.is_open2d() {
            self.root.join(".open2d").join("cortex")
        } else {
            self.root.join(".cortex")
        }
    }

    pub fn migrate_legacy_state_to(&self, destination: &Path) -> Result<bool, WorkspaceError> {
        let source = self.legacy_state_dir();
        if !source.is_dir() || source == destination {
            return Ok(false);
        }
        fs::create_dir_all(destination).map_err(WorkspaceError::Io)?;
        copy_directory_missing_only(&source, destination)?;
        Ok(true)
    }

    pub fn context_dir(&self) -> PathBuf {
        self.cortex_state_dir().join("context")
    }

    pub fn inventory_path(&self) -> PathBuf {
        self.context_dir().join("workspace_inventory.json")
    }

    /// Build a deterministic metadata-only inventory for this workspace. The
    /// scanner never reads file bodies, so it is safe to run before a Vault
    /// rebuild and cheap enough to use as the first grounding step for an
    /// arbitrary attached folder.
    pub fn scan_inventory(&self, max_files: usize) -> Result<WorkspaceInventory, WorkspaceError> {
        let max_files = max_files.clamp(1, 250_000);
        let requested = max_files.saturating_add(1);
        let mut files = self.list_files("", requested)?;
        let truncated = files.len() > max_files;
        if truncated {
            files.truncate(max_files);
        }

        let mut total_bytes = 0_u64;
        let mut textual_files = 0_usize;
        let mut extension_counts = BTreeMap::<String, usize>::new();
        let mut manifests = Vec::<PathBuf>::new();

        for relative in &files {
            let absolute = self.resolve(relative)?;
            if let Ok(metadata) = fs::metadata(&absolute) {
                total_bytes = total_bytes.saturating_add(metadata.len());
            }
            if looks_textual(relative) {
                textual_files += 1;
            }
            let extension = relative
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| value.to_ascii_lowercase())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "<none>".into());
            *extension_counts.entry(extension).or_default() += 1;

            if is_workspace_manifest(relative) {
                manifests.push(relative.clone());
            }
        }
        manifests.sort();

        let mut top_level_entries = fs::read_dir(&self.root)
            .map_err(WorkspaceError::Io)?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_name()?.to_str()?;
                if path.is_dir() && is_ignored_dir(name) {
                    return None;
                }
                path.strip_prefix(&self.root).ok().map(Path::to_path_buf)
            })
            .collect::<Vec<_>>();
        top_level_entries.sort();
        top_level_entries.truncate(256);

        Ok(WorkspaceInventory {
            schema_version: 1,
            root: self.root.clone(),
            name: self.profile.name.clone(),
            scanned_unix_ms: unix_millis(),
            file_count: files.len(),
            total_bytes,
            textual_files,
            extension_counts,
            manifests,
            top_level_entries,
            truncated,
        })
    }

    pub fn save_inventory(
        &self,
        inventory: &WorkspaceInventory,
    ) -> Result<PathBuf, WorkspaceError> {
        let directory = self.context_dir();
        fs::create_dir_all(&directory).map_err(WorkspaceError::Io)?;
        let path = self.inventory_path();
        let bytes = serde_json::to_vec_pretty(inventory)
            .map_err(|error| WorkspaceError::Invalid(error.to_string()))?;
        fs::write(&path, bytes).map_err(WorkspaceError::Io)?;
        Ok(path)
    }

    pub fn load_inventory(&self) -> Result<Option<WorkspaceInventory>, WorkspaceError> {
        let path = self.inventory_path();
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(WorkspaceError::Io)?;
        serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            WorkspaceError::Invalid(format!(
                "invalid Cortex workspace inventory {}: {error}",
                path.display()
            ))
        })
    }

    pub fn rebuild_inventory(
        &self,
        max_files: usize,
    ) -> Result<WorkspaceInventory, WorkspaceError> {
        let inventory = self.scan_inventory(max_files)?;
        self.save_inventory(&inventory)?;
        Ok(inventory)
    }

    pub fn resolve(&self, requested: impl AsRef<Path>) -> Result<PathBuf, WorkspaceError> {
        let requested = requested.as_ref();
        if requested
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(WorkspaceError::UnsafePath(requested.display().to_string()));
        }

        // H68C canonical workspace authority: models/tools may hand back either a workspace-relative
        // path or an absolute Windows path (including the \\?\ extended form returned by
        // fs::canonicalize). Absolute paths are accepted only after the nearest existing ancestor
        // canonicalizes inside the authoritative workspace root.
        let candidate = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            if requested
                .components()
                .any(|part| matches!(part, Component::RootDir | Component::Prefix(_)))
            {
                return Err(WorkspaceError::UnsafePath(requested.display().to_string()));
            }
            self.root.join(requested)
        };
        ensure_within_workspace(&self.root, &candidate)?;
        Ok(candidate)
    }

    /// Resolve a tool/model path to one logical project-relative identity above ordinary
    /// filesystem containment. Repeated workspace-name prefixes are corrected only when the
    /// stripped path already exists in the authoritative root and the nested directory is not
    /// itself a registered project.
    pub fn resolve_logical_path(
        &self,
        requested: impl AsRef<Path>,
    ) -> Result<LogicalPathResolution, WorkspaceError> {
        let requested = requested.as_ref();
        let absolute = self.resolve(requested)?;
        let mut relative = if requested.is_absolute() {
            absolute
                .strip_prefix(&self.root)
                .map(Path::to_path_buf)
                .map_err(|_| WorkspaceError::UnsafePath(requested.display().to_string()))?
        } else {
            normalize_relative_path(requested)?
        };
        let mut rewritten = false;
        let mut duplicate_tree_detected = false;
        let mut nested_project_detected = false;
        let mut reason = None;
        let root_name = self
            .root
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or_default();
        let mut components = relative.components();
        let first = components.next().and_then(|part| match part {
            Component::Normal(value) => value.to_str().map(str::to_string),
            _ => None,
        });
        if first
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(root_name))
        {
            let nested_root = self.root.join(root_name);
            nested_project_detected = directory_has_project_manifest(&nested_root);
            let stripped = components.as_path().to_path_buf();
            let canonical_candidate = self.root.join(&stripped);
            let nested_candidate = self.root.join(&relative);
            if !nested_project_detected
                && !stripped.as_os_str().is_empty()
                && canonical_candidate.exists()
            {
                duplicate_tree_detected = nested_candidate.exists();
                relative = stripped;
                rewritten = true;
                reason = Some(if duplicate_tree_detected {
                    "repeated workspace-root segment resolved to the existing authoritative root-relative path; an unregistered duplicate tree also exists".into()
                } else {
                    "repeated workspace-root segment resolved to the existing authoritative root-relative path".into()
                });
            }
        }
        let absolute = self.root.join(&relative);
        ensure_within_workspace(&self.root, &absolute)?;
        let class = classify_workspace_path(&relative);
        Ok(LogicalPathResolution {
            requested: requested.to_path_buf(),
            relative,
            absolute,
            class,
            rewritten,
            duplicate_tree_detected,
            nested_project_detected,
            reason,
        })
    }

    pub fn read_text(
        &self,
        relative: impl AsRef<Path>,
        max_bytes: usize,
    ) -> Result<String, WorkspaceError> {
        let path = self.resolve(relative)?;
        let meta = fs::metadata(&path).map_err(WorkspaceError::Io)?;
        if meta.len() > max_bytes as u64 {
            return Err(WorkspaceError::Invalid(format!(
                "file exceeds read limit: {}",
                path.display()
            )));
        }
        fs::read_to_string(path).map_err(WorkspaceError::Io)
    }

    /// Read a bounded UTF-8 window without failing merely because the complete file is larger than
    /// the requested context budget. The returned next_offset lets Cortex continue mechanically.
    pub fn read_text_window(
        &self,
        requested: impl AsRef<Path>,
        offset: usize,
        max_bytes: usize,
    ) -> Result<BoundedTextRead, WorkspaceError> {
        let path = self.resolve(requested)?;
        let bytes = fs::read(&path).map_err(WorkspaceError::Io)?;
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            WorkspaceError::Invalid(format!(
                "workspace text file is not valid UTF-8: {}: {error}",
                path.display()
            ))
        })?;
        let total_bytes = text.len();
        let limit = max_bytes.clamp(1, 1024 * 1024);
        let mut start = offset.min(total_bytes);
        while start > 0 && !text.is_char_boundary(start) {
            start -= 1;
        }
        let mut end = start.saturating_add(limit).min(total_bytes);
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        let truncated = end < total_bytes;
        Ok(BoundedTextRead {
            text: text[start..end].to_string(),
            offset: start,
            next_offset: truncated.then_some(end),
            total_bytes,
            truncated,
        })
    }

    pub fn list_files(
        &self,
        relative: impl AsRef<Path>,
        max_files: usize,
    ) -> Result<Vec<PathBuf>, WorkspaceError> {
        let base = self.resolve(relative)?;
        let mut result = Vec::new();
        collect_files(&self.root, &base, max_files, &mut result)?;
        result.sort();
        Ok(result)
    }

    pub fn search_text(
        &self,
        query: &str,
        roots: &[PathBuf],
        max_hits: usize,
    ) -> Result<Vec<SearchHit>, WorkspaceError> {
        if query.is_empty() {
            return Err(WorkspaceError::Invalid("search query is empty".into()));
        }
        let mut hits = Vec::new();
        let search_roots = if roots.is_empty() {
            vec![PathBuf::new()]
        } else {
            roots.to_vec()
        };
        for relative_root in search_roots {
            for relative in self.list_files(relative_root, 20_000)? {
                if hits.len() >= max_hits {
                    break;
                }
                if !looks_textual(&relative) {
                    continue;
                }
                let absolute = self.resolve(&relative)?;
                let Ok(text) = fs::read_to_string(&absolute) else {
                    continue;
                };
                for (line_index, line) in text.lines().enumerate() {
                    if line.contains(query) {
                        hits.push(SearchHit {
                            path: relative.clone(),
                            line: line_index + 1,
                            preview: line.trim().chars().take(240).collect(),
                        });
                        if hits.len() >= max_hits {
                            break;
                        }
                    }
                }
            }
        }
        Ok(hits)
    }
}

fn detect_profile(root: &Path) -> WorkspaceProfile {
    let mut kinds = BTreeSet::new();
    let mut build_systems = BTreeSet::new();
    let mut adapters = BTreeSet::new();

    if root.join(".git").exists() {
        kinds.insert(WorkspaceKind::Git);
        adapters.insert("git".to_string());
    }
    if root.join("Cargo.toml").is_file() {
        kinds.insert(WorkspaceKind::Rust);
        build_systems.insert("cargo".to_string());
    }
    if root.join("package.json").is_file() {
        kinds.insert(WorkspaceKind::Node);
        build_systems.insert("node".to_string());
    }
    if root.join("pyproject.toml").is_file()
        || root.join("requirements.txt").is_file()
        || root.join("setup.py").is_file()
    {
        kinds.insert(WorkspaceKind::Python);
        build_systems.insert("python".to_string());
    }
    if root.join("CMakeLists.txt").is_file() {
        kinds.insert(WorkspaceKind::Cmake);
        build_systems.insert("cmake".to_string());
    }

    let open2d = root
        .join("config")
        .join("architecture")
        .join("foundry_dependency_policy.json")
        .is_file()
        || root.join("apps").join("open2d_foundry").is_dir();
    if open2d {
        kinds.insert(WorkspaceKind::Open2d);
        adapters.insert("open2d".to_string());
    }

    if kinds.is_empty() {
        kinds.insert(WorkspaceKind::Generic);
    }

    WorkspaceProfile {
        root: root.to_path_buf(),
        name: root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("workspace")
            .to_string(),
        kinds: kinds.into_iter().collect(),
        build_systems: build_systems.into_iter().collect(),
        adapters: adapters.into_iter().collect(),
    }
}

pub fn discover_cortex_home() -> Result<PathBuf, WorkspaceError> {
    if let Some(path) = std::env::var_os("CORTEX_HOME").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    // A governed Vault root implies portable state unless an explicit CORTEX_HOME
    // overrides it. This keeps registry/chat/task state on the removable project
    // drive even when Cortex is launched by a surface that only propagated the
    // Vault authority.
    if let Some(path) = std::env::var_os("CORTEX_VAULT_ROOT").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path).join(".cortex").join("home"));
    }
    #[cfg(windows)]
    {
        if let Some(path) = std::env::var_os("LOCALAPPDATA") {
            return Ok(PathBuf::from(path).join("Cortex"));
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
            return Ok(PathBuf::from(path).join("cortex"));
        }
        if let Some(path) = std::env::var_os("HOME") {
            return Ok(PathBuf::from(path)
                .join(".local")
                .join("share")
                .join("cortex"));
        }
    }
    Err(WorkspaceError::Invalid(
        "Cortex home could not be resolved; set CORTEX_HOME explicitly".into(),
    ))
}

fn default_global_state_root(root: &Path) -> Result<PathBuf, WorkspaceError> {
    let portable = std::env::var("CORTEX_STATE_MODE")
        .ok()
        .is_some_and(|value| value.eq_ignore_ascii_case("portable"));
    if portable {
        return Ok(
            if detect_profile(root).kinds.contains(&WorkspaceKind::Open2d) {
                root.join(".open2d").join("cortex")
            } else {
                root.join(".cortex")
            },
        );
    }
    Ok(discover_cortex_home()?
        .join("workspaces")
        .join(format!("path-{:016x}", fnv_path(root))))
}

fn fnv_path(path: &Path) -> u64 {
    let text = path.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let text = text.to_ascii_lowercase();
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn copy_directory_missing_only(source: &Path, destination: &Path) -> Result<(), WorkspaceError> {
    for entry in fs::read_dir(source).map_err(WorkspaceError::Io)? {
        let entry = entry.map_err(WorkspaceError::Io)?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            fs::create_dir_all(&destination_path).map_err(WorkspaceError::Io)?;
            copy_directory_missing_only(&source_path, &destination_path)?;
        } else if source_path.is_file() && !destination_path.exists() {
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent).map_err(WorkspaceError::Io)?;
            }
            fs::copy(&source_path, &destination_path).map_err(WorkspaceError::Io)?;
        }
    }
    Ok(())
}

fn normalize_relative_path(path: &Path) -> Result<PathBuf, WorkspaceError> {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::Normal(value) => result.push(value),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(WorkspaceError::UnsafePath(path.display().to_string()));
            }
        }
    }
    Ok(result)
}

fn directory_has_project_manifest(directory: &Path) -> bool {
    if !directory.is_dir() {
        return false;
    }
    const NAMES: &[&str] = &[
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "CMakeLists.txt",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "go.mod",
    ];
    if NAMES.iter().any(|name| directory.join(name).is_file()) {
        return true;
    }
    fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .any(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".sln") || lower.ends_with(".vcxproj")
        })
}

fn classify_workspace_path(path: &Path) -> WorkspacePathClass {
    let parts = path
        .components()
        .filter_map(|part| match part {
            Component::Normal(v) => v.to_str().map(|s| s.to_ascii_lowercase()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let first = parts.first().map(String::as_str).unwrap_or_default();
    let file = parts.last().map(String::as_str).unwrap_or_default();
    if matches!(first, ".git" | ".cortex" | ".open2d") {
        return WorkspacePathClass::InternalState;
    }
    if matches!(
        first,
        "target" | "build" | "builds" | "dist" | "out" | "bin" | "obj"
    ) {
        return WorkspacePathClass::BuildOutput;
    }
    if matches!(first, "node_modules" | ".cargo" | ".venv" | "venv") {
        return WorkspacePathClass::DependencyCache;
    }
    if matches!(first, "vendor" | "third_party" | "third-party" | "external") {
        return WorkspacePathClass::Vendor;
    }
    if matches!(first, "logs" | "artifacts" | "generated" | "gen") {
        return WorkspacePathClass::Generated;
    }
    if file == ".env"
        || file.starts_with(".env.")
        || file.ends_with(".pem")
        || file.ends_with(".key")
        || file.ends_with(".pfx")
        || file.ends_with(".p12")
    {
        return WorkspacePathClass::Secret;
    }
    if is_workspace_manifest(path)
        || matches!(
            file,
            "package-lock.json"
                | "pnpm-lock.yaml"
                | "yarn.lock"
                | "vcpkg.json"
                | "conanfile.txt"
                | "conanfile.py"
        )
    {
        return WorkspacePathClass::Config;
    }
    if looks_textual(path) {
        WorkspacePathClass::AuthoritativeSource
    } else {
        WorkspacePathClass::Asset
    }
}

fn is_workspace_manifest(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        file_name.as_str(),
        "cargo.toml"
            | "cargo.lock"
            | "package.json"
            | "pyproject.toml"
            | "requirements.txt"
            | "setup.py"
            | "cmakelists.txt"
            | "makefile"
            | "meson.build"
            | "build.gradle"
            | "build.gradle.kts"
            | "pom.xml"
            | "go.mod"
            | "go.sum"
            | "source_manifest.json"
    ) || file_name.ends_with(".sln")
        || file_name.ends_with(".vcxproj")
}

fn collect_files(
    root: &Path,
    dir: &Path,
    max_files: usize,
    result: &mut Vec<PathBuf>,
) -> Result<(), WorkspaceError> {
    if result.len() >= max_files || !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(WorkspaceError::Io)? {
        let entry = entry.map_err(WorkspaceError::Io)?;
        let file_type = entry.file_type().map_err(WorkspaceError::Io)?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        let canonical = match fs::canonicalize(&path) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if !canonical.starts_with(root) {
            continue;
        }
        if path
            .file_name()
            .and_then(|value| value.to_str())
            .map(is_ignored_dir)
            .unwrap_or(false)
            && file_type.is_dir()
        {
            continue;
        }
        if file_type.is_dir() {
            collect_files(root, &path, max_files, result)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| WorkspaceError::UnsafePath(path.display().to_string()))?;
            result.push(relative.to_path_buf());
            if result.len() >= max_files {
                break;
            }
        }
    }
    Ok(())
}

fn ensure_within_workspace(root: &Path, candidate: &Path) -> Result<(), WorkspaceError> {
    let mut probe = candidate;
    loop {
        if probe.exists() {
            let canonical = fs::canonicalize(probe).map_err(WorkspaceError::Io)?;
            if !canonical.starts_with(root) {
                return Err(WorkspaceError::UnsafePath(candidate.display().to_string()));
            }
            return Ok(());
        }
        probe = probe
            .parent()
            .ok_or_else(|| WorkspaceError::UnsafePath(candidate.display().to_string()))?;
    }
}

fn is_ignored_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".cortex"
            | ".open2d"
            | "target"
            | "node_modules"
            | "build"
            | "builds"
            | "dist"
            | "logs"
            | "__pycache__"
            | ".venv"
            | "venv"
    )
}

fn looks_textual(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "rs" | "toml"
            | "json"
            | "md"
            | "txt"
            | "cmd"
            | "bat"
            | "ps1"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "c"
            | "cc"
            | "cpp"
            | "cxx"
            | "h"
            | "hpp"
            | "cs"
            | "java"
            | "kt"
            | "go"
            | "yml"
            | "yaml"
            | "ron"
            | "xml"
            | "html"
            | "css"
            | "scss"
            | "sh"
    )
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchHit {
    pub path: PathBuf,
    pub line: usize,
    pub preview: String,
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[derive(Debug)]
pub enum WorkspaceError {
    Io(io::Error),
    UnsafePath(String),
    Invalid(String),
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::UnsafePath(path) => write!(formatter, "unsafe workspace path: {path}"),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for WorkspaceError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_workspace(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "cortex-workspace-test-{}-{}-{label}",
            std::process::id(),
            unix_millis()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn arbitrary_folder_is_a_valid_workspace() {
        let root = temporary_workspace("generic");
        let workspace = Workspace::open(&root).unwrap();
        assert!(workspace.profile().kinds.contains(&WorkspaceKind::Generic));
        let canonical_root = fs::canonicalize(&root).unwrap();
        let portable = std::env::var("CORTEX_STATE_MODE")
            .ok()
            .is_some_and(|value| value.eq_ignore_ascii_case("portable"));
        if portable {
            assert_eq!(workspace.cortex_state_dir(), canonical_root.join(".cortex"));
        } else {
            assert!(!workspace.cortex_state_dir().starts_with(&canonical_root));
            assert!(workspace
                .cortex_state_dir()
                .components()
                .any(|component| component.as_os_str() == std::ffi::OsStr::new("workspaces")));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rust_workspace_is_detected_without_becoming_required() {
        let root = temporary_workspace("rust");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        let workspace = Workspace::open(&root).unwrap();
        assert!(workspace.profile().kinds.contains(&WorkspaceKind::Rust));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inventory_is_metadata_only_and_persistent() {
        let root = temporary_workspace("inventory");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src").join("lib.rs"),
            "pub fn value() -> u32 { 7 }\n",
        )
        .unwrap();
        let workspace = Workspace::open(&root).unwrap();
        let inventory = workspace.rebuild_inventory(100).unwrap();
        assert_eq!(inventory.file_count, 2);
        assert_eq!(inventory.textual_files, 2);
        assert!(inventory
            .manifests
            .iter()
            .any(|path| path == Path::new("Cargo.toml")));
        assert!(workspace.inventory_path().is_file());
        assert_eq!(workspace.load_inventory().unwrap().unwrap().file_count, 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parent_paths_are_rejected() {
        let root = temporary_workspace("unsafe");
        let workspace = Workspace::open(&root).unwrap();
        assert!(workspace.resolve("../outside").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn absolute_paths_inside_workspace_resolve_to_the_same_authority() {
        let root = temporary_workspace("absolute-in-root");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let workspace = Workspace::open(&root).unwrap();
        let absolute = fs::canonicalize(root.join("src/main.rs")).unwrap();
        assert_eq!(
            fs::canonicalize(workspace.resolve(&absolute).unwrap()).unwrap(),
            absolute
        );
        let outside = temporary_workspace("absolute-outside");
        fs::write(outside.join("outside.rs"), "outside").unwrap();
        assert!(workspace.resolve(outside.join("outside.rs")).is_err());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn repeated_workspace_name_prefers_authoritative_root_source() {
        let root = temporary_workspace("logical-root");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let workspace = Workspace::open(&root).unwrap();
        let repeated = workspace
            .root()
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap()
            .to_string();
        fs::create_dir_all(root.join(&repeated).join("src")).unwrap();
        fs::write(root.join(&repeated).join("src/main.rs"), "fn stale() {}\n").unwrap();
        let resolution = workspace
            .resolve_logical_path(PathBuf::from(&repeated).join("src/main.rs"))
            .unwrap();
        assert_eq!(resolution.relative, Path::new("src/main.rs"));
        assert!(resolution.rewritten && resolution.duplicate_tree_detected);
        assert!(!resolution.nested_project_detected);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registered_same_name_nested_project_is_preserved() {
        let root = temporary_workspace("logical-nested");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        let workspace = Workspace::open(&root).unwrap();
        let repeated = workspace
            .root()
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap()
            .to_string();
        let nested = root.join(&repeated);
        fs::create_dir_all(nested.join("src")).unwrap();
        fs::write(
            nested.join("Cargo.toml"),
            "[package]\nname=\"nested\"\nversion=\"0.1.0\"\n",
        )
        .unwrap();
        fs::write(nested.join("src/main.rs"), "fn main() {}\n").unwrap();
        let resolution = workspace
            .resolve_logical_path(PathBuf::from(&repeated).join("src/main.rs"))
            .unwrap();
        assert_eq!(
            resolution.relative,
            PathBuf::from(&repeated).join("src/main.rs")
        );
        assert!(!resolution.rewritten && resolution.nested_project_detected);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protected_paths_are_not_autonomous_source_targets() {
        assert!(!classify_workspace_path(Path::new("target/debug/app.exe"))
            .autonomous_mutation_allowed());
        assert!(
            !classify_workspace_path(Path::new("node_modules/pkg/index.js"))
                .autonomous_mutation_allowed()
        );
        assert!(classify_workspace_path(Path::new("src/main.rs")).autonomous_mutation_allowed());
    }

    #[test]
    fn bounded_text_window_returns_continuation_instead_of_large_file_failure() {
        let root = temporary_workspace("bounded-read");
        fs::write(root.join("large.rs"), "0123456789abcdef\n").unwrap();
        let workspace = Workspace::open(&root).unwrap();
        let first = workspace.read_text_window("large.rs", 0, 8).unwrap();
        assert_eq!(first.text, "01234567");
        assert_eq!(first.next_offset, Some(8));
        assert!(first.truncated);
        let second = workspace
            .read_text_window("large.rs", first.next_offset.unwrap(), 64)
            .unwrap();
        assert_eq!(second.text, "89abcdef\n");
        assert!(!second.truncated);
        assert_eq!(second.next_offset, None);
        fs::remove_dir_all(root).unwrap();
    }
}
