//! Project-agnostic milestone-driven software-development authority for Cortex.
//!
//! The first LCD-clock proof is deliberately not represented here. It is a
//! certification fixture that exercises these generic contracts just like any
//! other registered project.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEVELOPMENT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    Rust,
    CMake,
    Node,
    DotNet,
    JavaGradle,
    Python,
    Open2D,
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QualityCapability {
    Format,
    Validate,
    Test,
    Lint,
    Build,
    Package,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCapability {
    Launch,
    ProcessStatus,
    NativeWindow,
    Capture,
    Vision,
    Stdout,
    ExitCode,
    ProjectSpecific,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointAuthorityKind {
    ProjectNative,
    Generated,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectCheckpointAuthority {
    pub schema_version: u32,
    pub kind: CheckpointAuthorityKind,
    pub label: String,
    pub program: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub log_path: Option<PathBuf>,
    #[serde(default)]
    pub protected_paths: Vec<PathBuf>,
    #[serde(default)]
    pub covers_runtime: bool,
}

impl Default for ProjectCheckpointAuthority {
    fn default() -> Self {
        Self {
            schema_version: DEVELOPMENT_SCHEMA_VERSION,
            kind: CheckpointAuthorityKind::Generated,
            label: "Cortex generated quality profile".into(),
            program: None,
            args: Vec::new(),
            log_path: None,
            protected_paths: Vec::new(),
            covers_runtime: false,
        }
    }
}

impl ProjectCheckpointAuthority {
    pub fn project_native(&self) -> bool {
        matches!(self.kind, CheckpointAuthorityKind::ProjectNative)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectProfile {
    pub schema_version: u32,
    pub kind: ProjectKind,
    pub root: PathBuf,
    pub markers: Vec<String>,
    pub quality_capabilities: Vec<QualityCapability>,
    pub runtime_capabilities: Vec<RuntimeCapability>,
    #[serde(default)]
    pub checkpoint_authority: ProjectCheckpointAuthority,
}

pub fn detect_project_checkpoint_authority(root: &Path) -> ProjectCheckpointAuthority {
    let config = root.join(".cortex").join("checkpoint.json");
    let configured_checkpoint = if config.is_file() {
        fs::read(&config)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| {
                let program = value.get("program").and_then(Value::as_str)?.to_string();
                Some((value, program))
            })
    } else {
        None
    };
    if let Some((value, program)) = configured_checkpoint {
        let args = value
            .get("args")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        return ProjectCheckpointAuthority {
            schema_version: DEVELOPMENT_SCHEMA_VERSION,
            kind: CheckpointAuthorityKind::ProjectNative,
            label: value
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or("Project checkpoint")
                .to_string(),
            program: Some(program),
            args,
            log_path: value
                .get("log_path")
                .and_then(Value::as_str)
                .map(PathBuf::from),
            protected_paths: vec![PathBuf::from(".cortex/checkpoint.json")],
            covers_runtime: value
                .get("covers_runtime")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
    }

    // A project's own PCC is the quality authority, even when it is not the
    // Universal-PCC Python module. Execute its anchored Python entry point with
    // a fixed `full` argument rather than passing model-controlled shell text.
    if root.join("PCC.cmd").is_file() && root.join("ProjectControlCenter.py").is_file() {
        let script = root.join("ProjectControlCenter.py");
        return ProjectCheckpointAuthority {
            schema_version: DEVELOPMENT_SCHEMA_VERSION,
            kind: CheckpointAuthorityKind::ProjectNative,
            label: "Project-local PCC FULL quality gate".into(),
            program: Some("python".into()),
            args: vec![script.to_string_lossy().to_string(), "full".into()],
            log_path: None,
            protected_paths: vec![
                PathBuf::from("PCC.cmd"),
                PathBuf::from("ProjectControlCenter.py"),
            ],
            covers_runtime: false,
        };
    }

    let open2d_checkpoint = root
        .join("scripts")
        .join("Invoke-Open2DCortexCheckpoint.ps1");
    if open2d_checkpoint.is_file() {
        return ProjectCheckpointAuthority {
            schema_version: DEVELOPMENT_SCHEMA_VERSION,
            kind: CheckpointAuthorityKind::ProjectNative,
            label: "Open2D Cortex checkpoint".into(),
            program: Some("powershell".into()),
            args: vec![
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                open2d_checkpoint.to_string_lossy().to_string(),
                "-RepoRoot".into(),
                root.to_string_lossy().to_string(),
            ],
            log_path: Some(PathBuf::from("logs/sessions/LATEST_CORTEX_CHECKPOINT.log")),
            protected_paths: vec![
                PathBuf::from("scripts/Invoke-Open2DCortexCheckpoint.ps1"),
                PathBuf::from("scripts/Test-Open2DCortexStatic.ps1"),
                PathBuf::from("Open2DTools.cmd"),
            ],
            covers_runtime: true,
        };
    }

    for relative in [
        "scripts/Invoke-ProjectCheckpoint.ps1",
        "scripts/Invoke-Checkpoint.ps1",
        "scripts/checkpoint.ps1",
        "checkpoint.ps1",
    ] {
        let path = root.join(relative);
        if path.is_file() {
            return ProjectCheckpointAuthority {
                schema_version: DEVELOPMENT_SCHEMA_VERSION,
                kind: CheckpointAuthorityKind::ProjectNative,
                label: "Project checkpoint".into(),
                program: Some("powershell".into()),
                args: vec![
                    "-NoProfile".into(),
                    "-ExecutionPolicy".into(),
                    "Bypass".into(),
                    "-File".into(),
                    path.to_string_lossy().to_string(),
                ],
                log_path: None,
                protected_paths: vec![PathBuf::from(relative)],
                covers_runtime: false,
            };
        }
    }

    ProjectCheckpointAuthority::default()
}

pub fn detect_project_profile(root: &Path) -> ProjectProfile {
    let mut markers = Vec::new();
    let kind = if root.join("open2d.project.json").is_file() {
        markers.push("open2d.project.json".into());
        ProjectKind::Open2D
    } else if root.join("Cargo.toml").is_file() {
        markers.push("Cargo.toml".into());
        ProjectKind::Rust
    } else if root.join("CMakeLists.txt").is_file() {
        markers.push("CMakeLists.txt".into());
        ProjectKind::CMake
    } else if root.join("package.json").is_file() {
        markers.push("package.json".into());
        ProjectKind::Node
    } else if first_extension(root, "sln").is_some() || first_extension(root, "csproj").is_some() {
        markers.push(".sln/.csproj".into());
        ProjectKind::DotNet
    } else if root.join("build.gradle").is_file()
        || root.join("build.gradle.kts").is_file()
        || root.join("gradlew").is_file()
        || root.join("gradlew.bat").is_file()
    {
        markers.push("Gradle".into());
        ProjectKind::JavaGradle
    } else if root.join("pyproject.toml").is_file() || root.join("requirements.txt").is_file() {
        markers.push("pyproject.toml/requirements.txt".into());
        ProjectKind::Python
    } else {
        ProjectKind::Unknown
    };

    let quality_capabilities = match kind {
        ProjectKind::Rust => vec![
            QualityCapability::Format,
            QualityCapability::Validate,
            QualityCapability::Test,
            QualityCapability::Lint,
            QualityCapability::Build,
        ],
        ProjectKind::CMake | ProjectKind::DotNet => vec![
            QualityCapability::Validate,
            QualityCapability::Test,
            QualityCapability::Build,
        ],
        ProjectKind::JavaGradle => vec![
            QualityCapability::Validate,
            QualityCapability::Test,
            QualityCapability::Lint,
            QualityCapability::Build,
        ],
        ProjectKind::Node => vec![
            QualityCapability::Validate,
            QualityCapability::Test,
            QualityCapability::Lint,
            QualityCapability::Build,
        ],
        ProjectKind::Python => vec![QualityCapability::Validate, QualityCapability::Test],
        ProjectKind::Open2D => vec![
            QualityCapability::Format,
            QualityCapability::Validate,
            QualityCapability::Test,
            QualityCapability::Lint,
            QualityCapability::Build,
            QualityCapability::Package,
        ],
        ProjectKind::Unknown => Vec::new(),
    };

    let runnable_root = match kind {
        ProjectKind::Rust => fs::read_to_string(root.join("Cargo.toml"))
            .map(|manifest| manifest.lines().any(|line| line.trim() == "[package]"))
            .unwrap_or(false),
        ProjectKind::Unknown => false,
        _ => true,
    };
    let mut runtime_capabilities = vec![
        RuntimeCapability::ProcessStatus,
        RuntimeCapability::ExitCode,
    ];
    if runnable_root {
        runtime_capabilities.insert(0, RuntimeCapability::Launch);
        if matches!(kind, ProjectKind::Rust | ProjectKind::CMake) {
            runtime_capabilities.push(RuntimeCapability::Stdout);
        }
    }

    ProjectProfile {
        schema_version: DEVELOPMENT_SCHEMA_VERSION,
        kind,
        root: root.to_path_buf(),
        markers,
        quality_capabilities,
        runtime_capabilities,
        checkpoint_authority: detect_project_checkpoint_authority(root),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcceptanceCriterion {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MilestoneStatus {
    Pending,
    Active,
    Blocked,
    Complete,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Milestone {
    pub id: String,
    pub title: String,
    pub status: MilestoneStatus,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<AcceptanceCriterion>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Roadmap {
    pub schema_version: u32,
    pub project_name: String,
    pub source: Option<PathBuf>,
    pub generated: bool,
    pub milestones: Vec<Milestone>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoadmapDiscovery {
    pub candidates: Vec<PathBuf>,
    pub selected: Option<PathBuf>,
    pub roadmap: Option<Roadmap>,
}

pub fn discover_roadmap(root: &Path) -> Result<RoadmapDiscovery, String> {
    let candidates = roadmap_candidates(root);
    let selected = candidates.first().cloned();
    let roadmap = selected
        .as_ref()
        .map(|path| parse_roadmap(root, path))
        .transpose()?;
    Ok(RoadmapDiscovery {
        candidates,
        selected,
        roadmap,
    })
}

fn roadmap_candidates(root: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for relative in [
        "roadmap.json",
        "milestones.json",
        "ROADMAP.md",
        "Roadmap.md",
        "roadmap.md",
        "docs/roadmap.json",
        "docs/milestones.json",
        "docs/ROADMAP.md",
        "docs/roadmap.md",
        "docs/current/roadmap.json",
        "docs/current/milestones.json",
        "docs/current/ROADMAP.md",
        "docs/current/roadmap.md",
    ] {
        let path = root.join(relative);
        if path.is_file() {
            candidates.push(path);
        }
    }
    let roadmap_dir = root.join("docs").join("roadmap");
    if let Ok(entries) = fs::read_dir(&roadmap_dir) {
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("md"))
            })
            .collect::<Vec<_>>();
        paths.sort();
        candidates.extend(paths);
    }
    candidates.dedup();
    candidates
}

fn parse_roadmap(root: &Path, path: &Path) -> Result<Roadmap, String> {
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("json"))
    {
        parse_roadmap_json(root, path)
    } else {
        parse_roadmap_markdown(root, path)
    }
}

fn parse_roadmap_json(root: &Path, path: &Path) -> Result<Roadmap, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("failed to read roadmap {}: {error}", path.display()))?;
    if let Ok(mut roadmap) = serde_json::from_slice::<Roadmap>(&bytes) {
        roadmap.source = Some(path.to_path_buf());
        roadmap.generated = false;
        normalize_roadmap_statuses(&mut roadmap);
        return Ok(roadmap);
    }

    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid roadmap JSON {}: {error}", path.display()))?;
    let project_name = value
        .get("project_name")
        .or_else(|| value.get("project"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            root.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("project")
                .to_string()
        });
    let milestone_values = value
        .get("milestones")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("roadmap JSON {} has no milestones array", path.display()))?;
    let mut milestones = Vec::new();
    for (index, item) in milestone_values.iter().enumerate() {
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("M{:02}", index + 1));
        let title = item
            .get("title")
            .or_else(|| item.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Milestone")
            .to_string();
        let dependencies = item
            .get("dependencies")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let acceptance = item
            .get("acceptance")
            .or_else(|| item.get("acceptance_criteria"))
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .enumerate()
                    .filter_map(|(criterion_index, criterion)| {
                        if let Some(description) = criterion.as_str() {
                            return Some(AcceptanceCriterion {
                                id: format!("{id}-A{}", criterion_index + 1),
                                description: description.to_string(),
                                required: true,
                            });
                        }
                        let description = criterion
                            .get("description")
                            .or_else(|| criterion.get("text"))
                            .and_then(Value::as_str)?;
                        Some(AcceptanceCriterion {
                            id: criterion
                                .get("id")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("{id}-A{}", criterion_index + 1)),
                            description: description.to_string(),
                            required: criterion
                                .get("required")
                                .and_then(Value::as_bool)
                                .unwrap_or(true),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let status = match item.get("status").and_then(Value::as_str) {
            Some("active") => MilestoneStatus::Active,
            Some("blocked") => MilestoneStatus::Blocked,
            Some("complete") | Some("completed") | Some("done") => MilestoneStatus::Complete,
            _ => MilestoneStatus::Pending,
        };
        milestones.push(Milestone {
            id,
            title,
            status,
            dependencies,
            acceptance,
            notes: Vec::new(),
        });
    }
    if milestones.is_empty() {
        return Err(format!(
            "roadmap JSON {} contains no milestones",
            path.display()
        ));
    }
    let mut roadmap = Roadmap {
        schema_version: DEVELOPMENT_SCHEMA_VERSION,
        project_name,
        source: Some(path.to_path_buf()),
        generated: false,
        milestones,
    };
    normalize_roadmap_statuses(&mut roadmap);
    Ok(roadmap)
}

fn normalize_roadmap_statuses(roadmap: &mut Roadmap) {
    if roadmap
        .milestones
        .iter()
        .any(|milestone| milestone.status == MilestoneStatus::Active)
    {
        return;
    }
    let complete = roadmap
        .milestones
        .iter()
        .filter(|milestone| milestone.status == MilestoneStatus::Complete)
        .map(|milestone| milestone.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    if let Some(next) = roadmap.milestones.iter_mut().find(|milestone| {
        milestone.status == MilestoneStatus::Pending
            && milestone
                .dependencies
                .iter()
                .all(|dependency| complete.contains(dependency))
    }) {
        next.status = MilestoneStatus::Active;
    }
}

fn parse_roadmap_markdown(root: &Path, path: &Path) -> Result<Roadmap, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read roadmap {}: {error}", path.display()))?;
    let project_name = root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("project")
        .to_string();
    let mut milestones = Vec::new();
    let mut current: Option<Milestone> = None;
    let mut sequence = 1usize;

    for raw in text.lines() {
        let line = raw.trim();
        let heading = line
            .strip_prefix("## ")
            .or_else(|| line.strip_prefix("### "));
        if let Some(title) = heading {
            if let Some(milestone) = current.take() {
                milestones.push(milestone);
            }
            let normalized = title.trim().trim_matches('#').trim();
            if normalized.is_empty() {
                continue;
            }
            let (id, title) = milestone_id_and_title(normalized, sequence);
            sequence += 1;
            current = Some(Milestone {
                id,
                title,
                status: MilestoneStatus::Pending,
                dependencies: Vec::new(),
                acceptance: Vec::new(),
                notes: Vec::new(),
            });
            continue;
        }

        if let Some(milestone) = current.as_mut() {
            if let Some(rest) = line
                .strip_prefix("- [x] ")
                .or_else(|| line.strip_prefix("- [X] "))
                .or_else(|| line.strip_prefix("- [ ] "))
            {
                milestone.acceptance.push(AcceptanceCriterion {
                    id: format!("{}-A{}", milestone.id, milestone.acceptance.len() + 1),
                    description: rest.trim().to_string(),
                    required: true,
                });
            } else if let Some(rest) = line.strip_prefix("- ") {
                if !rest.trim().is_empty() {
                    milestone.notes.push(rest.trim().to_string());
                }
            }
        }
    }
    if let Some(milestone) = current.take() {
        milestones.push(milestone);
    }

    if milestones.is_empty() {
        milestones.push(Milestone {
            id: "M01".into(),
            title: "Roadmap execution".into(),
            status: MilestoneStatus::Active,
            dependencies: Vec::new(),
            acceptance: vec![AcceptanceCriterion {
                id: "M01-A1".into(),
                description: "Satisfy the goals described by the authoritative roadmap".into(),
                required: true,
            }],
            notes: vec!["Roadmap headings were not parseable as individual milestones; preserve the source document as authority.".into()],
        });
    } else if let Some(first) = milestones.first_mut() {
        first.status = MilestoneStatus::Active;
    }

    Ok(Roadmap {
        schema_version: DEVELOPMENT_SCHEMA_VERSION,
        project_name,
        source: Some(path.to_path_buf()),
        generated: false,
        milestones,
    })
}

fn milestone_id_and_title(value: &str, sequence: usize) -> (String, String) {
    let mut parts = value.splitn(2, |character: char| {
        character == '—' || character == '-' || character == ':'
    });
    let first = parts.next().unwrap_or(value).trim();
    let second = parts.next().map(str::trim).filter(|part| !part.is_empty());
    let looks_like_id = first.len() <= 16
        && first.chars().any(|character| character.is_ascii_digit())
        && first.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '.' || character == '_'
        });
    if looks_like_id {
        (first.to_string(), second.unwrap_or(first).to_string())
    } else {
        (format!("M{sequence:02}"), value.trim().to_string())
    }
}

pub fn proposed_roadmap(project_name: &str) -> Roadmap {
    Roadmap {
        schema_version: DEVELOPMENT_SCHEMA_VERSION,
        project_name: project_name.to_string(),
        source: None,
        generated: true,
        milestones: vec![
            Milestone {
                id: "M01".into(),
                title: "Foundation and project health".into(),
                status: MilestoneStatus::Active,
                dependencies: Vec::new(),
                acceptance: vec![
                    criterion("M01-A1", "Project structure is understood and build authority is identified"),
                    criterion("M01-A2", "Baseline quality gate can be executed or missing prerequisites are reported"),
                ],
                notes: Vec::new(),
            },
            Milestone {
                id: "M02".into(),
                title: "Primary requested functionality".into(),
                status: MilestoneStatus::Pending,
                dependencies: vec!["M01".into()],
                acceptance: vec![criterion("M02-A1", "Requested functionality is implemented and verified")],
                notes: Vec::new(),
            },
            Milestone {
                id: "M03".into(),
                title: "Runtime and release certification".into(),
                status: MilestoneStatus::Pending,
                dependencies: vec!["M02".into()],
                acceptance: vec![criterion("M03-A1", "Project-specific runtime/release evidence is captured")],
                notes: Vec::new(),
            },
        ],
    }
}

fn criterion(id: &str, description: &str) -> AcceptanceCriterion {
    AcceptanceCriterion {
        id: id.into(),
        description: description.into(),
        required: true,
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DevelopmentStage {
    Understand,
    Inspect,
    Plan,
    Implement,
    Format,
    Validate,
    Test,
    Lint,
    Build,
    Diagnose,
    Repair,
    Launch,
    RuntimeVerify,
    Capture,
    Certify,
    Evidence,
    Complete,
    Blocked,
}

impl DevelopmentStage {
    pub fn next(self) -> Self {
        match self {
            Self::Understand => Self::Inspect,
            Self::Inspect => Self::Plan,
            Self::Plan => Self::Implement,
            Self::Implement => Self::Format,
            Self::Format => Self::Validate,
            Self::Validate => Self::Test,
            Self::Test => Self::Lint,
            Self::Lint => Self::Build,
            Self::Build => Self::Launch,
            Self::Diagnose => Self::Repair,
            Self::Repair => Self::Validate,
            Self::Launch => Self::RuntimeVerify,
            Self::RuntimeVerify => Self::Capture,
            Self::Capture => Self::Certify,
            Self::Certify => Self::Evidence,
            Self::Evidence => Self::Complete,
            Self::Complete | Self::Blocked => self,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepairBudget {
    pub used: u32,
    pub maximum: u32,
}

impl Default for RepairBudget {
    fn default() -> Self {
        Self {
            used: 0,
            maximum: 3,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DevelopmentEvidence {
    pub id: String,
    pub kind: String,
    pub description: String,
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub metadata: Value,
    pub created_unix_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DevelopmentRun {
    pub schema_version: u32,
    pub id: String,
    pub project_root: PathBuf,
    pub project_kind: ProjectKind,
    pub milestone_id: String,
    pub milestone_title: String,
    pub stage: DevelopmentStage,
    pub repair_budget: RepairBudget,
    pub started_unix_ms: u128,
    pub updated_unix_ms: u128,
    pub last_error: Option<String>,
    pub next_action: Option<String>,
    #[serde(default)]
    pub evidence: Vec<DevelopmentEvidence>,
    #[serde(default)]
    pub completed_stages: Vec<DevelopmentStage>,
}

impl DevelopmentRun {
    pub fn begin(project_root: &Path, profile: &ProjectProfile, milestone: &Milestone) -> Self {
        let now = unix_ms();
        Self {
            schema_version: DEVELOPMENT_SCHEMA_VERSION,
            id: format!("{now}-{}-development", std::process::id()),
            project_root: project_root.to_path_buf(),
            project_kind: profile.kind,
            milestone_id: milestone.id.clone(),
            milestone_title: milestone.title.clone(),
            stage: DevelopmentStage::Understand,
            repair_budget: RepairBudget::default(),
            started_unix_ms: now,
            updated_unix_ms: now,
            last_error: None,
            next_action: Some("Understand the active milestone and acceptance criteria".into()),
            evidence: Vec::new(),
            completed_stages: Vec::new(),
        }
    }

    pub fn advance(&mut self) {
        if !self.completed_stages.contains(&self.stage) {
            self.completed_stages.push(self.stage);
        }
        self.stage = self.stage.next();
        self.updated_unix_ms = unix_ms();
        self.next_action = Some(format!("Continue development stage: {:?}", self.stage));
    }

    pub fn record_failure(&mut self, error: impl Into<String>) -> bool {
        self.last_error = Some(error.into());
        self.updated_unix_ms = unix_ms();
        if self.repair_budget.used < self.repair_budget.maximum {
            self.repair_budget.used += 1;
            self.stage = DevelopmentStage::Diagnose;
            self.next_action = Some(format!(
                "Diagnose failure and perform repair attempt {} of {}",
                self.repair_budget.used, self.repair_budget.maximum
            ));
            true
        } else {
            self.stage = DevelopmentStage::Blocked;
            self.next_action =
                Some("Repair budget exhausted; report unresolved evidence to the user".into());
            false
        }
    }

    pub fn add_evidence(
        &mut self,
        kind: impl Into<String>,
        description: impl Into<String>,
        path: Option<PathBuf>,
        metadata: Value,
    ) -> String {
        let now = unix_ms();
        let id = format!("{now}-{}-evidence", self.evidence.len());
        self.evidence.push(DevelopmentEvidence {
            id: id.clone(),
            kind: kind.into(),
            description: description.into(),
            path,
            metadata,
            created_unix_ms: now,
        });
        self.updated_unix_ms = now;
        id
    }
}

pub struct DevelopmentStore {
    root: PathBuf,
}

impl DevelopmentStore {
    pub fn open(state_root: &Path) -> Result<Self, String> {
        let root = state_root.join("development");
        fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        Ok(Self { root })
    }

    pub fn save(&self, run: &DevelopmentRun) -> Result<(), String> {
        fs::write(
            self.root.join(format!("{}.json", run.id)),
            serde_json::to_vec_pretty(run).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        fs::write(self.root.join("active.txt"), run.id.as_bytes())
            .map_err(|error| error.to_string())
    }

    pub fn active(&self) -> Result<Option<DevelopmentRun>, String> {
        let active = self.root.join("active.txt");
        if !active.is_file() {
            return Ok(None);
        }
        let id = fs::read_to_string(&active).map_err(|error| error.to_string())?;
        let path = self.root.join(format!("{}.json", id.trim()));
        if !path.is_file() {
            return Ok(None);
        }
        serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    pub fn save_roadmap(&self, roadmap: &Roadmap) -> Result<(), String> {
        fs::write(
            self.root.join("roadmap.json"),
            serde_json::to_vec_pretty(roadmap).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())
    }

    pub fn roadmap(&self) -> Result<Option<Roadmap>, String> {
        let path = self.root.join("roadmap.json");
        if !path.is_file() {
            return Ok(None);
        }
        serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    pub fn complete_and_begin_next(
        &self,
        run: &DevelopmentRun,
    ) -> Result<Option<DevelopmentRun>, String> {
        if run.stage != DevelopmentStage::Complete {
            return Err(
                "development milestone cannot complete before the run reaches complete stage"
                    .into(),
            );
        }
        let mut roadmap = self
            .roadmap()?
            .ok_or_else(|| "development run has no persisted roadmap".to_string())?;
        let current = roadmap
            .milestones
            .iter_mut()
            .find(|milestone| milestone.id == run.milestone_id)
            .ok_or_else(|| {
                format!(
                    "milestone {} is missing from persisted roadmap",
                    run.milestone_id
                )
            })?;
        current.status = MilestoneStatus::Complete;

        let completed = roadmap
            .milestones
            .iter()
            .filter(|milestone| milestone.status == MilestoneStatus::Complete)
            .map(|milestone| milestone.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        for milestone in &mut roadmap.milestones {
            if milestone.status == MilestoneStatus::Active {
                milestone.status = MilestoneStatus::Pending;
            }
        }
        let next_index = roadmap.milestones.iter().position(|milestone| {
            milestone.status == MilestoneStatus::Pending
                && milestone
                    .dependencies
                    .iter()
                    .all(|dependency| completed.contains(dependency))
        });
        if let Some(index) = next_index {
            roadmap.milestones[index].status = MilestoneStatus::Active;
        }
        self.save_roadmap(&roadmap)?;

        let Some(index) = next_index else {
            let _ = fs::remove_file(self.root.join("active.txt"));
            return Ok(None);
        };
        let profile = detect_project_profile(&run.project_root);
        let next = DevelopmentRun::begin(&run.project_root, &profile, &roadmap.milestones[index]);
        self.save(&next)?;
        Ok(Some(next))
    }

    pub fn begin(&self, root: &Path, roadmap: &Roadmap) -> Result<DevelopmentRun, String> {
        self.save_roadmap(roadmap)?;
        let profile = detect_project_profile(root);
        let milestone = roadmap
            .milestones
            .iter()
            .find(|milestone| milestone.status == MilestoneStatus::Active)
            .or_else(|| {
                roadmap
                    .milestones
                    .iter()
                    .find(|milestone| milestone.status == MilestoneStatus::Pending)
            })
            .ok_or_else(|| "roadmap has no active or pending milestone".to_string())?;
        let run = DevelopmentRun::begin(root, &profile, milestone);
        self.save(&run)?;
        Ok(run)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CertificationRequirement {
    pub id: String,
    pub description: String,
    pub capability: RuntimeCapability,
    pub required: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CertificationPlan {
    pub schema_version: u32,
    pub milestone_id: String,
    pub requirements: Vec<CertificationRequirement>,
}

pub fn certification_plan(milestone: &Milestone, profile: &ProjectProfile) -> CertificationPlan {
    let mut requirements = vec![CertificationRequirement {
        id: "process".into(),
        description: "A requested runtime executable/process launches successfully".into(),
        capability: RuntimeCapability::Launch,
        required: true,
    }];
    if matches!(
        profile.kind,
        ProjectKind::Rust | ProjectKind::CMake | ProjectKind::DotNet | ProjectKind::Open2D
    ) {
        requirements.extend([
            CertificationRequirement {
                id: "window".into(),
                description:
                    "A responsive application window is discoverable when the project is graphical"
                        .into(),
                capability: RuntimeCapability::NativeWindow,
                required: false,
            },
            CertificationRequirement {
                id: "capture".into(),
                description:
                    "Runtime visual evidence can be captured when visual acceptance criteria apply"
                        .into(),
                capability: RuntimeCapability::Capture,
                required: false,
            },
        ]);
    }
    for criterion in &milestone.acceptance {
        requirements.push(CertificationRequirement {
            id: criterion.id.clone(),
            description: criterion.description.clone(),
            capability: RuntimeCapability::ProjectSpecific,
            required: criterion.required,
        });
    }
    CertificationPlan {
        schema_version: DEVELOPMENT_SCHEMA_VERSION,
        milestone_id: milestone.id.clone(),
        requirements,
    }
}

fn first_extension(root: &Path, extension: &str) -> Option<PathBuf> {
    fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        })
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

    #[test]
    fn proof_fixture_name_uses_the_same_generic_rust_detection() {
        let fixture_root = std::env::temp_dir().join(format!(
            "cortex-development-generic-project-detection-{}-{}",
            std::process::id(),
            unix_ms()
        ));
        let proof_project = fixture_root.join("hello3d");
        let generic_project = fixture_root.join("ordinary-rust-project");

        fs::create_dir_all(&proof_project).expect("create proof fixture directory");
        fs::create_dir_all(&generic_project).expect("create generic fixture directory");
        fs::write(
            proof_project.join("Cargo.toml"),
            "[package]\nname = \"hello3d\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("write proof fixture Cargo.toml");
        fs::write(
            generic_project.join("Cargo.toml"),
            "[package]\nname = \"ordinary-rust-project\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("write generic fixture Cargo.toml");

        let proof_profile = detect_project_profile(&proof_project);
        let generic_profile = detect_project_profile(&generic_project);

        assert_eq!(proof_profile.kind, ProjectKind::Rust);
        assert_eq!(proof_profile.kind, generic_profile.kind);
        assert_eq!(proof_profile.markers, generic_profile.markers);
        assert_eq!(
            proof_profile.quality_capabilities,
            generic_profile.quality_capabilities
        );
        assert_eq!(
            proof_profile.runtime_capabilities,
            generic_profile.runtime_capabilities
        );

        let _ = fs::remove_dir_all(fixture_root);
    }

    #[test]
    fn project_local_pcc_is_checkpoint_authority_without_universal_pcc_module() {
        let root = std::env::temp_dir().join(format!("cortex-w11-pcc-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("PCC.cmd"), "@echo off\n").unwrap();
        fs::write(root.join("ProjectControlCenter.py"), "# test fixture\n").unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname=\"fixture\"\n").unwrap();
        let authority = detect_project_checkpoint_authority(&root);
        assert!(authority.project_native());
        assert_eq!(authority.program.as_deref(), Some("python"));
        assert_eq!(authority.args.last().map(String::as_str), Some("full"));
        assert!(authority.args[0].ends_with("ProjectControlCenter.py"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_native_checkpoint_outranks_generated_quality_profile() {
        let root = std::env::temp_dir().join(format!(
            "cortex-checkpoint-authority-{}",
            std::process::id()
        ));
        let scripts = root.join("scripts");
        fs::create_dir_all(&scripts).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\n",
        )
        .unwrap();
        fs::write(scripts.join("Invoke-ProjectCheckpoint.ps1"), "exit 0\n").unwrap();
        let profile = detect_project_profile(&root);
        assert!(profile.checkpoint_authority.project_native());
        assert_eq!(
            profile.checkpoint_authority.program.as_deref(),
            Some("powershell")
        );
        assert!(profile
            .runtime_capabilities
            .contains(&RuntimeCapability::Launch));
        assert!(profile
            .runtime_capabilities
            .contains(&RuntimeCapability::Stdout));
        assert!(profile
            .runtime_capabilities
            .contains(&RuntimeCapability::ExitCode));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_only_cargo_root_is_not_claimed_as_runnable_binary() {
        let root =
            std::env::temp_dir().join(format!("cortex-workspace-runtime-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers=[]\n").unwrap();
        let profile = detect_project_profile(&root);
        assert!(!profile
            .runtime_capabilities
            .contains(&RuntimeCapability::Launch));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn repair_budget_is_bounded() {
        let profile = ProjectProfile {
            schema_version: 1,
            kind: ProjectKind::Rust,
            root: PathBuf::from("project"),
            markers: vec![],
            quality_capabilities: vec![],
            runtime_capabilities: vec![],
            checkpoint_authority: ProjectCheckpointAuthority {
                schema_version: 1,
                kind: CheckpointAuthorityKind::Generated,
                label: "generated".into(),
                program: None,
                args: vec![],
                log_path: None,
                protected_paths: vec![],
                covers_runtime: false,
            },
        };
        let milestone = Milestone {
            id: "M01".into(),
            title: "Fixture".into(),
            status: MilestoneStatus::Active,
            dependencies: vec![],
            acceptance: vec![],
            notes: vec![],
        };
        let mut run = DevelopmentRun::begin(Path::new("project"), &profile, &milestone);
        assert!(run.record_failure("one"));
        assert!(run.record_failure("two"));
        assert!(run.record_failure("three"));
        assert!(!run.record_failure("four"));
        assert_eq!(run.stage, DevelopmentStage::Blocked);
    }
}
// M11_UNIVERSAL_FOUNDATION_BRIDGE
// Additive M11A-T bridge. M10 remains the execution/lifecycle authority.
pub mod universal_m11 {
    pub use cortex_universal::*;
}
