//! M11 universal Cortex agent foundation.
//!
//! This crate is intentionally project-agnostic and additive.  It provides the
//! common contracts needed by Cortex development, tooling, providers, media,
//! and future protocol adapters without replacing the M10 ExecutionSession
//! authority.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub const M11_SCHEMA_VERSION: u32 = 1;
pub const M11_PASS_MARKERS: [&str; 20] = [
    "M11A_PROJECT_PROFILE",
    "M11B_TOOLCHAIN_DISCOVERY",
    "M11C_QUALITY_GATE_PLAN",
    "M11D_RUNTIME_ARTIFACT",
    "M11E_GENERIC_COMMAND_PROFILE",
    "M11F_EVIDENCE_BROKER",
    "M11G_CAPABILITY_VIEW",
    "M11H_MODEL_CAPABILITY_PROFILE",
    "M11I_TASK_CAPSULE",
    "M11J_REPAIR_STRATEGY",
    "M11K_CODE_INTELLIGENCE",
    "M11L_ARCHITECT_EDITOR_VERIFIER",
    "M11M_SKILL_CONTRACT",
    "M11N_MCP_A2A_CONTRACT",
    "M11O_IMAGE_RECIPE",
    "M11P_WORKFLOW_PROFILE",
    "M11Q_IMAGE_COMPATIBILITY",
    "M11R_VISION_OUTPUT_CONTRACT",
    "M11S_RESOURCE_LEASE",
    "M11T_EVAL_TELEMETRY",
];

// -------------------------------------------------------------------------
// M11A — Universal ProjectProfile
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum LanguageKind {
    Rust,
    C,
    Cpp,
    CSharp,
    Python,
    JavaScript,
    TypeScript,
    Java,
    Kotlin,
    Go,
    Lua,
    Shell,
    Unknown(String),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum BuildSystemKind {
    Cargo,
    CMake,
    MsBuild,
    DotNet,
    Node,
    Python,
    Go,
    Maven,
    Gradle,
    Make,
    Ninja,
    DirectCompiler,
    GenericCommands,
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    Workspace,
    Application,
    Library,
    Script,
    SingleFile,
    Composite,
    Unknown,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectCapabilities {
    pub format: bool,
    pub lint: bool,
    pub validate: bool,
    pub build: bool,
    pub test: bool,
    pub run: bool,
    pub debug: bool,
    pub package: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectProfile {
    pub schema_version: u32,
    pub root: PathBuf,
    pub kind: ProjectKind,
    pub languages: Vec<LanguageKind>,
    pub build_systems: Vec<BuildSystemKind>,
    pub manifests: Vec<PathBuf>,
    pub entry_points: Vec<PathBuf>,
    pub capabilities: ProjectCapabilities,
    pub confidence: u8,
}

pub fn detect_project_profile(root: &Path) -> ProjectProfile {
    let mut languages = BTreeSet::new();
    let mut manifests = Vec::new();
    let mut entries = Vec::new();
    let mut build_systems = BTreeSet::new();
    let mut source_count = 0usize;

    scan_shallow(root, 4, &mut |absolute, relative| {
        if absolute.is_file() {
            if let Some(language) = language_for_path(relative) {
                source_count = source_count.saturating_add(1);
                languages.insert(language);
            }
            if is_entry_point(relative) {
                entries.push(relative.to_path_buf());
            }
            if let Some(build) = build_system_for_manifest(relative) {
                manifests.push(relative.to_path_buf());
                build_systems.insert(build);
            }
        }
    });

    let generic_profile = root.join(".cortex").join("project.json");
    if generic_profile.is_file() {
        manifests.push(PathBuf::from(".cortex/project.json"));
        build_systems.insert(BuildSystemKind::GenericCommands);
    }

    if build_systems.is_empty() && languages.contains(&LanguageKind::Cpp) {
        build_systems.insert(BuildSystemKind::DirectCompiler);
    }
    if build_systems.is_empty() && languages.contains(&LanguageKind::C) {
        build_systems.insert(BuildSystemKind::DirectCompiler);
    }

    let languages = languages.into_iter().collect::<Vec<_>>();
    let build_systems = build_systems.into_iter().collect::<Vec<_>>();
    entries.sort();
    entries.dedup();
    manifests.sort();
    manifests.dedup();

    let kind = if build_systems.len() > 1 || languages.len() > 1 {
        ProjectKind::Composite
    } else if matches!(
        build_systems.first(),
        Some(&BuildSystemKind::Python) | Some(&BuildSystemKind::Node)
    ) {
        ProjectKind::Script
    } else if source_count == 1 && build_systems.contains(&BuildSystemKind::DirectCompiler) {
        ProjectKind::SingleFile
    } else if !entries.is_empty() {
        ProjectKind::Application
    } else if source_count > 0 {
        ProjectKind::Library
    } else {
        ProjectKind::Unknown
    };

    let capabilities = capabilities_for(&build_systems, &languages);
    let confidence = if !manifests.is_empty() {
        95
    } else if !languages.is_empty() {
        75
    } else {
        20
    };

    ProjectProfile {
        schema_version: M11_SCHEMA_VERSION,
        root: root.to_path_buf(),
        kind,
        languages,
        build_systems,
        manifests,
        entry_points: entries,
        capabilities,
        confidence,
    }
}

// -------------------------------------------------------------------------
// M11B — ToolchainDiscovery
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolchainRecord {
    pub id: String,
    pub executable: PathBuf,
    pub version_hint: Option<String>,
    pub activation_script: Option<PathBuf>,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolchainCatalog {
    pub schema_version: u32,
    pub discovered_unix_ms: u128,
    pub tools: BTreeMap<String, ToolchainRecord>,
}

impl ToolchainCatalog {
    pub fn discover() -> Self {
        let mut catalog = ToolchainCatalog {
            schema_version: M11_SCHEMA_VERSION,
            discovered_unix_ms: unix_ms(),
            tools: BTreeMap::new(),
        };
        for (id, names, caps) in [
            (
                "cargo",
                &["cargo.exe", "cargo"][..],
                &["rust", "build", "test"][..],
            ),
            (
                "rustc",
                &["rustc.exe", "rustc"][..],
                &["rust", "compile"][..],
            ),
            (
                "cmake",
                &["cmake.exe", "cmake"][..],
                &["cpp", "configure", "build"][..],
            ),
            ("ninja", &["ninja.exe", "ninja"][..], &["build"][..]),
            (
                "clang++",
                &["clang++.exe", "clang++"][..],
                &["cpp", "compile"][..],
            ),
            ("clang", &["clang.exe", "clang"][..], &["c", "compile"][..]),
            ("g++", &["g++.exe", "g++"][..], &["cpp", "compile"][..]),
            ("gcc", &["gcc.exe", "gcc"][..], &["c", "compile"][..]),
            (
                "msbuild",
                &["MSBuild.exe", "msbuild.exe", "msbuild"][..],
                &["cpp", "dotnet", "build"][..],
            ),
            (
                "dotnet",
                &["dotnet.exe", "dotnet"][..],
                &["dotnet", "build", "test", "run"][..],
            ),
            (
                "python",
                &["python.exe", "python3", "python"][..],
                &["python", "run"][..],
            ),
            (
                "node",
                &["node.exe", "node"][..],
                &["javascript", "typescript", "run"][..],
            ),
            (
                "npm",
                &["npm.cmd", "npm.exe", "npm"][..],
                &["javascript", "typescript", "build", "test"][..],
            ),
            (
                "go",
                &["go.exe", "go"][..],
                &["go", "build", "test", "run"][..],
            ),
            ("java", &["java.exe", "java"][..], &["java", "run"][..]),
            (
                "javac",
                &["javac.exe", "javac"][..],
                &["java", "compile"][..],
            ),
            (
                "make",
                &["make.exe", "mingw32-make.exe", "make"][..],
                &["build"][..],
            ),
            (
                "mvn",
                &["mvn.cmd", "mvn.exe", "mvn"][..],
                &["java", "build", "test"][..],
            ),
            (
                "gradle",
                &["gradle.bat", "gradle.exe", "gradle"][..],
                &["java", "kotlin", "build", "test"][..],
            ),
            (
                "tsc",
                &["tsc.cmd", "tsc.exe", "tsc"][..],
                &["typescript", "validate"][..],
            ),
            (
                "pytest",
                &["pytest.exe", "pytest"][..],
                &["python", "test"][..],
            ),
            (
                "pyright",
                &["pyright.cmd", "pyright.exe", "pyright"][..],
                &["python", "validate"][..],
            ),
        ] {
            if let Some(path) = find_on_path(names) {
                catalog.tools.insert(
                    id.into(),
                    ToolchainRecord {
                        id: id.into(),
                        executable: path,
                        version_hint: None,
                        activation_script: None,
                        capabilities: caps.iter().map(|value| (*value).to_string()).collect(),
                    },
                );
            }
        }
        if cfg!(windows) && !catalog.tools.contains_key("msvc") {
            if let Some(msvc) = discover_msvc() {
                catalog.tools.insert("msvc".into(), msvc);
            }
        }
        catalog
    }

    pub fn has(&self, id: &str) -> bool {
        self.tools.contains_key(id)
    }

    pub fn get(&self, id: &str) -> Option<&ToolchainRecord> {
        self.tools.get(id)
    }
}

// -------------------------------------------------------------------------
// M11C — Capability-based QualityGatePlan
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GateCapability {
    Format,
    Lint,
    Validate,
    Build,
    Test,
    Launch,
    RuntimeVerify,
    ImageValidate,
    VisionReview,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QualityGateStep {
    pub capability: GateCapability,
    pub required: bool,
    pub command: Option<CommandSpec>,
    pub expected_artifact: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QualityGatePlan {
    pub schema_version: u32,
    pub project_root: PathBuf,
    pub steps: Vec<QualityGateStep>,
    pub diagnostics: Vec<String>,
}

pub fn plan_quality_gate(
    profile: &ProjectProfile,
    tools: &ToolchainCatalog,
    runtime_required: bool,
) -> QualityGatePlan {
    if let Ok(Some(custom)) = load_generic_command_profile(&profile.root) {
        return custom.to_quality_gate_plan(&profile.root, runtime_required);
    }

    if profile.build_systems.contains(&BuildSystemKind::Cargo) {
        return cargo_quality_gate(profile, tools, runtime_required);
    }
    if profile.build_systems.contains(&BuildSystemKind::CMake) {
        return cmake_quality_gate(profile, tools, runtime_required);
    }
    if profile.build_systems.contains(&BuildSystemKind::MsBuild) {
        return msbuild_quality_gate(profile, tools, runtime_required);
    }
    if profile.build_systems.contains(&BuildSystemKind::DotNet) {
        return dotnet_quality_gate(profile, tools, runtime_required);
    }
    if profile
        .build_systems
        .contains(&BuildSystemKind::DirectCompiler)
        && (profile.languages.contains(&LanguageKind::Cpp)
            || profile.languages.contains(&LanguageKind::C))
    {
        return direct_cpp_quality_gate(profile, tools, runtime_required);
    }
    if profile.build_systems.contains(&BuildSystemKind::Go) {
        return simple_tool_gate(
            profile,
            tools,
            "go",
            &["test", "./..."],
            &["build", "./..."],
            runtime_required,
        );
    }
    if profile.build_systems.contains(&BuildSystemKind::Python) {
        return python_quality_gate(profile, tools, runtime_required);
    }
    if profile.build_systems.contains(&BuildSystemKind::Node) {
        return node_quality_gate(profile, tools, runtime_required);
    }
    if profile.build_systems.contains(&BuildSystemKind::Maven) {
        return maven_quality_gate(profile, tools, runtime_required);
    }
    if profile.build_systems.contains(&BuildSystemKind::Gradle) {
        return gradle_quality_gate(profile, tools, runtime_required);
    }
    if profile.build_systems.contains(&BuildSystemKind::Make) {
        return make_quality_gate(profile, tools, runtime_required);
    }
    if profile.build_systems.contains(&BuildSystemKind::Ninja) {
        return ninja_quality_gate(profile, tools, runtime_required);
    }

    QualityGatePlan {
        schema_version: M11_SCHEMA_VERSION,
        project_root: profile.root.clone(),
        steps: Vec::new(),
        diagnostics: vec!["No executable quality-gate adapter could be synthesized from the detected project profile.".into()],
    }
}

// -------------------------------------------------------------------------
// M11D — RuntimeArtifact resolution
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeArtifact {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub expected_window: bool,
    pub evidence: String,
}

pub fn resolve_runtime_artifact(
    profile: &ProjectProfile,
    plan: &QualityGatePlan,
) -> Option<RuntimeArtifact> {
    if let Ok(Some(custom)) = load_generic_command_profile(&profile.root) {
        if let Some(run) = custom.run {
            if let Some((program, args)) = run.split_first() {
                return Some(RuntimeArtifact {
                    program: PathBuf::from(program),
                    args: args.to_vec(),
                    cwd: profile.root.clone(),
                    env: BTreeMap::new(),
                    expected_window: custom.expected_window.unwrap_or(false),
                    evidence: "generic command profile".into(),
                });
            }
        }
    }

    if let Some(artifact) = plan
        .steps
        .iter()
        .rev()
        .find_map(|step| step.expected_artifact.clone())
    {
        return Some(RuntimeArtifact {
            program: profile.root.join(&artifact),
            args: Vec::new(),
            cwd: profile.root.clone(),
            env: BTreeMap::new(),
            // A build artifact is not automatically a GUI application. Explicit
            // project command profiles carry expected_window when runtime evidence
            // requires a native window.
            expected_window: false,
            evidence: "quality-gate expected artifact".into(),
        });
    }

    if profile.build_systems.contains(&BuildSystemKind::CMake) {
        if let Some(program) = find_cmake_runtime_binary(&profile.root) {
            return Some(RuntimeArtifact {
                program,
                args: Vec::new(),
                cwd: profile.root.clone(),
                env: BTreeMap::new(),
                expected_window: false,
                evidence: "discovered CMake build artifact".into(),
            });
        }
    }

    if profile.build_systems.contains(&BuildSystemKind::Python) {
        if let Some(entry) = profile.entry_points.first() {
            return Some(RuntimeArtifact {
                program: PathBuf::from("python"),
                args: vec![entry.to_string_lossy().to_string()],
                cwd: profile.root.clone(),
                env: BTreeMap::new(),
                expected_window: false,
                evidence: "python entry point".into(),
            });
        }
    }
    if profile.build_systems.contains(&BuildSystemKind::Node) {
        if let Some(entry) = profile.entry_points.first() {
            return Some(RuntimeArtifact {
                program: PathBuf::from("node"),
                args: vec![entry.to_string_lossy().to_string()],
                cwd: profile.root.clone(),
                env: BTreeMap::new(),
                expected_window: false,
                evidence: "node entry point".into(),
            });
        }
    }
    None
}

fn find_cmake_runtime_binary(root: &Path) -> Option<PathBuf> {
    let build_root = root.join(".cortex").join("build").join("cmake");
    if !build_root.is_dir() {
        return None;
    }

    let mut candidates = Vec::<PathBuf>::new();
    let mut stack = vec![(build_root.clone(), 0usize)];
    while let Some((directory, depth)) = stack.pop() {
        if depth > 4 {
            continue;
        }
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if !matches!(
                    name.as_str(),
                    "cmakefiles" | "_deps" | "testing" | "compileridc" | "compileridcxx"
                ) {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            #[cfg(windows)]
            let executable = path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
            #[cfg(not(windows))]
            let executable = path.extension().is_none();

            if executable {
                candidates.push(path);
            }
        }
    }

    candidates.sort_by_key(|path| {
        (
            path.components().count(),
            path.to_string_lossy().to_ascii_lowercase(),
        )
    });
    candidates.into_iter().next()
}

// -------------------------------------------------------------------------
// M11E — Generic command profile fallback
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenericCommandProfile {
    #[serde(default)]
    pub format: Option<Vec<String>>,
    #[serde(default)]
    pub lint: Option<Vec<String>>,
    #[serde(default)]
    pub validate: Option<Vec<String>>,
    #[serde(default)]
    pub build: Option<Vec<String>>,
    #[serde(default)]
    pub test: Option<Vec<String>>,
    #[serde(default)]
    pub run: Option<Vec<String>>,
    #[serde(default)]
    pub artifact: Option<String>,
    #[serde(default)]
    pub expected_window: Option<bool>,
}

impl GenericCommandProfile {
    pub fn to_quality_gate_plan(&self, root: &Path, runtime_required: bool) -> QualityGatePlan {
        let mut steps = Vec::new();
        push_profile_step(
            &mut steps,
            GateCapability::Format,
            false,
            root,
            self.format.as_ref(),
            None,
        );
        push_profile_step(
            &mut steps,
            GateCapability::Lint,
            false,
            root,
            self.lint.as_ref(),
            None,
        );
        push_profile_step(
            &mut steps,
            GateCapability::Validate,
            true,
            root,
            self.validate.as_ref(),
            None,
        );
        push_profile_step(
            &mut steps,
            GateCapability::Build,
            true,
            root,
            self.build.as_ref(),
            self.artifact.as_ref().map(PathBuf::from),
        );
        push_profile_step(
            &mut steps,
            GateCapability::Test,
            false,
            root,
            self.test.as_ref(),
            None,
        );
        if runtime_required {
            push_profile_step(
                &mut steps,
                GateCapability::Launch,
                true,
                root,
                self.run.as_ref(),
                self.artifact.as_ref().map(PathBuf::from),
            );
        }
        QualityGatePlan {
            schema_version: M11_SCHEMA_VERSION,
            project_root: root.to_path_buf(),
            steps,
            diagnostics: Vec::new(),
        }
    }
}

pub fn load_generic_command_profile(root: &Path) -> Result<Option<GenericCommandProfile>, String> {
    let path = root.join(".cortex").join("project.json");
    if !path.is_file() {
        return Ok(None);
    }
    serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
        .map(Some)
        .map_err(|error| error.to_string())
}

// -------------------------------------------------------------------------
// M11F — Evidence Broker contracts
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    ProjectSource,
    ProjectManifest,
    Lockfile,
    ResolvedDependency,
    LocalDependencySource,
    CompilerDiagnostic,
    LanguageServer,
    InstalledExample,
    UpstreamRelease,
    OfficialDocumentation,
    WebResearch,
    ModelMemory,
    Image,
    VisionObservation,
    Artifact,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceRecord {
    pub kind: EvidenceKind,
    pub authority_rank: u8,
    pub source: String,
    pub version_scope: Option<String>,
    pub summary: String,
    pub content_hash: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiContract {
    pub package: String,
    pub resolved_version: Option<String>,
    pub required_symbols: Vec<String>,
    pub facts: Vec<String>,
    pub evidence: Vec<EvidenceRecord>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceBundle {
    pub records: Vec<EvidenceRecord>,
}

impl EvidenceBundle {
    pub fn push(&mut self, mut record: EvidenceRecord) {
        if record.authority_rank == 0 {
            record.authority_rank = evidence_authority(&record.kind);
        }
        self.records.push(record);
        self.records
            .sort_by_key(|right| std::cmp::Reverse(right.authority_rank));
    }

    pub fn highest_authority(&self) -> Option<&EvidenceRecord> {
        self.records.first()
    }
}

pub fn evidence_authority(kind: &EvidenceKind) -> u8 {
    match kind {
        EvidenceKind::ProjectSource => 100,
        EvidenceKind::ProjectManifest => 98,
        EvidenceKind::Lockfile => 97,
        EvidenceKind::ResolvedDependency => 96,
        EvidenceKind::LocalDependencySource => 95,
        EvidenceKind::CompilerDiagnostic => 94,
        EvidenceKind::LanguageServer => 92,
        EvidenceKind::InstalledExample => 90,
        EvidenceKind::UpstreamRelease => 85,
        EvidenceKind::OfficialDocumentation => 82,
        EvidenceKind::WebResearch => 65,
        EvidenceKind::Image | EvidenceKind::VisionObservation | EvidenceKind::Artifact => 60,
        EvidenceKind::ModelMemory => 10,
    }
}

pub fn mutation_requires_grounding(missing_packages: &[String], api_sensitive: bool) -> bool {
    api_sensitive && !missing_packages.is_empty()
}

// -------------------------------------------------------------------------
// M11G — Dynamic Capability Views
// -------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Chat,
    Inspect,
    Develop,
    Repair,
    Verify,
    Runtime,
    Research,
    ImageGenerate,
    ImageReview,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStage {
    Observe,
    Ground,
    Plan,
    Mutate,
    Verify,
    Launch,
    Review,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityView {
    pub task: TaskKind,
    pub stage: CapabilityStage,
    pub allowed_tools: Vec<String>,
    pub max_visible_tools: usize,
}

pub fn default_capability_view(task: TaskKind, stage: CapabilityStage) -> CapabilityView {
    let tools = match stage {
        CapabilityStage::Observe => vec![
            "workspace.status",
            "source.read",
            "source.list",
            "source.search",
            "project.profile",
        ],
        CapabilityStage::Ground => vec![
            "source.search",
            "source.read",
            "project.profile",
            "workspace.status",
        ],
        CapabilityStage::Plan => vec![
            "project.profile",
            "workspace.status",
            "source.read",
            "source.search",
        ],
        CapabilityStage::Mutate => vec![
            "source.read",
            "source.replace_text",
            "source.write_text",
            "source.transaction_status",
            "build.project_validate",
        ],
        CapabilityStage::Verify => vec![
            "build.project_format",
            "build.project_validate",
            "build.project_test",
            "workspace.status",
        ],
        CapabilityStage::Launch => vec![
            "runtime.launch_project",
            "runtime.process_status",
            "runtime.process_stop",
            "capture.screen",
        ],
        CapabilityStage::Review => vec!["capture.screen", "image.inspect", "workspace.status"],
    };
    CapabilityView {
        task,
        stage,
        allowed_tools: tools.into_iter().map(str::to_string).collect(),
        max_visible_tools: 12,
    }
}

pub fn clamp_capability_view(
    mut view: CapabilityView,
    model: &ModelCapabilityProfile,
) -> CapabilityView {
    let model_limit = model.max_reliable_visible_tools.max(1);
    let limit = view.max_visible_tools.min(model_limit);
    view.max_visible_tools = limit;
    view.allowed_tools.truncate(limit);
    view
}

// -------------------------------------------------------------------------
// M11H — Model Capability Profiles
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelCapabilityProfile {
    pub id: String,
    pub family: Option<String>,
    pub quantization: Option<String>,
    pub context_window: Option<usize>,
    pub reliable_context_window: Option<usize>,
    pub max_reliable_visible_tools: usize,
    pub supports_chat: bool,
    pub supports_code: bool,
    pub supports_vision: bool,
    pub supports_embeddings: bool,
    pub supports_native_tools: bool,
    pub supports_constrained_json: bool,
    pub supports_fim: bool,
    pub preferred_edit_mode: String,
    pub planning_score: f32,
    pub coding_score: f32,
    pub reconstruction_score: f32,
    pub known_failures: Vec<String>,
}

impl ModelCapabilityProfile {
    pub fn conservative_local(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            max_reliable_visible_tools: 10,
            supports_chat: true,
            supports_code: true,
            preferred_edit_mode: "whole_file_or_small_diff".into(),
            planning_score: 0.5,
            coding_score: 0.5,
            reconstruction_score: 0.4,
            ..Self::default()
        }
    }
}

// -------------------------------------------------------------------------
// M11I — Task Capsule / context compiler contract
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskCapsule {
    pub schema_version: u32,
    pub objective: String,
    pub stage: String,
    pub candidate_revision: Option<u64>,
    pub relevant_files: Vec<PathBuf>,
    pub relevant_symbols: Vec<String>,
    pub api_contracts: Vec<ApiContract>,
    pub diagnostics: Vec<String>,
    pub project_rules: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub attempted_strategies: Vec<String>,
    pub required_next_stage: Option<String>,
}

// -------------------------------------------------------------------------
// M11J — Repair Strategy / reconstruction escalation
// -------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepairStrategy {
    Surgical,
    MultiFile,
    DependencyAdjustment,
    Reconstruction,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepairSignal {
    pub likely_api_version_mismatch: bool,
    pub errors_from_same_dependency: usize,
    pub affected_files: usize,
    pub compact_module: bool,
    pub missing_dependency_grounding: Vec<String>,
}

pub fn choose_repair_strategy(signal: &RepairSignal) -> RepairStrategy {
    if signal.likely_api_version_mismatch
        && signal.errors_from_same_dependency >= 4
        && signal.compact_module
    {
        RepairStrategy::Reconstruction
    } else if !signal.missing_dependency_grounding.is_empty() {
        RepairStrategy::DependencyAdjustment
    } else if signal.affected_files > 1 {
        RepairStrategy::MultiFile
    } else {
        RepairStrategy::Surgical
    }
}

// -------------------------------------------------------------------------
// M11K — Code-intelligence protocol plane
// -------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntelligenceBackend {
    Compiler,
    LanguageServer,
    Scip,
    TreeSitter,
    AstGrep,
    TextSearch,
    Embeddings,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SymbolQueryKind {
    Definition,
    References,
    Implementations,
    Hover,
    Callers,
    Callees,
    Dependencies,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymbolQuery {
    pub kind: SymbolQueryKind,
    pub symbol: String,
    pub path: Option<PathBuf>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub preferred_backends: Vec<IntelligenceBackend>,
}

// -------------------------------------------------------------------------
// M11L — Architect → Editor → Verifier contracts
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditSpecification {
    pub summary: String,
    pub files: Vec<PathBuf>,
    pub invariants: Vec<String>,
    pub required_evidence: Vec<String>,
    pub verification: Vec<String>,
    pub prefer_whole_file: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRolePlan {
    pub architect_model: Option<String>,
    pub editor_model: Option<String>,
    pub verifier_is_deterministic: bool,
    pub isolate_editor_context: bool,
}

impl Default for AgentRolePlan {
    fn default() -> Self {
        Self {
            architect_model: None,
            editor_model: None,
            verifier_is_deterministic: true,
            isolate_editor_context: true,
        }
    }
}

// -------------------------------------------------------------------------
// M11M — Skills
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillManifest {
    pub schema_version: u32,
    pub id: String,
    pub version: String,
    pub description: String,
    pub triggers: Vec<String>,
    pub instructions: Vec<String>,
    pub required_capabilities: Vec<String>,
    pub required_tools: Vec<String>,
    pub optional_tools: Vec<String>,
    pub resources: Vec<String>,
    pub scripts: Vec<String>,
    pub validators: Vec<String>,
    pub model_preferences: Vec<String>,
    pub permission_profile: Option<String>,
    pub provenance: Option<String>,
}

// -------------------------------------------------------------------------
// M11N — MCP/A2A-ready protocol contracts
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProtocolEndpoint {
    pub protocol: String,
    pub version: String,
    pub endpoint: String,
    pub stateless_core: bool,
    pub extensions: Vec<String>,
    pub capabilities: Vec<String>,
}

// -------------------------------------------------------------------------
// M11O — Rich image generation recipe
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ImageGenerationRecipe {
    pub schema_version: u32,
    pub intent: String,
    pub prompt: String,
    pub negative_prompt: Option<String>,
    pub width: u32,
    pub height: u32,
    pub count: u32,
    pub seed: Option<u64>,
    pub model: Option<String>,
    pub vae: Option<String>,
    pub sampler: Option<String>,
    pub scheduler: Option<String>,
    pub steps: Option<u32>,
    pub guidance: Option<f32>,
    pub denoise: Option<f32>,
    pub loras: Vec<String>,
    pub embeddings: Vec<String>,
    pub input_images: Vec<PathBuf>,
    pub reference_images: Vec<PathBuf>,
    pub masks: Vec<PathBuf>,
    pub control_inputs: Vec<ControlInput>,
    pub ip_adapter_inputs: Vec<PathBuf>,
    pub upscale: Option<String>,
    pub workflow_profile: Option<String>,
    pub output_contract: Option<ImageOutputContract>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ControlInput {
    pub kind: String,
    pub image: PathBuf,
    pub strength: f32,
}

// -------------------------------------------------------------------------
// M11P — Typed ComfyUI workflow profiles
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowProfile {
    pub schema_version: u32,
    pub id: String,
    pub version: String,
    pub capabilities: Vec<String>,
    pub bindings: BTreeMap<String, String>,
    pub required_models: Vec<String>,
    pub required_nodes: Vec<String>,
    pub supported_model_families: Vec<String>,
    pub estimated_vram_mb: Option<u64>,
}

// -------------------------------------------------------------------------
// M11Q — Image model compatibility/provenance graph
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageModelProfile {
    pub id: String,
    pub family: String,
    pub architecture: String,
    pub checkpoint: PathBuf,
    pub checkpoint_hash: Option<String>,
    pub vae: Option<PathBuf>,
    pub compatible_loras: Vec<String>,
    pub compatible_controlnets: Vec<String>,
    pub compatible_ip_adapters: Vec<String>,
    pub supported_workflows: Vec<String>,
    pub precision: Option<String>,
    pub estimated_vram_mb: Option<u64>,
    pub source: Option<String>,
    pub license: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageProvenance {
    pub model_id: String,
    pub model_checksum: Option<String>,
    pub workflow_id: Option<String>,
    pub workflow_checksum: Option<String>,
    pub seed: Option<u64>,
    pub sampler: Option<String>,
    pub scheduler: Option<String>,
    pub steps: Option<u32>,
    pub loras: Vec<String>,
    pub controlnets: Vec<String>,
    pub ip_adapters: Vec<String>,
    pub source_images: Vec<PathBuf>,
    pub source_hashes: Vec<String>,
    pub parent_artifact: Option<String>,
    pub generation_revision: u64,
    pub generated_unix_ms: u128,
}

pub fn validate_image_stack(
    recipe: &ImageGenerationRecipe,
    workflow: &WorkflowProfile,
    model: &ImageModelProfile,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if !workflow.supported_model_families.is_empty()
        && !workflow
            .supported_model_families
            .iter()
            .any(|family| family.eq_ignore_ascii_case(&model.family))
    {
        errors.push(format!(
            "workflow {} does not support model family {}",
            workflow.id, model.family
        ));
    }
    for lora in &recipe.loras {
        if !model.compatible_loras.is_empty() && !model.compatible_loras.contains(lora) {
            errors.push(format!(
                "LoRA {lora} is not certified for model {}",
                model.id
            ));
        }
    }
    for control in &recipe.control_inputs {
        if !model.compatible_controlnets.is_empty()
            && !model
                .compatible_controlnets
                .iter()
                .any(|item| item == &control.kind)
        {
            errors.push(format!(
                "ControlNet/control input {} is not certified for model {}",
                control.kind, model.id
            ));
        }
    }
    if !recipe.ip_adapter_inputs.is_empty() && model.compatible_ip_adapters.is_empty() {
        errors.push(format!("model {} has no certified IP-Adapter", model.id));
    }
    if let Some(requested) = &recipe.workflow_profile {
        if requested != &workflow.id {
            errors.push(format!(
                "recipe requests workflow {requested}, but {} was selected",
                workflow.id
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// -------------------------------------------------------------------------
// M11R — Vision critic / output contracts
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ImageOutputContract {
    pub exact_width: Option<u32>,
    pub exact_height: Option<u32>,
    pub alpha_required: bool,
    pub single_subject: bool,
    pub no_text: bool,
    pub centered_subject: bool,
    pub max_colors: Option<u32>,
    pub semantic_requirements: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct VisionReview {
    pub accepted: bool,
    pub mechanical_score: f32,
    pub semantic_score: f32,
    pub violations: Vec<String>,
    pub retry_guidance: Vec<String>,
}

// -------------------------------------------------------------------------
// M11S — Resource/GPU leases
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceLease {
    pub execution_id: String,
    pub provider: String,
    pub model: Option<String>,
    pub cpu_threads: Option<u32>,
    pub ram_mb: Option<u64>,
    pub gpu_index: Option<u32>,
    pub vram_mb: Option<u64>,
    pub residency_priority: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceScheduler {
    pub capacity_vram_mb: Option<u64>,
    pub leases: Vec<ResourceLease>,
}

impl ResourceScheduler {
    pub fn reserved_vram_mb(&self) -> u64 {
        self.leases.iter().filter_map(|lease| lease.vram_mb).sum()
    }

    pub fn can_reserve_vram(&self, requested_mb: u64) -> bool {
        self.capacity_vram_mb
            .map(|capacity| self.reserved_vram_mb().saturating_add(requested_mb) <= capacity)
            .unwrap_or(true)
    }

    pub fn try_acquire(&mut self, lease: ResourceLease) -> Result<(), String> {
        if let Some(requested) = lease.vram_mb {
            if !self.can_reserve_vram(requested) {
                return Err(format!(
                    "resource lease would overcommit VRAM: reserved={} MiB requested={} MiB capacity={:?} MiB",
                    self.reserved_vram_mb(),
                    requested,
                    self.capacity_vram_mb
                ));
            }
        }
        self.leases.retain(|existing| {
            !(existing.execution_id == lease.execution_id
                && existing.provider == lease.provider
                && existing.model == lease.model)
        });
        self.leases.push(lease);
        Ok(())
    }

    pub fn release_execution(&mut self, execution_id: &str) {
        self.leases
            .retain(|lease| lease.execution_id != execution_id);
    }
}

// -------------------------------------------------------------------------
// M11T — Eval Lab + OpenTelemetry-ready execution spans
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct EvalMetrics {
    pub verified_completion: bool,
    pub duration_ms: u128,
    pub model_calls: u32,
    pub tool_calls: u32,
    pub invalid_tool_calls: u32,
    pub compiler_errors_introduced: u32,
    pub compiler_errors_resolved: u32,
    pub api_hallucinations: u32,
    pub grounding_completeness: f32,
    pub rollback_count: u32,
    pub runtime_verified: bool,
    pub image_contract_score: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionSpan {
    pub name: String,
    pub started_unix_ms: u128,
    pub duration_ms: u128,
    pub model: Option<String>,
    pub tool: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub attributes: BTreeMap<String, String>,
}

// -------------------------------------------------------------------------
// Universal adapter facade
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UniversalProjectPlan {
    pub profile: ProjectProfile,
    pub toolchains: ToolchainCatalog,
    pub quality_gate: QualityGatePlan,
    pub runtime: Option<RuntimeArtifact>,
}

pub fn plan_project(root: &Path, runtime_required: bool) -> UniversalProjectPlan {
    let profile = detect_project_profile(root);
    let toolchains = ToolchainCatalog::discover();
    let quality_gate = plan_quality_gate(&profile, &toolchains, runtime_required);
    let runtime = resolve_runtime_artifact(&profile, &quality_gate);
    UniversalProjectPlan {
        profile,
        toolchains,
        quality_gate,
        runtime,
    }
}

// -------------------------------------------------------------------------
// Internal helpers
// -------------------------------------------------------------------------

fn scan_shallow(root: &Path, max_depth: usize, visit: &mut dyn FnMut(&Path, &Path)) {
    fn walk(
        root: &Path,
        current: &Path,
        depth: usize,
        max_depth: usize,
        visit: &mut dyn FnMut(&Path, &Path),
    ) {
        if depth > max_depth {
            return;
        }
        let Ok(entries) = fs::read_dir(current) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir()
                && matches!(
                    name.as_ref(),
                    ".git" | ".open2d" | "target" | "node_modules" | ".venv" | "build" | "dist"
                )
            {
                continue;
            }
            let relative = path.strip_prefix(root).unwrap_or(&path);
            visit(&path, relative);
            if path.is_dir() {
                walk(root, &path, depth + 1, max_depth, visit);
            }
        }
    }
    walk(root, root, 0, max_depth, visit);
}

fn language_for_path(path: &Path) -> Option<LanguageKind> {
    let extension = path
        .extension()
        .and_then(OsStr::to_str)?
        .to_ascii_lowercase();
    Some(match extension.as_str() {
        "rs" => LanguageKind::Rust,
        "c" | "h" => LanguageKind::C,
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => LanguageKind::Cpp,
        "cs" => LanguageKind::CSharp,
        "py" | "pyw" => LanguageKind::Python,
        "js" | "jsx" | "mjs" | "cjs" => LanguageKind::JavaScript,
        "ts" | "tsx" | "mts" | "cts" => LanguageKind::TypeScript,
        "java" => LanguageKind::Java,
        "kt" | "kts" => LanguageKind::Kotlin,
        "go" => LanguageKind::Go,
        "lua" => LanguageKind::Lua,
        "sh" | "bash" | "ps1" | "bat" | "cmd" => LanguageKind::Shell,
        _ => return None,
    })
}

fn build_system_for_manifest(path: &Path) -> Option<BuildSystemKind> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match name.as_str() {
        "cargo.toml" => Some(BuildSystemKind::Cargo),
        "cmakelists.txt" => Some(BuildSystemKind::CMake),
        "package.json" => Some(BuildSystemKind::Node),
        "pyproject.toml" | "setup.py" | "requirements.txt" => Some(BuildSystemKind::Python),
        "go.mod" => Some(BuildSystemKind::Go),
        "pom.xml" => Some(BuildSystemKind::Maven),
        "build.gradle" | "build.gradle.kts" | "gradlew" | "gradlew.bat" => {
            Some(BuildSystemKind::Gradle)
        }
        "makefile" => Some(BuildSystemKind::Make),
        "build.ninja" => Some(BuildSystemKind::Ninja),
        _ if matches!(extension.as_str(), "sln" | "vcxproj") => Some(BuildSystemKind::MsBuild),
        _ if matches!(extension.as_str(), "csproj" | "fsproj" | "vbproj") => {
            Some(BuildSystemKind::DotNet)
        }
        _ => None,
    }
}

fn is_entry_point(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "main.rs"
            | "main.c"
            | "main.cpp"
            | "main.cc"
            | "main.cxx"
            | "main.py"
            | "app.py"
            | "main.js"
            | "index.js"
            | "main.ts"
            | "index.ts"
            | "main.go"
    )
}

fn capabilities_for(builds: &[BuildSystemKind], languages: &[LanguageKind]) -> ProjectCapabilities {
    let buildable = !builds.is_empty();
    let runnable = buildable
        || languages.iter().any(|language| {
            matches!(
                language,
                &LanguageKind::Python | &LanguageKind::JavaScript | &LanguageKind::TypeScript
            )
        });
    ProjectCapabilities {
        format: builds.contains(&BuildSystemKind::Cargo)
            || builds.contains(&BuildSystemKind::DotNet)
            || builds.contains(&BuildSystemKind::Go),
        lint: buildable,
        validate: buildable,
        build: buildable,
        test: buildable,
        run: runnable,
        debug: runnable,
        package: buildable,
    }
}

fn find_on_path(names: &[&str]) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        for name in names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn discover_msvc() -> Option<ToolchainRecord> {
    let program_files_x86 = env::var_os("ProgramFiles(x86)")?;
    let vswhere = PathBuf::from(program_files_x86)
        .join("Microsoft Visual Studio")
        .join("Installer")
        .join("vswhere.exe");
    if !vswhere.is_file() {
        return None;
    }
    let output = Command::new(&vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let install = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if install.is_empty() {
        return None;
    }
    let install = PathBuf::from(install);
    let tools_root = install.join("VC").join("Tools").join("MSVC");
    let mut versions = fs::read_dir(&tools_root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    versions.sort();
    let version = versions.pop()?;
    let cl = version
        .join("bin")
        .join("Hostx64")
        .join("x64")
        .join("cl.exe");
    if !cl.is_file() {
        return None;
    }
    let devcmd = install.join("Common7").join("Tools").join("VsDevCmd.bat");
    Some(ToolchainRecord {
        id: "msvc".into(),
        executable: cl,
        version_hint: version
            .file_name()
            .map(|value| value.to_string_lossy().to_string()),
        activation_script: devcmd.is_file().then_some(devcmd),
        capabilities: vec!["cpp".into(), "compile".into(), "windows".into()],
    })
}

#[cfg(not(windows))]
fn discover_msvc() -> Option<ToolchainRecord> {
    None
}

fn command(program: impl Into<String>, args: &[&str], cwd: &Path) -> CommandSpec {
    CommandSpec {
        program: program.into(),
        args: args.iter().map(|value| (*value).to_string()).collect(),
        cwd: cwd.to_path_buf(),
        env: BTreeMap::new(),
    }
}

fn cargo_quality_gate(
    profile: &ProjectProfile,
    tools: &ToolchainCatalog,
    runtime_required: bool,
) -> QualityGatePlan {
    let mut diagnostics = Vec::new();
    if !tools.has("cargo") {
        diagnostics.push("Cargo project detected but cargo is not discoverable on PATH.".into());
    }
    let mut steps = vec![
        QualityGateStep {
            capability: GateCapability::Format,
            required: true,
            command: Some(command("cargo", &["fmt", "--", "--check"], &profile.root)),
            expected_artifact: None,
        },
        QualityGateStep {
            capability: GateCapability::Validate,
            required: true,
            command: Some(command(
                "cargo",
                &["check", "--message-format=json"],
                &profile.root,
            )),
            expected_artifact: None,
        },
        QualityGateStep {
            capability: GateCapability::Test,
            required: false,
            command: Some(command("cargo", &["test"], &profile.root)),
            expected_artifact: None,
        },
        QualityGateStep {
            capability: GateCapability::Build,
            required: true,
            command: Some(command("cargo", &["build"], &profile.root)),
            expected_artifact: cargo_expected_artifact(profile),
        },
    ];
    if runtime_required {
        steps.push(QualityGateStep {
            capability: GateCapability::Launch,
            required: true,
            command: None,
            expected_artifact: cargo_expected_artifact(profile),
        });
    }
    QualityGatePlan {
        schema_version: M11_SCHEMA_VERSION,
        project_root: profile.root.clone(),
        steps,
        diagnostics,
    }
}

fn cargo_expected_artifact(profile: &ProjectProfile) -> Option<PathBuf> {
    let manifest = fs::read_to_string(profile.root.join("Cargo.toml")).ok();
    let package_name = manifest
        .as_deref()
        .and_then(cargo_manifest_package_name)
        .or_else(|| {
            profile
                .root
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
        })?;
    #[cfg(windows)]
    {
        Some(PathBuf::from(format!("target/debug/{package_name}.exe")))
    }
    #[cfg(not(windows))]
    {
        Some(PathBuf::from(format!("target/debug/{package_name}")))
    }
}

fn cargo_manifest_package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for raw in manifest.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "name" {
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

fn cmake_quality_gate(
    profile: &ProjectProfile,
    tools: &ToolchainCatalog,
    runtime_required: bool,
) -> QualityGatePlan {
    let mut diagnostics = Vec::new();
    if !tools.has("cmake") {
        diagnostics.push("CMake project detected but cmake is not discoverable on PATH.".into());
    }
    let build_dir = ".cortex/build/cmake";
    let mut steps = vec![
        QualityGateStep {
            capability: GateCapability::Validate,
            required: true,
            command: Some(command(
                "cmake",
                &["-S", ".", "-B", build_dir],
                &profile.root,
            )),
            expected_artifact: None,
        },
        QualityGateStep {
            capability: GateCapability::Build,
            required: true,
            command: Some(command(
                "cmake",
                &["--build", build_dir, "--config", "Debug"],
                &profile.root,
            )),
            expected_artifact: None,
        },
    ];
    if runtime_required {
        steps.push(QualityGateStep {
            capability: GateCapability::Launch,
            required: true,
            command: None,
            expected_artifact: None,
        });
    }
    QualityGatePlan {
        schema_version: M11_SCHEMA_VERSION,
        project_root: profile.root.clone(),
        steps,
        diagnostics,
    }
}

fn msbuild_quality_gate(
    profile: &ProjectProfile,
    tools: &ToolchainCatalog,
    runtime_required: bool,
) -> QualityGatePlan {
    let project = profile
        .manifests
        .iter()
        .find(|path| {
            matches!(
                path.extension().and_then(OsStr::to_str),
                Some("sln") | Some("vcxproj")
            )
        })
        .cloned();
    let mut diagnostics = Vec::new();
    let program = tools
        .get("msbuild")
        .map(|record| record.executable.to_string_lossy().to_string())
        .unwrap_or_else(|| "msbuild".into());
    if project.is_none() {
        diagnostics.push("MSBuild profile selected without a solution/project manifest.".into());
    }
    let mut steps = Vec::new();
    if let Some(project) = project {
        let project_text = project.to_string_lossy().to_string();
        steps.push(QualityGateStep {
            capability: GateCapability::Build,
            required: true,
            command: Some(CommandSpec {
                program,
                args: vec![
                    project_text,
                    "/t:Build".into(),
                    "/p:Configuration=Debug".into(),
                ],
                cwd: profile.root.clone(),
                env: BTreeMap::new(),
            }),
            expected_artifact: None,
        });
    }
    if runtime_required {
        steps.push(QualityGateStep {
            capability: GateCapability::Launch,
            required: true,
            command: None,
            expected_artifact: None,
        });
    }
    QualityGatePlan {
        schema_version: M11_SCHEMA_VERSION,
        project_root: profile.root.clone(),
        steps,
        diagnostics,
    }
}

fn dotnet_quality_gate(
    profile: &ProjectProfile,
    tools: &ToolchainCatalog,
    runtime_required: bool,
) -> QualityGatePlan {
    let mut diagnostics = Vec::new();
    if !tools.has("dotnet") {
        diagnostics.push(".NET project detected but dotnet is not discoverable on PATH.".into());
    }
    let mut steps = vec![
        QualityGateStep {
            capability: GateCapability::Build,
            required: true,
            command: Some(command("dotnet", &["build"], &profile.root)),
            expected_artifact: None,
        },
        QualityGateStep {
            capability: GateCapability::Test,
            required: false,
            command: Some(command("dotnet", &["test", "--no-build"], &profile.root)),
            expected_artifact: None,
        },
    ];
    if runtime_required {
        steps.push(QualityGateStep {
            capability: GateCapability::Launch,
            required: true,
            command: Some(command("dotnet", &["run", "--no-build"], &profile.root)),
            expected_artifact: None,
        });
    }
    QualityGatePlan {
        schema_version: M11_SCHEMA_VERSION,
        project_root: profile.root.clone(),
        steps,
        diagnostics,
    }
}

fn direct_cpp_quality_gate(
    profile: &ProjectProfile,
    tools: &ToolchainCatalog,
    runtime_required: bool,
) -> QualityGatePlan {
    let source = profile
        .entry_points
        .iter()
        .find(|path| {
            matches!(
                path.extension().and_then(OsStr::to_str),
                Some("cpp") | Some("cc") | Some("cxx") | Some("c")
            )
        })
        .cloned()
        .or_else(|| find_first_source(&profile.root, &["cpp", "cc", "cxx", "c"]));
    let mut diagnostics = Vec::new();
    let Some(source) = source else {
        return QualityGatePlan {
            schema_version: M11_SCHEMA_VERSION,
            project_root: profile.root.clone(),
            steps: Vec::new(),
            diagnostics: vec![
                "C/C++ source detected but no compile entry point could be selected.".into(),
            ],
        };
    };
    let stem = source.file_stem().and_then(OsStr::to_str).unwrap_or("app");
    #[cfg(windows)]
    let artifact = PathBuf::from(format!(".cortex/build/{stem}.exe"));
    #[cfg(not(windows))]
    let artifact = PathBuf::from(format!(".cortex/build/{stem}"));

    let source_text = source.to_string_lossy().to_string();
    let artifact_text = artifact.to_string_lossy().to_string();
    let source_is_cpp = matches!(
        source.extension().and_then(OsStr::to_str),
        Some("cpp") | Some("cc") | Some("cxx")
    );
    let build_command = if let Some(msvc) = tools.get("msvc") {
        if let Some(devcmd) = &msvc.activation_script {
            let shell = format!(
                "call \"{}\" -arch=x64 -host_arch=x64 && if not exist .cortex\\build mkdir .cortex\\build && cl /nologo /EHsc \"{}\" /Fe:\"{}\"",
                devcmd.display(), source.display(), artifact.display()
            );
            CommandSpec {
                program: "cmd.exe".into(),
                args: vec!["/d".into(), "/s".into(), "/c".into(), shell],
                cwd: profile.root.clone(),
                env: BTreeMap::new(),
            }
        } else {
            CommandSpec {
                program: msvc.executable.to_string_lossy().to_string(),
                args: vec![
                    "/nologo".into(),
                    "/EHsc".into(),
                    source_text.clone(),
                    format!("/Fe:{artifact_text}"),
                ],
                cwd: profile.root.clone(),
                env: BTreeMap::new(),
            }
        }
    } else if source_is_cpp {
        if let Some(clang) = tools.get("clang++") {
            shell_compile_command(
                &profile.root,
                &clang.executable,
                &source_text,
                &artifact_text,
                "-std=c++20",
            )
        } else if let Some(gpp) = tools.get("g++") {
            shell_compile_command(
                &profile.root,
                &gpp.executable,
                &source_text,
                &artifact_text,
                "-std=c++20",
            )
        } else {
            diagnostics.push(
                "C++ source detected but no MSVC, clang++, or g++ compiler was discovered.".into(),
            );
            return QualityGatePlan {
                schema_version: M11_SCHEMA_VERSION,
                project_root: profile.root.clone(),
                steps: Vec::new(),
                diagnostics,
            };
        }
    } else if let Some(clang) = tools.get("clang") {
        shell_compile_command(
            &profile.root,
            &clang.executable,
            &source_text,
            &artifact_text,
            "-std=c17",
        )
    } else if let Some(gcc) = tools.get("gcc") {
        shell_compile_command(
            &profile.root,
            &gcc.executable,
            &source_text,
            &artifact_text,
            "-std=c17",
        )
    } else {
        diagnostics
            .push("C source detected but no MSVC, clang, or gcc compiler was discovered.".into());
        return QualityGatePlan {
            schema_version: M11_SCHEMA_VERSION,
            project_root: profile.root.clone(),
            steps: Vec::new(),
            diagnostics,
        };
    };

    let mut steps = vec![QualityGateStep {
        capability: GateCapability::Build,
        required: true,
        command: Some(build_command),
        expected_artifact: Some(artifact.clone()),
    }];
    if runtime_required {
        steps.push(QualityGateStep {
            capability: GateCapability::Launch,
            required: true,
            command: None,
            expected_artifact: Some(artifact),
        });
        steps.push(QualityGateStep {
            capability: GateCapability::RuntimeVerify,
            required: true,
            command: None,
            expected_artifact: None,
        });
    }
    QualityGatePlan {
        schema_version: M11_SCHEMA_VERSION,
        project_root: profile.root.clone(),
        steps,
        diagnostics,
    }
}

fn shell_compile_command(
    root: &Path,
    compiler: &Path,
    source: &str,
    artifact: &str,
    standard: &str,
) -> CommandSpec {
    #[cfg(windows)]
    {
        let script = format!(
            "if not exist .cortex\\build mkdir .cortex\\build && \"{}\" \"{}\" {} -o \"{}\"",
            compiler.display(),
            source,
            standard,
            artifact
        );
        CommandSpec {
            program: "cmd.exe".into(),
            args: vec!["/d".into(), "/s".into(), "/c".into(), script],
            cwd: root.to_path_buf(),
            env: BTreeMap::new(),
        }
    }
    #[cfg(not(windows))]
    {
        let script = format!(
            "mkdir -p .cortex/build && \"{}\" \"{}\" {} -o \"{}\"",
            compiler.display(),
            source,
            standard,
            artifact
        );
        CommandSpec {
            program: "sh".into(),
            args: vec!["-c".into(), script],
            cwd: root.to_path_buf(),
            env: BTreeMap::new(),
        }
    }
}

fn python_quality_gate(
    profile: &ProjectProfile,
    tools: &ToolchainCatalog,
    runtime_required: bool,
) -> QualityGatePlan {
    let mut diagnostics = Vec::new();
    if !tools.has("python") {
        diagnostics.push("Python project detected but python is not discoverable on PATH.".into());
    }
    let entry = profile
        .entry_points
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("main.py"));
    let entry_text = entry.to_string_lossy().to_string();
    let mut steps = vec![QualityGateStep {
        capability: GateCapability::Validate,
        required: true,
        command: Some(CommandSpec {
            program: "python".into(),
            args: vec!["-m".into(), "py_compile".into(), entry_text.clone()],
            cwd: profile.root.clone(),
            env: BTreeMap::new(),
        }),
        expected_artifact: None,
    }];
    if tools.has("pyright") {
        steps.push(QualityGateStep {
            capability: GateCapability::Lint,
            required: false,
            command: Some(command("pyright", &["."], &profile.root)),
            expected_artifact: None,
        });
    }
    if tools.has("pytest") {
        steps.push(QualityGateStep {
            capability: GateCapability::Test,
            required: false,
            command: Some(command("pytest", &["-q"], &profile.root)),
            expected_artifact: None,
        });
    }
    if runtime_required {
        steps.push(QualityGateStep {
            capability: GateCapability::Launch,
            required: true,
            command: Some(CommandSpec {
                program: "python".into(),
                args: vec![entry_text],
                cwd: profile.root.clone(),
                env: BTreeMap::new(),
            }),
            expected_artifact: None,
        });
    }
    QualityGatePlan {
        schema_version: M11_SCHEMA_VERSION,
        project_root: profile.root.clone(),
        steps,
        diagnostics,
    }
}

fn node_quality_gate(
    profile: &ProjectProfile,
    tools: &ToolchainCatalog,
    runtime_required: bool,
) -> QualityGatePlan {
    let mut diagnostics = Vec::new();
    if !tools.has("node") {
        diagnostics.push("Node project detected but node is not discoverable on PATH.".into());
    }
    let mut steps = Vec::new();
    if profile.languages.contains(&LanguageKind::TypeScript) && tools.has("tsc") {
        steps.push(QualityGateStep {
            capability: GateCapability::Validate,
            required: true,
            command: Some(command("tsc", &["--noEmit"], &profile.root)),
            expected_artifact: None,
        });
    }
    if tools.has("npm") {
        steps.push(QualityGateStep {
            capability: GateCapability::Build,
            required: false,
            command: Some(command(
                "npm",
                &["run", "build", "--if-present"],
                &profile.root,
            )),
            expected_artifact: None,
        });
        steps.push(QualityGateStep {
            capability: GateCapability::Test,
            required: false,
            command: Some(command("npm", &["test", "--if-present"], &profile.root)),
            expected_artifact: None,
        });
    }
    if runtime_required {
        let entry = profile
            .entry_points
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("index.js"));
        steps.push(QualityGateStep {
            capability: GateCapability::Launch,
            required: true,
            command: Some(CommandSpec {
                program: "node".into(),
                args: vec![entry.to_string_lossy().to_string()],
                cwd: profile.root.clone(),
                env: BTreeMap::new(),
            }),
            expected_artifact: None,
        });
    }
    QualityGatePlan {
        schema_version: M11_SCHEMA_VERSION,
        project_root: profile.root.clone(),
        steps,
        diagnostics,
    }
}

fn maven_quality_gate(
    profile: &ProjectProfile,
    tools: &ToolchainCatalog,
    runtime_required: bool,
) -> QualityGatePlan {
    let mut diagnostics = Vec::new();
    if !tools.has("mvn") {
        diagnostics.push("Maven project detected but mvn is not discoverable on PATH.".into());
    }
    let mut steps = vec![
        QualityGateStep {
            capability: GateCapability::Test,
            required: true,
            command: Some(command("mvn", &["test"], &profile.root)),
            expected_artifact: None,
        },
        QualityGateStep {
            capability: GateCapability::Build,
            required: true,
            command: Some(command("mvn", &["package", "-DskipTests"], &profile.root)),
            expected_artifact: None,
        },
    ];
    if runtime_required {
        steps.push(QualityGateStep {
            capability: GateCapability::Launch,
            required: true,
            command: None,
            expected_artifact: None,
        });
    }
    QualityGatePlan {
        schema_version: M11_SCHEMA_VERSION,
        project_root: profile.root.clone(),
        steps,
        diagnostics,
    }
}

fn gradle_quality_gate(
    profile: &ProjectProfile,
    tools: &ToolchainCatalog,
    runtime_required: bool,
) -> QualityGatePlan {
    let wrapper = if cfg!(windows) {
        profile.root.join("gradlew.bat")
    } else {
        profile.root.join("gradlew")
    };
    let program = if wrapper.is_file() {
        wrapper.to_string_lossy().to_string()
    } else {
        "gradle".into()
    };
    let mut diagnostics = Vec::new();
    if !wrapper.is_file() && !tools.has("gradle") {
        diagnostics.push("Gradle project detected but neither a project wrapper nor gradle on PATH is available.".into());
    }
    let mut steps = vec![
        QualityGateStep {
            capability: GateCapability::Test,
            required: true,
            command: Some(CommandSpec {
                program: program.clone(),
                args: vec!["test".into()],
                cwd: profile.root.clone(),
                env: BTreeMap::new(),
            }),
            expected_artifact: None,
        },
        QualityGateStep {
            capability: GateCapability::Build,
            required: true,
            command: Some(CommandSpec {
                program,
                args: vec!["build".into(), "-x".into(), "test".into()],
                cwd: profile.root.clone(),
                env: BTreeMap::new(),
            }),
            expected_artifact: None,
        },
    ];
    if runtime_required {
        steps.push(QualityGateStep {
            capability: GateCapability::Launch,
            required: true,
            command: None,
            expected_artifact: None,
        });
    }
    QualityGatePlan {
        schema_version: M11_SCHEMA_VERSION,
        project_root: profile.root.clone(),
        steps,
        diagnostics,
    }
}

fn make_quality_gate(
    profile: &ProjectProfile,
    tools: &ToolchainCatalog,
    runtime_required: bool,
) -> QualityGatePlan {
    let mut diagnostics = Vec::new();
    let program = tools
        .get("make")
        .map(|tool| tool.executable.to_string_lossy().to_string())
        .unwrap_or_else(|| "make".into());
    if !tools.has("make") {
        diagnostics.push("Make project detected but make is not discoverable on PATH.".into());
    }
    let mut steps = vec![QualityGateStep {
        capability: GateCapability::Build,
        required: true,
        command: Some(CommandSpec {
            program,
            args: Vec::new(),
            cwd: profile.root.clone(),
            env: BTreeMap::new(),
        }),
        expected_artifact: None,
    }];
    if runtime_required {
        steps.push(QualityGateStep {
            capability: GateCapability::Launch,
            required: true,
            command: None,
            expected_artifact: None,
        });
    }
    QualityGatePlan {
        schema_version: M11_SCHEMA_VERSION,
        project_root: profile.root.clone(),
        steps,
        diagnostics,
    }
}

fn ninja_quality_gate(
    profile: &ProjectProfile,
    tools: &ToolchainCatalog,
    runtime_required: bool,
) -> QualityGatePlan {
    let mut diagnostics = Vec::new();
    if !tools.has("ninja") {
        diagnostics.push("Ninja project detected but ninja is not discoverable on PATH.".into());
    }
    let mut steps = vec![QualityGateStep {
        capability: GateCapability::Build,
        required: true,
        command: Some(command("ninja", &[], &profile.root)),
        expected_artifact: None,
    }];
    if runtime_required {
        steps.push(QualityGateStep {
            capability: GateCapability::Launch,
            required: true,
            command: None,
            expected_artifact: None,
        });
    }
    QualityGatePlan {
        schema_version: M11_SCHEMA_VERSION,
        project_root: profile.root.clone(),
        steps,
        diagnostics,
    }
}

fn simple_tool_gate(
    profile: &ProjectProfile,
    tools: &ToolchainCatalog,
    tool: &str,
    validate_args: &[&str],
    build_args: &[&str],
    runtime_required: bool,
) -> QualityGatePlan {
    let mut diagnostics = Vec::new();
    if !tools.has(tool) {
        diagnostics.push(format!(
            "{} project detected but {tool} is not discoverable on PATH.",
            tool.to_ascii_uppercase()
        ));
    }
    let mut steps = vec![
        QualityGateStep {
            capability: GateCapability::Validate,
            required: true,
            command: Some(command(tool, validate_args, &profile.root)),
            expected_artifact: None,
        },
        QualityGateStep {
            capability: GateCapability::Build,
            required: true,
            command: Some(command(tool, build_args, &profile.root)),
            expected_artifact: None,
        },
    ];
    if runtime_required {
        steps.push(QualityGateStep {
            capability: GateCapability::Launch,
            required: true,
            command: Some(command(tool, &["run", "."], &profile.root)),
            expected_artifact: None,
        });
    }
    QualityGatePlan {
        schema_version: M11_SCHEMA_VERSION,
        project_root: profile.root.clone(),
        steps,
        diagnostics,
    }
}

fn push_profile_step(
    steps: &mut Vec<QualityGateStep>,
    capability: GateCapability,
    required: bool,
    root: &Path,
    value: Option<&Vec<String>>,
    artifact: Option<PathBuf>,
) {
    let command = value
        .and_then(|items| items.split_first())
        .map(|(program, args)| CommandSpec {
            program: program.clone(),
            args: args.to_vec(),
            cwd: root.to_path_buf(),
            env: BTreeMap::new(),
        });
    if command.is_some() || required {
        steps.push(QualityGateStep {
            capability,
            required,
            command,
            expected_artifact: artifact,
        });
    }
}

fn find_first_source(root: &Path, extensions: &[&str]) -> Option<PathBuf> {
    let mut found = None;
    scan_shallow(root, 2, &mut |absolute, relative| {
        if found.is_some() || !absolute.is_file() {
            return;
        }
        if relative
            .extension()
            .and_then(OsStr::to_str)
            .map(|ext| {
                extensions
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(ext))
            })
            .unwrap_or(false)
        {
            found = Some(relative.to_path_buf());
        }
    });
    found
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn temp_project(name: &str) -> PathBuf {
        let root = env::temp_dir().join(format!(
            "cortex-universal-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn m11_has_twenty_pass_markers() {
        assert_eq!(M11_PASS_MARKERS.len(), 20);
    }

    #[test]
    fn single_cpp_project_gets_direct_compiler_profile() {
        let root = temp_project("cpp");
        fs::write(root.join("main.cpp"), "int main(){return 0;}\n").unwrap();
        let profile = detect_project_profile(&root);
        assert!(profile.languages.contains(&LanguageKind::Cpp));
        assert!(profile
            .build_systems
            .contains(&BuildSystemKind::DirectCompiler));
        assert_eq!(profile.kind, ProjectKind::SingleFile);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cargo_runtime_artifact_uses_manifest_package_name_not_folder_name() {
        let root = temp_project("cargo-artifact-name");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"actual_app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();

        let profile = detect_project_profile(&root);
        let artifact = cargo_expected_artifact(&profile).unwrap();
        let rendered = artifact.to_string_lossy().replace('\\', "/");
        assert!(
            rendered.ends_with("target/debug/actual_app")
                || rendered.ends_with("target/debug/actual_app.exe")
        );

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cmake_outranks_direct_cpp_fallback() {
        let root = temp_project("cmake");
        fs::write(root.join("main.cpp"), "int main(){return 0;}\n").unwrap();
        fs::write(
            root.join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.20)\n",
        )
        .unwrap();
        let profile = detect_project_profile(&root);
        assert!(profile.build_systems.contains(&BuildSystemKind::CMake));
        assert!(!profile
            .build_systems
            .contains(&BuildSystemKind::DirectCompiler));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn generic_profile_compiles_to_quality_gate() {
        let root = temp_project("generic");
        fs::create_dir_all(root.join(".cortex")).unwrap();
        fs::write(
            root.join(".cortex/project.json"),
            r#"{
          "validate": ["tool", "check"],
          "build": ["tool", "build"],
          "run": ["tool", "run"],
          "artifact": "out/app.exe",
          "expected_window": true
        }"#,
        )
        .unwrap();
        let profile = detect_project_profile(&root);
        let plan = plan_quality_gate(&profile, &ToolchainCatalog::default(), true);
        assert!(plan
            .steps
            .iter()
            .any(|step| step.capability == GateCapability::Build));
        let runtime = resolve_runtime_artifact(&profile, &plan).unwrap();
        assert!(runtime.expected_window);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn missing_grounding_is_a_hard_gate_for_api_sensitive_mutation() {
        assert!(mutation_requires_grounding(&["wgpu".into()], true));
        assert!(!mutation_requires_grounding(&["wgpu".into()], false));
        assert!(!mutation_requires_grounding(&[], true));
    }

    #[test]
    fn api_mismatch_selects_reconstruction() {
        let signal = RepairSignal {
            likely_api_version_mismatch: true,
            errors_from_same_dependency: 12,
            affected_files: 1,
            compact_module: true,
            missing_dependency_grounding: vec!["wgpu".into()],
        };
        assert_eq!(
            choose_repair_strategy(&signal),
            RepairStrategy::Reconstruction
        );
    }

    #[test]
    fn capability_views_are_bounded_for_local_models() {
        for stage in [
            CapabilityStage::Observe,
            CapabilityStage::Ground,
            CapabilityStage::Plan,
            CapabilityStage::Mutate,
            CapabilityStage::Verify,
            CapabilityStage::Launch,
            CapabilityStage::Review,
        ] {
            let view = default_capability_view(TaskKind::Repair, stage);
            assert!(view.allowed_tools.len() <= view.max_visible_tools);
            assert!(view.max_visible_tools <= 12);
        }
    }

    #[test]
    fn evidence_authority_puts_model_memory_last() {
        assert!(
            evidence_authority(&EvidenceKind::ProjectSource)
                > evidence_authority(&EvidenceKind::OfficialDocumentation)
        );
        assert!(
            evidence_authority(&EvidenceKind::OfficialDocumentation)
                > evidence_authority(&EvidenceKind::ModelMemory)
        );
    }

    #[test]
    fn resource_scheduler_rejects_vram_overcommit() {
        let scheduler = ResourceScheduler {
            capacity_vram_mb: Some(12_000),
            leases: vec![ResourceLease {
                execution_id: "e1".into(),
                provider: "native".into(),
                model: Some("coder".into()),
                cpu_threads: None,
                ram_mb: None,
                gpu_index: Some(0),
                vram_mb: Some(8_000),
                residency_priority: 10,
            }],
        };
        assert!(scheduler.can_reserve_vram(4_000));
        assert!(!scheduler.can_reserve_vram(4_001));
    }

    #[test]
    fn capability_view_honors_model_reliability_limit() {
        let view = default_capability_view(TaskKind::Repair, CapabilityStage::Mutate);
        let mut model = ModelCapabilityProfile::conservative_local("test");
        model.max_reliable_visible_tools = 3;
        let view = clamp_capability_view(view, &model);
        assert_eq!(view.max_visible_tools, 3);
        assert!(view.allowed_tools.len() <= 3);
    }

    #[test]
    fn image_stack_rejects_cross_family_workflow() {
        let recipe = ImageGenerationRecipe {
            schema_version: M11_SCHEMA_VERSION,
            intent: "concept".into(),
            prompt: "test".into(),
            width: 512,
            height: 512,
            count: 1,
            workflow_profile: Some("sdxl-concept".into()),
            ..ImageGenerationRecipe::default()
        };
        let workflow = WorkflowProfile {
            id: "sdxl-concept".into(),
            supported_model_families: vec!["sdxl".into()],
            ..WorkflowProfile::default()
        };
        let model = ImageModelProfile {
            id: "flux-model".into(),
            family: "flux".into(),
            ..ImageModelProfile::default()
        };
        assert!(validate_image_stack(&recipe, &workflow, &model).is_err());
    }

    #[test]
    fn image_recipe_round_trips() {
        let recipe = ImageGenerationRecipe {
            schema_version: M11_SCHEMA_VERSION,
            intent: "sprite".into(),
            prompt: "oak tree".into(),
            width: 64,
            height: 64,
            count: 1,
            output_contract: Some(ImageOutputContract {
                exact_width: Some(64),
                exact_height: Some(64),
                alpha_required: true,
                single_subject: true,
                no_text: true,
                centered_subject: true,
                max_colors: Some(32),
                semantic_requirements: vec!["top-down".into()],
            }),
            ..ImageGenerationRecipe::default()
        };
        let encoded = serde_json::to_vec(&recipe).unwrap();
        let decoded: ImageGenerationRecipe = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.width, 64);
        assert!(decoded.output_contract.unwrap().alpha_required);
    }
}
