//! Canonical project-aware Cortex tool broker.
//!
//! Open2D-specific runtime behavior is supplied through `cortex_adapter_open2d`;
//! this crate owns only the generic tool broker and catalog.

use cortex_adapter_git::GitAdapter;
use cortex_capture::CaptureService;
use cortex_context::ContextStore;
use cortex_development::{
    certification_plan, detect_project_profile, discover_roadmap, proposed_roadmap,
    CheckpointAuthorityKind, DevelopmentStore, MilestoneStatus, QualityCapability,
};
use cortex_execution::spine::{
    begin_grounding_for_workspace, canonical_project_relative, claim_transaction_for_workspace,
    complete_grounding_for_workspace, grounded_dependencies_for_workspace,
    parse_ground_before_write_packages, record_grounded_dependency_for_workspace,
    record_tool_for_workspace, record_tool_result_for_workspace,
    revoke_grounded_dependencies_for_workspace,
};
use cortex_image::{
    catalog_path_from_state, generated_output_root_from_state, metadata_root_from_state,
    ImageArtifactCatalog,
};
use cortex_jobs::{EventKind, EventStore, JobStatus, JobStore};
use cortex_pcc::PccClient;
use cortex_permissions::{parse_permission, permission_for_tool, Permission, PermissionPolicy};
use cortex_plugin::PluginRegistry;
use cortex_process::{CommandResult, OwnedProcessState, ProcessService, ProjectOperation};
use cortex_protocol::{
    EmbeddingProvider, ImageArtifactStatus, ImageGenerationRequest, ImageProvider, ToolCall,
    ToolDefinition, ToolExecutor, ToolResultInput, VisionProvider,
};
use cortex_registry::WorkspaceRegistry;
use cortex_rpc::{call as rpc_call, default_address, request_with_token as rpc_request_with_token};
use cortex_session::DevSession;
use cortex_toolchain::inspect_local_toolchain;
use cortex_transactions::TransactionManager;
use cortex_vault::{LibraryMemoryDatabase, Vault, VaultStoragePolicy};
use cortex_workspace::{LogicalPathResolution, Workspace};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs};

#[derive(Clone, Debug, Default)]
struct QualitySnapshot {
    errors: usize,
    warnings: usize,
    fingerprints: BTreeSet<String>,
    likely_api_version_mismatch: bool,
}

#[derive(Clone, Debug)]
struct CandidateRepairState {
    transaction_id: String,
    generation: u32,
    baseline: Option<QualitySnapshot>,
    baseline_origin: &'static str,
    mutations: u32,
    validations: u32,
    last_validation_success: bool,
    last_decision: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CandidateDecision {
    KeepVerified,
    KeepImproved,
    RollbackUnchanged,
    RollbackRegressed,
    ObserveWithoutBaseline,
}

impl CandidateDecision {
    fn label(self) -> &'static str {
        match self {
            Self::KeepVerified => "keep_verified",
            Self::KeepImproved => "keep_improved",
            Self::RollbackUnchanged => "rollback_unchanged",
            Self::RollbackRegressed => "rollback_regressed",
            Self::ObserveWithoutBaseline => "observe_without_baseline",
        }
    }

    fn should_rollback(self) -> bool {
        matches!(self, Self::RollbackUnchanged | Self::RollbackRegressed)
    }
}

#[derive(Clone, Debug)]
struct VerificationRecord {
    source_revision: u64,
    success: bool,
    errors: usize,
    warnings: usize,
    quality_fingerprint: String,
}

pub trait ToolExtension: Send {
    fn id(&self) -> &str;
    fn status(&self) -> Value;
    fn definitions(&self) -> Vec<ToolDefinition>;
    fn execute(&mut self, call: &ToolCall) -> Option<Result<Value, String>>;
}

pub struct ToolBroker {
    workspace: Workspace,
    transactions: TransactionManager,
    processes: ProcessService,
    vault: Vault,
    context: ContextStore,
    jobs: JobStore,
    events: EventStore,
    development: DevelopmentStore,
    embedding_provider: Option<Box<dyn EmbeddingProvider>>,
    permissions: PermissionPolicy,
    session: DevSession,
    capture: CaptureService,
    image_catalog: ImageArtifactCatalog,
    image_provider: Option<Box<dyn ImageProvider>>,
    vision_provider: Option<Box<dyn VisionProvider>>,
    git: Option<GitAdapter>,
    extensions: Vec<Box<dyn ToolExtension>>,
    failure_fingerprints: HashMap<String, u32>,
    last_quality_snapshot: Option<QualitySnapshot>,
    dependency_grounding_required: bool,
    grounded_dependency_packages: BTreeSet<String>,
    required_dependency_packages: BTreeSet<String>,
    source_revision: u64,
    verification_records: BTreeMap<String, VerificationRecord>,
    active_candidate: Option<CandidateRepairState>,
    candidate_generation: u32,
    quality_fingerprints: HashMap<String, u32>,
}

impl ToolBroker {
    pub fn new(
        project_root: impl AsRef<Path>,
        image_provider: Option<Box<dyn ImageProvider>>,
        vision_provider: Option<Box<dyn VisionProvider>>,
        embedding_provider: Option<Box<dyn EmbeddingProvider>>,
    ) -> Result<Self, String> {
        Self::new_with_extensions(
            project_root,
            image_provider,
            vision_provider,
            embedding_provider,
            Vec::new(),
        )
    }

    pub fn new_with_extensions(
        project_root: impl AsRef<Path>,
        image_provider: Option<Box<dyn ImageProvider>>,
        vision_provider: Option<Box<dyn VisionProvider>>,
        embedding_provider: Option<Box<dyn EmbeddingProvider>>,
        extensions: Vec<Box<dyn ToolExtension>>,
    ) -> Result<Self, String> {
        let bootstrap = Workspace::open(project_root.as_ref()).map_err(|e| e.to_string())?;
        let workspace = if let Ok(registry) = WorkspaceRegistry::open_default() {
            match registry.attach(&bootstrap) {
                Ok(record) => registry.open_workspace(&record).unwrap_or(bootstrap),
                Err(_) => bootstrap,
            }
        } else {
            bootstrap
        };
        let transactions = TransactionManager::new(workspace.root(), workspace.cortex_state_dir())?;
        let context = ContextStore::new(workspace.clone());
        let jobs = JobStore::open(workspace.cortex_state_dir())?;
        let events = EventStore::open(workspace.cortex_state_dir())?;
        let development = DevelopmentStore::open(&workspace.cortex_state_dir())?;
        let files = workspace
            .list_files("", 20_000)
            .map_err(|e| e.to_string())?;
        let vault_path = workspace
            .cortex_state_dir()
            .join("vault")
            .join("vault.json");
        let vault = if vault_path.is_file() {
            Vault::load(&vault_path)
                .unwrap_or_else(|_| Vault::build_lexical(workspace.root(), &files, 512 * 1024))
        } else {
            Vault::build_lexical(workspace.root(), &files, 512 * 1024)
        };
        let permission_path = workspace.cortex_state_dir().join("permissions.json");
        let permissions = PermissionPolicy::load_or_default(&permission_path)?;
        if !permission_path.is_file() {
            permissions.save(&permission_path)?;
        }
        let session = DevSession::create_in(workspace.root(), workspace.sessions_dir(), "cortex")?;
        let capture = CaptureService::new(workspace.root().to_path_buf());
        let image_catalog =
            ImageArtifactCatalog::load(&catalog_path_from_state(&workspace.cortex_state_dir()))?;
        let git = GitAdapter::detect(workspace.root());
        let grounded_dependency_packages = grounded_dependencies_for_workspace(workspace.root())
            .unwrap_or_default()
            .into_iter()
            .collect::<BTreeSet<_>>();
        Ok(Self {
            workspace,
            transactions,
            processes: ProcessService::default(),
            vault,
            context,
            jobs,
            events,
            development,
            embedding_provider,
            permissions,
            session,
            capture,
            image_catalog,
            image_provider,
            vision_provider,
            git,
            extensions,
            failure_fingerprints: HashMap::new(),
            last_quality_snapshot: None,
            dependency_grounding_required: false,
            grounded_dependency_packages,
            required_dependency_packages: BTreeSet::new(),
            source_revision: 0,
            verification_records: BTreeMap::new(),
            active_candidate: None,
            candidate_generation: 0,
            quality_fingerprints: HashMap::new(),
        })
    }

    pub fn workspace_root(&self) -> &Path {
        self.workspace.root()
    }

    fn canonical_source_path(
        &self,
        requested: &str,
        mutating: bool,
    ) -> Result<(String, LogicalPathResolution), String> {
        let resolution = self
            .workspace
            .resolve_logical_path(requested)
            .map_err(|e| e.to_string())?;
        if mutating && !resolution.class.autonomous_mutation_allowed() {
            return Err(format!("autonomous source mutation denied for path class {:?}: requested `{}` resolved to `{}`", resolution.class, requested, resolution.relative.display()));
        }
        Ok((
            resolution.relative.to_string_lossy().replace('\\', "/"),
            resolution,
        ))
    }

    fn dependency_grounding(&mut self) -> Result<Value, String> {
        project_dependency_grounding(self.workspace.root(), Some(&self.processes))
    }

    fn quality_result(&mut self, result: &CommandResult) -> Value {
        let current = quality_snapshot(result);
        let previous = self.last_quality_snapshot.replace(current.clone());
        let delta = quality_delta(previous.as_ref(), &current);
        if result.success {
            self.dependency_grounding_required = false;
            self.required_dependency_packages.clear();
        } else if current.likely_api_version_mismatch {
            self.dependency_grounding_required = true;
            let implicated = diagnostic_dependency_packages(self.workspace.root(), result);
            self.required_dependency_packages = if implicated.is_empty() {
                direct_dependency_names(self.workspace.root())
            } else {
                implicated
            };
            // M11U2: compiler diagnostics do not invalidate exact-version evidence.
            // Grounding remains valid until the dependency graph itself changes
            // (for example Cargo.toml mutation), preventing repeated re-grounding
            // of the same package after every compiler check.
        }
        let regressed = delta
            .get("regressed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let missing_grounding = self
            .required_dependency_packages
            .difference(&self.grounded_dependency_packages)
            .cloned()
            .collect::<Vec<_>>();
        json!({
            "current": quality_snapshot_json(&current), "delta": delta,
            "dependency_grounding_required": self.dependency_grounding_required,
            "required_dependency_packages": self.required_dependency_packages,
            "grounded_dependency_packages": self.grounded_dependency_packages,
            "missing_dependency_grounding": missing_grounding,
            "recommended_action": if current.likely_api_version_mismatch {
                "Before another API-sensitive mutation, ground every required package against the exact resolved local dependency source with source.search dependency_package. The mutation tools enforce this requirement."
            } else if regressed {
                "The candidate regressed. Return to the best-known candidate and change repair strategy before another mutation."
            } else { "Continue only if the next action addresses remaining diagnostic fingerprints." }
        })
    }

    fn record_verification(&mut self, capability: &str, result: &CommandResult) {
        let snapshot = quality_snapshot(result);
        self.verification_records.insert(
            capability.to_string(),
            VerificationRecord {
                source_revision: self.source_revision,
                success: result.success,
                errors: snapshot.errors,
                warnings: snapshot.warnings,
                quality_fingerprint: quality_fingerprint(&snapshot, result.success),
            },
        );
    }

    fn command_result_with_revision(&self, result: &CommandResult) -> Result<Value, String> {
        let mut value = serde_json::to_value(result).map_err(|error| error.to_string())?;
        if let Value::Object(object) = &mut value {
            object.insert("source_revision".into(), Value::from(self.source_revision));
        }
        Ok(value)
    }

    fn verification_authority_json(&self) -> Value {
        let profile = detect_project_profile(self.workspace.root());
        let required = if profile.checkpoint_authority.project_native() {
            vec!["checkpoint"]
        } else {
            profile
                .quality_capabilities
                .iter()
                .filter(|capability| !matches!(capability, QualityCapability::Package))
                .map(|capability| quality_capability_label(*capability))
                .collect::<Vec<_>>()
        };
        let mut evidence = serde_json::Map::new();
        let mut current_failures = 0usize;
        let mut current_successes = 0usize;
        let mut stale_records = 0usize;
        for capability in &required {
            let value = match self.verification_records.get(*capability) {
                Some(record) if record.source_revision == self.source_revision => {
                    if record.success {
                        current_successes += 1;
                    } else {
                        current_failures += 1;
                    }
                    json!({
                        "state": if record.success { "pass" } else { "fail" },
                        "source_revision": record.source_revision,
                        "errors": record.errors,
                        "warnings": record.warnings,
                        "quality_fingerprint": record.quality_fingerprint
                    })
                }
                Some(record) => {
                    stale_records += 1;
                    json!({
                        "state": "stale",
                        "source_revision": record.source_revision,
                        "current_source_revision": self.source_revision,
                        "last_success": record.success,
                        "quality_fingerprint": record.quality_fingerprint
                    })
                }
                None => json!({"state":"unknown"}),
            };
            evidence.insert((*capability).to_string(), value);
        }
        let state = if current_failures > 0 {
            "failed"
        } else if !required.is_empty() && current_successes == required.len() {
            "healthy"
        } else if stale_records > 0 {
            "stale"
        } else {
            "unknown"
        };
        json!({
            "state": state,
            "source_revision": self.source_revision,
            "required_capabilities": required,
            "checkpoint_authority": profile.checkpoint_authority,
            "evidence": evidence,
            "claim_policy": "When a project-native checkpoint exists it is the highest project-health authority. Otherwise every generated quality capability must have PASS evidence for the current source revision. Any source mutation makes older evidence stale."
        })
    }

    fn mutation_dependency_packages(&self, text: &str) -> BTreeSet<String> {
        dependency_references_in_text(self.workspace.root(), text)
    }

    fn ensure_mutation_dependency_grounding(&self, text: &str) -> Result<(), String> {
        let referenced = self.mutation_dependency_packages(text);
        let mut required = referenced;
        if self.dependency_grounding_required {
            required.extend(self.required_dependency_packages.iter().cloned());
        }
        let missing = required
            .difference(&self.grounded_dependency_packages)
            .cloned()
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return Ok(());
        }
        Err(format!(
            "Ground Before Write blocked this API-sensitive mutation. Exact dependency-source evidence is missing for: {}. Use source.search with dependency_package for each package (against the resolved local source), then retry the mutation. Do not guess dependency APIs from memory.",
            missing.join(", ")
        ))
    }

    fn note_source_mutation(&mut self, path: &str) {
        self.source_revision = self.source_revision.saturating_add(1);
        if path.eq_ignore_ascii_case("Cargo.toml")
            || path.ends_with("/Cargo.toml")
            || path.ends_with("\\Cargo.toml")
        {
            let revoked = self
                .grounded_dependency_packages
                .iter()
                .cloned()
                .collect::<Vec<_>>();
            let _ = revoke_grounded_dependencies_for_workspace(self.workspace.root(), &revoked);
            self.grounded_dependency_packages.clear();
            // M11U2: changing the manifest invalidates evidence, not the identity
            // of the packages that the current diagnostics require. Preserve the
            // exact required set until the mandatory post-mutation validation
            // recomputes it from the newly resolved graph.
            self.dependency_grounding_required = !self.required_dependency_packages.is_empty();
        }
        if let Some(candidate) = self.active_candidate.as_mut() {
            candidate.mutations = candidate.mutations.saturating_add(1);
            candidate.last_validation_success = false;
            candidate.last_decision = "mutation_pending_validation";
        }
    }

    fn capture_candidate_baseline(&mut self) -> (Option<QualitySnapshot>, Value) {
        match self
            .processes
            .run_project_operation(self.workspace.root(), ProjectOperation::Validate)
        {
            Ok(result) => {
                let _ = self.record_development_command(
                    "quality.candidate_baseline",
                    "Candidate repair baseline validation",
                    &result,
                );
                let snapshot = quality_snapshot(&result);
                self.last_quality_snapshot = Some(snapshot.clone());
                self.record_verification("validate", &result);
                if snapshot.likely_api_version_mismatch {
                    self.dependency_grounding_required = true;
                    let implicated = diagnostic_dependency_packages(self.workspace.root(), &result);
                    self.required_dependency_packages = if implicated.is_empty() {
                        direct_dependency_names(self.workspace.root())
                    } else {
                        implicated
                    };
                    for package in &self.required_dependency_packages {
                        self.grounded_dependency_packages.remove(package);
                    }
                    let revoked = self
                        .required_dependency_packages
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>();
                    let _ =
                        revoke_grounded_dependencies_for_workspace(self.workspace.root(), &revoked);
                }
                (
                    Some(snapshot.clone()),
                    json!({
                        "available": true,
                        "origin": "fresh_project_validate",
                        "success": result.success,
                        "quality": quality_snapshot_json(&snapshot),
                        "duration_ms": result.duration_ms,
                        "program": result.program,
                        "args": result.args
                    }),
                )
            }
            Err(error) => (
                None,
                json!({
                    "available": false,
                    "origin": "unavailable",
                    "error": error,
                    "instruction": "Candidate monotonicity cannot compare project quality until the active project adapter exposes validation. Source transaction behavior remains reversible."
                }),
            ),
        }
    }

    fn candidate_status_json(&self) -> Value {
        match &self.active_candidate {
            Some(candidate) => json!({
                "transaction_id": candidate.transaction_id,
                "generation": candidate.generation,
                "baseline_available": candidate.baseline.is_some(),
                "baseline_origin": candidate.baseline_origin,
                "mutations": candidate.mutations,
                "validations": candidate.validations,
                "last_validation_success": candidate.last_validation_success,
                "last_decision": candidate.last_decision
            }),
            None => Value::Null,
        }
    }

    fn evaluate_active_candidate(&mut self, result: &CommandResult) -> Result<Value, String> {
        let current = quality_snapshot(result);
        let fingerprint = quality_fingerprint(&current, result.success);
        let repeat_count = if result.success {
            0
        } else {
            let count = self
                .quality_fingerprints
                .entry(fingerprint.clone())
                .or_insert(0);
            *count = count.saturating_add(1);
            *count
        };

        let Some(candidate_snapshot) = self.active_candidate.as_ref().cloned() else {
            return Ok(json!({
                "active": false,
                "decision": "no_active_candidate",
                "quality_fingerprint": fingerprint,
                "repeat_count": repeat_count
            }));
        };

        if self.transactions.active_id() != Some(candidate_snapshot.transaction_id.as_str()) {
            self.active_candidate = None;
            return Ok(json!({
                "active": false,
                "decision": "candidate_transaction_mismatch",
                "transaction_id": candidate_snapshot.transaction_id,
                "quality_fingerprint": fingerprint,
                "repeat_count": repeat_count,
                "strategy_change_required": true
            }));
        }

        let decision = candidate_decision(
            candidate_snapshot.baseline.as_ref(),
            &current,
            result.success,
        );
        let delta = candidate_snapshot
            .baseline
            .as_ref()
            .map(|baseline| quality_delta(Some(baseline), &current))
            .unwrap_or_else(|| quality_delta(None, &current));
        let strategy_change_required = decision.should_rollback() || repeat_count > 1;

        if decision.should_rollback() {
            let rollback_id = self
                .transactions
                .rollback()
                .map_err(|error| error.to_string())?;
            if rollback_id.is_some() {
                self.source_revision = self.source_revision.saturating_add(1);
            }
            self.active_candidate = None;

            let restoration = match self
                .processes
                .run_project_operation(self.workspace.root(), ProjectOperation::Validate)
            {
                Ok(restored) => {
                    let restored_snapshot = quality_snapshot(&restored);
                    self.last_quality_snapshot = Some(restored_snapshot.clone());
                    self.record_verification("validate", &restored);
                    json!({
                        "verified": true,
                        "success": restored.success,
                        "quality": quality_snapshot_json(&restored_snapshot),
                        "matches_baseline": candidate_snapshot
                            .baseline
                            .as_ref()
                            .is_some_and(|baseline| baseline.fingerprints == restored_snapshot.fingerprints
                                && baseline.errors == restored_snapshot.errors
                                && baseline.warnings == restored_snapshot.warnings)
                    })
                }
                Err(error) => json!({
                    "verified": false,
                    "error": error
                }),
            };

            return Ok(json!({
                "active": false,
                "transaction_id": candidate_snapshot.transaction_id,
                "generation": candidate_snapshot.generation,
                "decision": decision.label(),
                "rolled_back": rollback_id.is_some(),
                "rollback_transaction_id": rollback_id,
                "mutations": candidate_snapshot.mutations,
                "validations": candidate_snapshot.validations.saturating_add(1),
                "delta": delta,
                "quality_fingerprint": fingerprint,
                "repeat_count": repeat_count,
                "strategy_change_required": strategy_change_required,
                "restoration": restoration,
                "next_action": "Begin a fresh candidate transaction only after changing repair strategy and, when required, refreshing dependency/API grounding."
            }));
        }

        if let Some(candidate) = self.active_candidate.as_mut() {
            candidate.validations = candidate.validations.saturating_add(1);
            candidate.last_validation_success = result.success;
            candidate.last_decision = decision.label();
        }

        Ok(json!({
            "active": true,
            "transaction_id": candidate_snapshot.transaction_id,
            "generation": candidate_snapshot.generation,
            "decision": decision.label(),
            "rolled_back": false,
            "mutations": candidate_snapshot.mutations,
            "validations": candidate_snapshot.validations.saturating_add(1),
            "delta": delta,
            "quality_fingerprint": fingerprint,
            "repeat_count": repeat_count,
            "strategy_change_required": strategy_change_required,
            "commit_ready": result.success,
            "next_action": if result.success {
                "Continue the remaining quality gate and commit only after required verification succeeds."
            } else {
                "The candidate improved but remains provisional. Continue repairing inside this candidate; do not commit yet."
            }
        }))
    }
    pub fn session(&self) -> &DevSession {
        &self.session
    }

    fn workspace_status(&mut self) -> Result<Value, String> {
        let cargo_toml = if self.workspace.root().join("Cargo.toml").is_file() {
            Some(
                self.workspace
                    .read_text("Cargo.toml", 1024 * 1024)
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        let manifest_path = self
            .workspace
            .resolve("SOURCE_MANIFEST.json")
            .map_err(|error| error.to_string())?;
        let source_manifest = if manifest_path.is_file() {
            serde_json::from_slice::<Value>(
                &std::fs::read(&manifest_path).map_err(|error| error.to_string())?,
            )
            .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        let cargo_layout = cargo_toml
            .as_deref()
            .map(cargo_workspace_summary)
            .unwrap_or_else(|| {
                json!({
                    "member_count": 0,
                    "members": [],
                    "apps": [],
                    "crates": [],
                    "generic_cortex_authorities": [],
                    "open2d_cortex_compatibility": [],
                    "reference_members": [],
                    "excluded": []
                })
            });
        let manifest_summary = source_manifest_summary(&source_manifest);
        let checkpoint = checkpoint_summary(self.workspace.root());
        let authority_audit = authority_audit_summary(
            &self
                .workspace
                .root()
                .join("logs")
                .join("diagnostics")
                .join("cortex-authority-r051.json"),
        );
        let non_workspace_apps = non_workspace_app_manifests(
            self.workspace.root(),
            cargo_layout
                .get("members")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        );
        let inspection_snapshot = json!({
            "workspace_root": self.workspace.root(),
            "workspace_profile": self.workspace.profile(),
            "cargo_workspace": cargo_layout,
            "non_workspace_app_manifests": non_workspace_apps,
            "source_manifest": manifest_summary,
            "checkpoint": checkpoint,
            "cortex_authority_audit": authority_audit,
            "authority_notes": {
                "cargo_members_are_active_build_graph": true,
                "non_workspace_app_manifests_are_not_active_members": true,
                "source_manifest_entries_are_inventory_not_automatically_runtime_authority": true
            }
        });
        let registry = WorkspaceRegistry::open_default().ok();
        let registered_workspaces = registry
            .as_ref()
            .and_then(|value| value.list().ok())
            .unwrap_or_default();
        let active_registered_workspace = registry
            .as_ref()
            .and_then(|value| value.active().ok().flatten());
        let mut adapter_status = serde_json::Map::new();
        if self.git.is_some() {
            adapter_status.insert("git".into(), Value::String("active".into()));
        }
        for extension in &self.extensions {
            adapter_status.insert(extension.id().to_string(), extension.status());
        }
        let project_topology = project_topology(&self.workspace)?;
        let dependency_grounding = self.dependency_grounding()?;
        let verification_authority = self.verification_authority_json();
        Ok(json!({
            "root": self.workspace.root(),
            "profile": self.workspace.profile(),
            "project_topology": project_topology,
            "dependency_grounding": dependency_grounding,
            "dependency_grounding_state": {
                "required": self.dependency_grounding_required,
                "required_packages": self.required_dependency_packages,
                "grounded_packages": self.grounded_dependency_packages
            },
            "verification_authority": verification_authority,
            "inspection_snapshot": inspection_snapshot,
            "cargo_toml": cargo_toml,
            "source_manifest": source_manifest,
            "active_transaction": self.transactions.active_id(),
            "session_id": self.session.id.clone(),
            "state_directory": self.workspace.cortex_state_dir(),
            "adapters": adapter_status,
            "workspace_registry": {
                "home": registry.as_ref().map(|value| value.home().to_path_buf()),
                "count": registered_workspaces.len(),
                "active": active_registered_workspace
            },
            "context": self.context.load_inventory()?,
            "context_index": self.context.load_index()?,
            "vault": self.vault.status(),
            "development": self.development.active()?,
            "permissions": self.permissions,
            "plugins": PluginRegistry::discover(
                self.workspace.root(),
                &self.workspace.cortex_state_dir()
            ).unwrap_or_default().len()
        }))
    }

    fn authorize_tool(&self, name: &str) -> Result<(), String> {
        self.permissions.require(permission_for_tool(name), name)
    }

    fn authorize_pcc_command(&self, key: &str) -> Result<PccClient, String> {
        let client = PccClient::discover(self.workspace.root())?;
        let descriptor = client.command_descriptor(key)?;
        let operation = format!("pcc command {key}");

        if let Some(risk) = descriptor.get("risk").and_then(Value::as_str) {
            match risk {
                "source_mutation" => self
                    .permissions
                    .require(Permission::WorkspaceWrite, &operation)?,
                "external_mutation" => self
                    .permissions
                    .require(Permission::ExternalPath, &operation)?,
                _ => {}
            }
        }

        for permission in descriptor
            .get("permissions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let permission = parse_permission(permission).ok_or_else(|| {
                format!("PCC command '{key}' declares unknown Cortex permission: {permission}")
            })?;
            self.permissions.require(permission, &operation)?;
        }

        for side_effect in descriptor
            .get("side_effects")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let permission = match side_effect {
                "source_files" | "workspace_files" => Some(Permission::WorkspaceWrite),
                "external_path" | "external_paths" => Some(Permission::ExternalPath),
                "git" | "git_write" => Some(Permission::GitWrite),
                "delete" | "deletes" => Some(Permission::Delete),
                "credentials" => Some(Permission::Credentials),
                "system" | "system_change" => Some(Permission::SystemChange),
                "internet" | "network_internet" => Some(Permission::NetworkInternet),
                _ => None,
            };
            if let Some(permission) = permission {
                self.permissions.require(permission, &operation)?;
            }
        }
        Ok(client)
    }

    fn require_rust_workspace(&self) -> Result<(), String> {
        if self.workspace.root().join("Cargo.toml").is_file() {
            Ok(())
        } else {
            Err("active workspace is not a Cargo/Rust workspace".into())
        }
    }

    fn record_development_command(
        &self,
        kind: &str,
        description: &str,
        result: &CommandResult,
    ) -> Result<(), String> {
        let Some(mut run) = self.development.active()? else {
            return Ok(());
        };
        run.add_evidence(
            kind,
            description,
            None,
            json!({
                "program": result.program,
                "args": result.args,
                "exit_code": result.exit_code,
                "success": result.success,
                "duration_ms": result.duration_ms,
                "diagnostics": result.diagnostics
            }),
        );
        if !result.success {
            let detail = if !result.stderr.trim().is_empty() {
                result.stderr.trim()
            } else if !result.stdout.trim().is_empty() {
                result.stdout.trim()
            } else {
                "project command failed without textual diagnostics"
            };
            run.record_failure(format!("{description}: {}", tail_chars(detail, 8_000)));
        }
        self.development.save(&run)
    }

    fn record_development_evidence(
        &self,
        kind: &str,
        description: &str,
        path: Option<PathBuf>,
        metadata: Value,
    ) -> Result<(), String> {
        let Some(mut run) = self.development.active()? else {
            return Ok(());
        };
        run.add_evidence(kind, description, path, metadata);
        self.development.save(&run)
    }

    fn call_vscode(&self, method: &str, params: Value) -> Result<Value, String> {
        let port = std::env::var("CORTEX_VSCODE_RPC_PORT")
            .or_else(|_| std::env::var("OPEN2D_VSCODE_RPC_PORT"))
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(7338);
        let id = format!("cortex-vscode-{}-{}", std::process::id(), unix_ms());
        let token_path = self.workspace.cortex_state_dir().join("rpc.token");
        let token = std::fs::read_to_string(&token_path).map_err(|e| {
            format!(
                "Cortex RPC token unavailable at {}: {e}",
                token_path.display()
            )
        })?;
        let response = rpc_call(
            default_address(port),
            &rpc_request_with_token(id, method, params, token.trim().to_string()),
        )?;
        if response.ok {
            Ok(response.result)
        } else {
            Err(response
                .error
                .unwrap_or_else(|| "VS Code bridge request failed".into()))
        }
    }

    // O2D-R051N9M10_EXECUTION_SPINE
    fn m10_normalize_call(&self, call: &ToolCall) -> Result<ToolCall, String> {
        if !call.name.starts_with("source.") {
            return Ok(call.clone());
        }
        let mut normalized = call.clone();
        if let Some(arguments) = normalized.arguments.as_object_mut() {
            let path = arguments
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Some(path) = path {
                arguments.insert(
                    "path".into(),
                    Value::String(canonical_project_relative(self.workspace.root(), path)?),
                );
            }
            if let Some(Value::Array(paths)) = arguments.get_mut("paths") {
                for value in paths {
                    let Some(path) = value.as_str().map(str::to_string) else {
                        continue;
                    };
                    *value =
                        Value::String(canonical_project_relative(self.workspace.root(), path)?);
                }
            }
        }
        Ok(normalized)
    }

    fn m10_is_mutating_source_tool(name: &str) -> bool {
        matches!(
            name,
            "source.write_text" | "source.replace_text" | "source.apply_patch"
        )
    }

    fn m10_grounding_query(call: &ToolCall, package: &str) -> String {
        let module = package.replace('-', "_");
        let needle = format!("{module}::");
        for field in ["content", "new", "old"] {
            let Some(text) = call.arguments.get(field).and_then(Value::as_str) else {
                continue;
            };
            let Some(index) = text.find(&needle) else {
                continue;
            };
            let tail = &text[index + needle.len()..];
            for token in
                tail.split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            {
                if !token.is_empty() && token != "self" && token != "crate" {
                    return token.to_string();
                }
            }
        }
        module
    }

    fn m11_live_repair_strategy(&self) -> cortex_universal::RepairStrategy {
        let errors = self
            .last_quality_snapshot
            .as_ref()
            .map(|snapshot| snapshot.errors)
            .unwrap_or_default();
        let mismatch = self
            .last_quality_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.likely_api_version_mismatch);
        cortex_universal::choose_repair_strategy(&cortex_universal::RepairSignal {
            likely_api_version_mismatch: mismatch,
            errors_from_same_dependency: if mismatch { errors } else { 0 },
            affected_files: 1,
            compact_module: mismatch && errors <= 64,
            missing_dependency_grounding: self
                .required_dependency_packages
                .difference(&self.grounded_dependency_packages)
                .cloned()
                .collect(),
        })
    }

    fn m10_ground_and_replay(
        &mut self,
        call: &ToolCall,
        first_error: &str,
    ) -> Result<Value, String> {
        let packages = parse_ground_before_write_packages(first_error);
        if packages.is_empty() {
            return Err(first_error.to_string());
        }

        // M11U-H3D: a mutation authored before dependency grounding is stale.
        // Ground exact local dependency evidence, persist it on the retained
        // ExecutionSession, and force regeneration instead of replaying guessed
        // source that was produced before the evidence existed.
        let execution_id = begin_grounding_for_workspace(self.workspace.root(), &packages)?;
        let mut failures = Vec::new();
        let mut evidence = Vec::<Value>::new();
        for package in &packages {
            let primary_query = Self::m10_grounding_query(call, package);
            let queries = if primary_query.eq_ignore_ascii_case(package)
                || primary_query.eq_ignore_ascii_case(&package.replace('-', "_"))
            {
                vec!["pub ".to_string(), package.replace('-', "_")]
            } else {
                vec![primary_query, "pub ".to_string()]
            };

            for (query_index, query) in queries.into_iter().enumerate() {
                let grounding = ToolCall {
                    call_id: format!("{}-m11-ground-{package}-{query_index}", call.call_id),
                    name: "source.search".into(),
                    arguments: json!({
                        "query": query,
                        "dependency_package": package,
                        "max_hits": 24
                    }),
                };
                let _ = record_tool_for_workspace(
                    self.workspace.root(),
                    "source.search",
                    package,
                    true,
                );
                match self.execute_inner_once(&grounding) {
                    Ok(value) => evidence.push(json!({
                        "package": package,
                        "query": query,
                        "result": value
                    })),
                    Err(ground_error) => {
                        failures.push(format!("{package} query `{query}`: {ground_error}"))
                    }
                }
                let _ = record_tool_for_workspace(
                    self.workspace.root(),
                    "source.search",
                    package,
                    false,
                );
            }
        }

        if !failures.is_empty() {
            return Err(format!(
                "Ground Before Write controller recovery could not ground the complete dependency set [{}]. {}",
                packages.join(", "),
                failures.join(" | ")
            ));
        }
        if let Some(execution_id) = execution_id.as_deref() {
            complete_grounding_for_workspace(self.workspace.root(), execution_id)?;
        }

        let evidence_text = compact_json_chars(&json!(evidence), 9_000);
        let strategy = self.m11_live_repair_strategy();
        let strategy_instruction = if matches!(
            strategy,
            cortex_universal::RepairStrategy::Reconstruction
        ) {
            "RECONSTRUCTION is required: re-read the affected compact module and regenerate it coherently from exact-version evidence instead of continuing piecemeal API substitutions."
        } else {
            "Regenerate the blocked mutation from the exact-version evidence before continuing."
        };
        Err(format!(
            "M11 grounded regeneration required. Exact local dependency evidence is now grounded for [{}]. The blocked API-sensitive mutation was NOT replayed because it was authored before that evidence existed. Controller repair strategy: {:?}. {} Do not reuse guessed APIs from the blocked payload.\n\nGROUNDED EVIDENCE:\n{}",
            packages.join(", "),
            strategy,
            strategy_instruction,
            evidence_text
        ))
    }

    fn controller_verify_after_source_mutation(&mut self) -> Result<Value, String> {
        let root = self.workspace.root().to_path_buf();

        let _ = record_tool_for_workspace(&root, "build.project_format", "", true);
        let format = self
            .processes
            .run_project_operation(&root, ProjectOperation::Format)?;
        self.record_development_command(
            "quality.controller_post_mutation_format",
            "Controller post-mutation formatting verification",
            &format,
        )?;
        self.record_verification("format", &format);
        let format_value = self.command_result_with_revision(&format)?;
        let _ = record_tool_result_for_workspace(&root, "build.project_format", "", format.success);

        let _ = record_tool_for_workspace(&root, "build.project_validate", "", true);
        let validate = self
            .processes
            .run_project_operation(&root, ProjectOperation::Validate)?;
        self.record_development_command(
            "quality.controller_post_mutation_validate",
            "Controller post-mutation project validation",
            &validate,
        )?;
        let quality = self.quality_result(&validate);
        self.record_verification("validate", &validate);
        let candidate = self.evaluate_active_candidate(&validate)?;
        let mut validate_value = self.command_result_with_revision(&validate)?;
        if let Value::Object(object) = &mut validate_value {
            object.insert("cortex_quality".into(), quality.clone());
            object.insert("cortex_candidate".into(), candidate.clone());
        }
        let _ =
            record_tool_result_for_workspace(&root, "build.project_validate", "", validate.success);

        Ok(json!({
            "authority": "m11u2_controller_post_mutation_verifier",
            "format": format_value,
            "validate": validate_value,
            "quality": quality,
            "candidate": candidate,
            "next_action": if validate.success {
                "Compiler validation is green for the current revision. Continue only with remaining requested quality/runtime acceptance."
            } else {
                "Compiler validation failed. The returned diagnostics are now the only repair authority; do not make another source mutation until required dependency grounding is satisfied."
            }
        }))
    }

    fn execute_inner(&mut self, call: &ToolCall) -> Result<Value, String> {
        let call = self.m10_normalize_call(call)?;
        let target = call
            .arguments
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        cortex_execution::spine::stage_guard_for_workspace(
            self.workspace.root(),
            &call.name,
            &target,
        )?;
        let _ = record_tool_for_workspace(self.workspace.root(), &call.name, &target, true);
        let first = self.execute_inner_once(&call);
        let mut result = match first {
            Err(error) if Self::m10_is_mutating_source_tool(&call.name) => {
                self.m10_ground_and_replay(&call, &error)
            }
            other => other,
        };
        if let Ok(output) = &result {
            if call.name == "source.begin_transaction" {
                if let Some(transaction_id) = output.get("transaction_id").and_then(Value::as_str) {
                    let _ = claim_transaction_for_workspace(self.workspace.root(), transaction_id);
                }
            }
        }

        // M11U2: source mutation and compiler feedback are one controller action.
        // This removes the model's ability to perform long speculative edit chains
        // before seeing whether the previous edit actually improved the project.
        let mut outer_tool_result_recorded = false;
        if Self::m10_is_mutating_source_tool(&call.name) {
            if let Ok(mutation) = result.clone() {
                // Record mutation completion before the controller runs the compiler so
                // the final visible Live Run phase remains the verification phase.
                let _ = cortex_execution::spine::record_tool_result_for_workspace(
                    self.workspace.root(),
                    &call.name,
                    &target,
                    true,
                );
                outer_tool_result_recorded = true;
                result = match self.controller_verify_after_source_mutation() {
                    Ok(verification) => Ok(json!({
                        "mutation": mutation,
                        "controller_verification": verification,
                        "instruction": "The controller already formatted/validated this mutation. Use the returned compiler diagnostics before any further source change."
                    })),
                    Err(error) => Err(format!(
                        "Source mutation completed, but mandatory M11U2 post-mutation verification could not run: {error}"
                    )),
                };
            }
        }

        if !outer_tool_result_recorded {
            let _ = cortex_execution::spine::record_tool_result_for_workspace(
                self.workspace.root(),
                &call.name,
                &target,
                result.is_ok(),
            );
        }
        result
    }

    fn execute_inner_once(&mut self, call: &ToolCall) -> Result<Value, String> {
        // M10A9_FINAL_TOOL_GATE
        // Ground-Before-Write's internal dependency searches are allowed while
        // the retained execution is explicitly in Grounding. Every other real
        // tool execution must pass the semantic-progress authority here.
        let m10a9_target = call
            .arguments
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        cortex_execution::spine::authorize_tool_attempt_for_workspace(
            self.workspace.root(),
            &call.name,
            m10a9_target,
        )?;
        self.authorize_tool(&call.name)?;
        match call.name.as_str() {
            "workspace.status" | "project.status" => self.workspace_status(),
            "workspace.context_status" => Ok(json!({
                "inventory": self.context.load_inventory()?,
                "path": self.context.inventory_path()
            })),
            "workspace.registry_list" => {
                let registry = WorkspaceRegistry::open_default()?;
                Ok(json!({
                    "home": registry.home(),
                    "active": registry.active()?,
                    "workspaces": registry.list()?
                }))
            }
            "workspace.storage_status" => {
                let registry = WorkspaceRegistry::open_default()?;
                Ok(json!({
                    "authority":"cortex_storage_authority",
                    "home":registry.home(),
                    "configured":registry.library_root()?,
                    "layout":registry.library_layout()?,
                    "offsite_backup_root":registry.offsite_backup_root()?,
                    "eligible_volumes":registry.storage_volumes(false).unwrap_or_default()
                }))
            }
            "workspace.storage_volumes" => {
                let registry = WorkspaceRegistry::open_default()?;
                let include_removable = call
                    .arguments
                    .get("include_removable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                Ok(json!({
                    "authority":"cortex_storage_volumes",
                    "include_removable":include_removable,
                    "volumes":registry.storage_volumes(include_removable)?
                }))
            }
            "workspace.storage_set" => {
                let registry = WorkspaceRegistry::open_default()?;
                let root = required_string(&call.arguments, "root")?;
                let layout = registry.provision_library_root(root)?;
                Ok(json!({
                    "authority":"cortex_storage_authority",
                    "configured":registry.library_root()?,
                    "layout":layout,
                    "note":"Changing the configured authority does not migrate an existing live Cortex installation. Use the dedicated storage-authority migration workflow for that future operation."
                }))
            }
            "workspace.offsite_backup_set" => {
                let registry = WorkspaceRegistry::open_default()?;
                let root = required_string(&call.arguments, "root")?;
                let record = if root.trim().is_empty() {
                    registry.clear_offsite_backup_root()?
                } else {
                    registry.set_offsite_backup_root(root)?
                };
                Ok(json!({
                    "authority":"cortex_offsite_backup_root",
                    "record":record,
                    "cloud_upload_confirmed":false,
                    "note":"Cortex verifies the local synchronized-folder copy only. The sync provider remains authoritative for remote cloud upload completion."
                }))
            }
            "workspace.scan_storage" => {
                let registry = WorkspaceRegistry::open_default()?;
                let max_directories = arg_u64(&call.arguments, "max_directories")
                    .unwrap_or(250_000)
                    .clamp(1, 2_000_000) as usize;
                let max_files = arg_u64(&call.arguments, "max_files")
                    .unwrap_or(2_000_000)
                    .clamp(1, 20_000_000) as usize;
                Ok(serde_json::to_value(
                    registry.scan_configured_storage(max_directories, max_files)?,
                )
                .map_err(|error| error.to_string())?)
            }
            "workspace.scan_machine" => {
                let registry = WorkspaceRegistry::open_default()?;
                let include_removable = call
                    .arguments
                    .get("include_removable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let max_directories = arg_u64(&call.arguments, "max_directories")
                    .unwrap_or(1_000_000)
                    .clamp(1, 4_000_000) as usize;
                let max_files = arg_u64(&call.arguments, "max_files")
                    .unwrap_or(8_000_000)
                    .clamp(1, 40_000_000) as usize;
                Ok(serde_json::to_value(registry.scan_machine_catalog(
                    include_removable,
                    max_directories,
                    max_files,
                )?)
                .map_err(|error| error.to_string())?)
            }
            "workspace.migration_plan" => {
                let registry = WorkspaceRegistry::open_default()?;
                let project_id = required_string(&call.arguments, "project_id")?;
                Ok(
                    serde_json::to_value(registry.propose_managed_migration(project_id)?)
                        .map_err(|error| error.to_string())?,
                )
            }
            "workspace.migration_apply" => {
                let registry = WorkspaceRegistry::open_default()?;
                let plan_id = required_string(&call.arguments, "plan_id")?;
                Ok(
                    serde_json::to_value(registry.apply_managed_migration(plan_id)?)
                        .map_err(|error| error.to_string())?,
                )
            }
            "workspace.context_rebuild" => {
                let max_files = arg_u64(&call.arguments, "max_files")
                    .unwrap_or(50_000)
                    .clamp(1, 250_000) as usize;
                let inventory = self.context.rebuild_inventory(max_files)?;
                Ok(json!({
                    "inventory": inventory,
                    "path": self.context.inventory_path()
                }))
            }
            "workspace.index_status" => Ok(json!({
                "index": self.context.load_index()?,
                "path": self.context.index_path()
            })),
            "workspace.index_rebuild" => {
                let max_files = arg_u64(&call.arguments, "max_files")
                    .unwrap_or(50_000)
                    .clamp(1, 250_000) as usize;
                let max_hash_bytes = arg_u64(&call.arguments, "max_hash_bytes")
                    .unwrap_or(512 * 1024)
                    .clamp(4 * 1024, 8 * 1024 * 1024) as usize;

                let mut job = self
                    .jobs
                    .create("context_index", "Rebuild workspace context index")?;
                self.jobs.progress(
                    &mut job,
                    0,
                    None,
                    "scanning workspace metadata and content fingerprints",
                )?;
                self.events.append(
                    EventKind::Context,
                    "Context index",
                    "agent tool rebuild started",
                    None,
                    json!({
                        "job_id": job.id.clone(),
                        "max_files": max_files,
                        "max_hash_bytes": max_hash_bytes
                    }),
                )?;

                match self.context.rebuild_index(max_files, max_hash_bytes) {
                    Ok((index, delta)) => {
                        let result = json!({
                            "index": index,
                            "delta": delta,
                            "path": self.context.index_path()
                        });
                        self.jobs.update(
                            &mut job,
                            JobStatus::Succeeded,
                            "context index rebuilt",
                            result.clone(),
                        )?;
                        self.events.append(
                            EventKind::Context,
                            "Context index",
                            "agent tool rebuild completed",
                            Some(true),
                            json!({
                                "job_id": job.id.clone(),
                                "new_files": delta.new_files.len(),
                                "modified_files": delta.modified_files.len(),
                                "deleted_files": delta.deleted_files.len(),
                                "unchanged_files": delta.unchanged_files,
                                "hashed_files": delta.hashed_files,
                                "reused_hashes": delta.reused_hashes
                            }),
                        )?;
                        Ok(json!({"job": job, "result": result}))
                    }
                    Err(error) => {
                        self.jobs.update(
                            &mut job,
                            JobStatus::Failed,
                            &error,
                            json!({"path": self.context.index_path()}),
                        )?;
                        self.events.append(
                            EventKind::Error,
                            "Context index failed",
                            &error,
                            Some(false),
                            json!({"job_id": job.id.clone()}),
                        )?;
                        Err(error)
                    }
                }
            }
            "source.list" => {
                let requested = arg_string(&call.arguments, "path").unwrap_or_default();
                let (path, resolution) = if requested.is_empty() {
                    (String::new(), None)
                } else {
                    let (p, r) = self.canonical_source_path(&requested, false)?;
                    (p, Some(r))
                };
                let max = arg_u64(&call.arguments, "max_files")
                    .unwrap_or(120)
                    .clamp(1, 2_000) as usize;
                let files = self
                    .workspace
                    .list_files(&path, max)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"files":files,"path_resolution":resolution}))
            }
            "source.read" => {
                let requested = required_string(&call.arguments, "path")?;
                let (path, resolution) = self.canonical_source_path(requested, false)?;
                let offset = arg_u64(&call.arguments, "offset").unwrap_or(0) as usize;
                let max = arg_u64(&call.arguments, "max_bytes")
                    .unwrap_or(64 * 1024)
                    .clamp(1, 256 * 1024) as usize;
                let window = self
                    .workspace
                    .read_text_window(&path, offset, max)
                    .map_err(|e| e.to_string())?;
                Ok(
                    json!({"text":window.text,"offset":window.offset,"next_offset":window.next_offset,"total_bytes":window.total_bytes,"truncated":window.truncated,"path_resolution":resolution}),
                )
            }
            "dependency.ground" => {
                let package = required_string(&call.arguments, "package")?;
                let ecosystem =
                    arg_string(&call.arguments, "ecosystem").unwrap_or_else(|| "cargo".into());
                let max_hits = arg_u64(&call.arguments, "max_hits")
                    .unwrap_or(40)
                    .clamp(1, 100) as usize;
                let result = dependency_api_search(
                    self.workspace.root(),
                    Some(&self.processes),
                    &ecosystem,
                    package,
                    "pub ",
                    max_hits,
                )?;
                let canonical = canonical_dependency_name(self.workspace.root(), package)
                    .unwrap_or_else(|| package.to_string());
                self.grounded_dependency_packages.insert(canonical.clone());
                let _ = record_grounded_dependency_for_workspace(self.workspace.root(), &canonical);
                let remaining_required_dependency_packages = self
                    .required_dependency_packages
                    .difference(&self.grounded_dependency_packages)
                    .cloned()
                    .collect::<Vec<_>>();
                let dependency_grounding_complete =
                    remaining_required_dependency_packages.is_empty();
                Ok(json!({
                    "authority": "exact_local_dependency_source",
                    "grounding_recorded": true,
                    "grounded_package": canonical,
                    "ecosystem": ecosystem,
                    "evidence": result,
                    "remaining_required_dependency_packages": remaining_required_dependency_packages,
                    "dependency_grounding_complete": dependency_grounding_complete,
                    "instruction": "Exact local dependency source is grounded. Use source.search with dependency_package for narrower symbol evidence when the repair needs a specific API detail."
                }))
            }
            "dependency.ensure" => {
                self.require_rust_workspace()?;
                let package = required_string(&call.arguments, "package")?;
                let explicit_version = arg_string(&call.arguments, "version");
                let (version, version_authority) = if let Some(version) = explicit_version {
                    (version, "explicit_controller_request")
                } else {
                    let versions = locally_installed_cargo_versions(package);
                    match versions.as_slice() {
                        [version] => (format!("={version}"), "unique_local_cargo_source"),
                        [] => {
                            return Err(format!(
                                "dependency.ensure cannot resolve `{package}` deterministically: no locally installed Cargo source version is available"
                            ))
                        }
                        _ => {
                            return Err(format!(
                                "dependency.ensure cannot resolve `{package}` deterministically: multiple locally installed versions are available: {}",
                                versions.join(", ")
                            ))
                        }
                    }
                };
                if self.transactions.active_id().is_none() {
                    return Err("dependency.ensure requires an active Cortex source transaction so Cargo.toml/Cargo.lock remain rollback-safe".into());
                }
                self.transactions
                    .checkpoint_path("Cargo.toml")
                    .map_err(|error| error.to_string())?;
                if self.workspace.root().join("Cargo.lock").is_file() {
                    self.transactions
                        .checkpoint_path("Cargo.lock")
                        .map_err(|error| error.to_string())?;
                }
                let manifest = fs::read_to_string(self.workspace.root().join("Cargo.toml"))
                    .map_err(|error| error.to_string())?;
                let (updated, changed) =
                    ensure_cargo_dependency_text(&manifest, package, &version)?;
                if changed {
                    self.transactions
                        .write_text("Cargo.toml", &updated)
                        .map_err(|error| error.to_string())?;
                    self.note_source_mutation("Cargo.toml");
                }
                Ok(json!({
                    "package": package,
                    "version": version,
                    "version_authority": version_authority,
                    "changed": changed,
                    "transaction_id": self.transactions.active_id(),
                    "instruction": "The dependency declaration is transactional. Run the project quality/checkpoint authority to resolve Cargo.lock, then ground the exact resolved package before API-sensitive source mutation."
                }))
            }
            "source.search" => {
                let query = required_string(&call.arguments, "query")?;
                let max_hits = arg_u64(&call.arguments, "max_hits")
                    .unwrap_or(50)
                    .clamp(1, 250) as usize;
                if let Some(package) = arg_string(&call.arguments, "dependency_package") {
                    let ecosystem =
                        arg_string(&call.arguments, "ecosystem").unwrap_or_else(|| "cargo".into());
                    let mut result = dependency_api_search(
                        self.workspace.root(),
                        Some(&self.processes),
                        &ecosystem,
                        &package,
                        query,
                        max_hits,
                    )?;
                    let canonical = canonical_dependency_name(self.workspace.root(), &package)
                        .unwrap_or_else(|| package.clone());
                    self.grounded_dependency_packages.insert(canonical.clone());
                    let _ =
                        record_grounded_dependency_for_workspace(self.workspace.root(), &canonical);
                    let remaining_required_dependency_packages = self
                        .required_dependency_packages
                        .difference(&self.grounded_dependency_packages)
                        .cloned()
                        .collect::<Vec<_>>();
                    let dependency_grounding_complete =
                        remaining_required_dependency_packages.is_empty();
                    if let Value::Object(object) = &mut result {
                        object.insert("grounding_recorded".into(), Value::Bool(true));
                        object.insert("grounded_package".into(), Value::String(canonical));
                        object.insert(
                            "grounded_dependency_packages".into(),
                            json!(self.grounded_dependency_packages),
                        );
                        object.insert(
                            "remaining_required_dependency_packages".into(),
                            json!(remaining_required_dependency_packages),
                        );
                        object.insert(
                            "dependency_grounding_complete".into(),
                            Value::Bool(dependency_grounding_complete),
                        );
                    }
                    return Ok(result);
                }
                let requested_roots = call
                    .arguments
                    .get("roots")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let mut roots = Vec::new();
                let mut resolutions = Vec::new();
                for requested in requested_roots {
                    let (path, resolution) = self.canonical_source_path(&requested, false)?;
                    roots.push(PathBuf::from(path));
                    resolutions.push(resolution);
                }
                let hits = self
                    .workspace
                    .search_text(query, &roots, max_hits)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"hits":hits,"root_resolutions":resolutions}))
            }
            "source.transaction_status" => Ok(json!({
                "transaction": self.transactions.active_summary(),
                "candidate": self.candidate_status_json()
            })),
            "source.transaction_files" => {
                let summary = self.transactions.active_summary();
                Ok(json!({
                    "transaction_id": summary.as_ref().map(|value| value.id.clone()),
                    "touched": summary.as_ref().map(|value| value.touched.clone()).unwrap_or_default(),
                    "created": summary.as_ref().map(|value| value.created.clone()).unwrap_or_default(),
                    "candidate": self.candidate_status_json()
                }))
            }
            "source.checkpoint" => {
                let requested = required_string(&call.arguments, "path")?;
                let (path, resolution) = self.canonical_source_path(requested, false)?;
                self.transactions
                    .checkpoint_path(&path)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"checkpointed":path,"path_resolution":resolution}))
            }
            "source.begin_transaction" => {
                if let Some(transaction_id) = self.transactions.active_id().map(str::to_string) {
                    return Ok(json!({
                        "transaction_id": transaction_id,
                        "reused": true,
                        "candidate": self.candidate_status_json(),
                        "dependency_grounding_required": self.dependency_grounding_required,
                        "controller_repair_strategy": format!("{:?}", self.m11_live_repair_strategy()),
                        "instruction": "A Cortex source transaction is already active. Reuse this transaction instead of attempting to open a nested transaction."
                    }));
                }
                let label =
                    arg_string(&call.arguments, "label").unwrap_or_else(|| "agent-edit".into());
                let (baseline, baseline_evidence) = self.capture_candidate_baseline();
                let id = self.transactions.begin(&label).map_err(|e| e.to_string())?;
                self.candidate_generation = self.candidate_generation.saturating_add(1);
                self.active_candidate = Some(CandidateRepairState {
                    transaction_id: id.clone(),
                    generation: self.candidate_generation,
                    baseline_origin: if baseline.is_some() {
                        "fresh_project_validate"
                    } else {
                        "unavailable"
                    },
                    baseline,
                    mutations: 0,
                    validations: 0,
                    last_validation_success: false,
                    last_decision: "baseline_captured",
                });
                Ok(json!({
                    "transaction_id": id,
                    "candidate_generation": self.candidate_generation,
                    "quality_baseline": baseline_evidence,
                    "dependency_grounding_required": self.dependency_grounding_required,
                    "controller_repair_strategy": format!("{:?}", self.m11_live_repair_strategy()),
                    "instruction": "All source mutations in this transaction are provisional until project validation proves the candidate is non-regressing. When the controller selects Reconstruction, rebuild the compact affected module coherently from grounded evidence rather than patching individual API errors."
                }))
            }
            "source.write_text" => {
                let requested = required_string(&call.arguments, "path")?;
                let (path, resolution) = self.canonical_source_path(requested, true)?;
                let content = required_string(&call.arguments, "content")?;
                if !path.eq_ignore_ascii_case("Cargo.toml")
                    && !path.ends_with("/Cargo.toml")
                    && !path.ends_with("\\Cargo.toml")
                {
                    self.ensure_mutation_dependency_grounding(content)?;
                }
                let (content, normalized_escaped_multiline) =
                    normalize_multiline_text_argument(content);
                let is_cargo_manifest = path.eq_ignore_ascii_case("Cargo.toml")
                    || path.ends_with("/Cargo.toml")
                    || path.ends_with("\\Cargo.toml");
                if is_cargo_manifest
                    && !content.contains('\n')
                    && content.matches("\\n").count() >= 2
                {
                    return Err("refusing to write Cargo.toml with literal backslash-n separators; emit decoded TOML text with real newlines".into());
                }
                self.transactions
                    .write_text(&path, &content)
                    .map_err(|e| e.to_string())?;
                self.note_source_mutation(&path);
                Ok(json!({
                    "written":true,
                    "path":path,
                    "normalized_escaped_multiline":normalized_escaped_multiline,
                    "path_resolution":resolution,
                    "candidate":self.candidate_status_json()
                }))
            }
            "source.replace_text" => {
                let requested = required_string(&call.arguments, "path")?;
                let (path, resolution) = self.canonical_source_path(requested, true)?;
                let old = required_string(&call.arguments, "old")?;
                let new = required_string(&call.arguments, "new")?;
                if !path.eq_ignore_ascii_case("Cargo.toml")
                    && !path.ends_with("/Cargo.toml")
                    && !path.ends_with("\\Cargo.toml")
                {
                    self.ensure_mutation_dependency_grounding(&format!("{old}\n{new}"))?;
                }
                let expected =
                    arg_u64(&call.arguments, "expected_occurrences").unwrap_or(1) as usize;
                let replaced = self
                    .transactions
                    .replace_text(&path, old, new, expected)
                    .map_err(|e| e.to_string())?;
                self.note_source_mutation(&path);
                Ok(json!({
                    "replaced":replaced,
                    "path":path,
                    "path_resolution":resolution,
                    "candidate":self.candidate_status_json()
                }))
            }
            "source.commit" => {
                if let Some(candidate) = &self.active_candidate {
                    if candidate.baseline.is_some()
                        && candidate.mutations > 0
                        && !candidate.last_validation_success
                    {
                        return Err(format!(
                            "candidate generation {} cannot be committed before successful project validation; current decision is `{}`",
                            candidate.generation, candidate.last_decision
                        ));
                    }
                }
                let candidate = self.candidate_status_json();
                let id = self.transactions.commit().map_err(|e| e.to_string())?;
                let committed = id.is_some();
                if committed {
                    self.active_candidate = None;
                }
                Ok(json!({
                    "transaction_id": id,
                    "committed": committed,
                    "candidate": candidate
                }))
            }
            "source.rollback" => {
                let candidate = self.candidate_status_json();
                let id = self.transactions.rollback().map_err(|e| e.to_string())?;
                let rolled_back = id.is_some();
                if rolled_back {
                    self.source_revision = self.source_revision.saturating_add(1);
                    self.active_candidate = None;
                }
                Ok(json!({
                    "transaction_id": id,
                    "rolled_back": rolled_back,
                    "candidate": candidate
                }))
            }
            "toolchain.status" => Ok(json!(inspect_local_toolchain())),
            "project.profile" => Ok(json!(detect_project_profile(self.workspace.root()))),
            "development.roadmap" => {
                let discovery = discover_roadmap(self.workspace.root())?;
                let roadmap = discovery.roadmap.clone().unwrap_or_else(|| {
                    let name = self
                        .workspace
                        .root()
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("project");
                    proposed_roadmap(name)
                });
                let requires_approval = roadmap.generated;
                Ok(json!({
                    "discovery": discovery,
                    "roadmap": roadmap,
                    "requires_approval": requires_approval
                }))
            }
            "development.run_status" => Ok(json!({"active": self.development.active()?})),
            "development.run_begin" => {
                let discovery = discover_roadmap(self.workspace.root())?;
                let roadmap = discovery.roadmap.unwrap_or_else(|| {
                    let name = self
                        .workspace
                        .root()
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("project");
                    proposed_roadmap(name)
                });
                if roadmap.generated
                    && !call
                        .arguments
                        .get("approve_generated_roadmap")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                {
                    return Err("no authoritative roadmap was found; inspect development.roadmap and explicitly approve the generated milestone plan before beginning".into());
                }
                let run = self.development.begin(self.workspace.root(), &roadmap)?;
                self.events.append(
                    EventKind::Job,
                    "Development milestone",
                    format!("started {} — {}", run.milestone_id, run.milestone_title),
                    None,
                    json!({"development_run_id": run.id}),
                )?;
                Ok(json!({"run": run, "roadmap": roadmap}))
            }
            "development.run_advance" => {
                let Some(mut run) = self.development.active()? else {
                    return Ok(json!({
                        "active": false,
                        "advanced": false,
                        "reason": "no_active_development_run",
                        "instruction": "This request is not owned by a roadmap development run; project-quality repair may continue without advancing a milestone run."
                    }));
                };
                if let Some(error) = arg_string(&call.arguments, "failure") {
                    let repair_available = run.record_failure(error);
                    self.development.save(&run)?;
                    Ok(json!({"run": run, "repair_available": repair_available}))
                } else {
                    run.advance();
                    self.development.save(&run)?;
                    Ok(json!({"run": run}))
                }
            }
            "development.milestone_complete" => {
                let run = self
                    .development
                    .active()?
                    .ok_or_else(|| "no active Cortex development run".to_string())?;
                let next = self.development.complete_and_begin_next(&run)?;
                Ok(json!({
                    "completed_milestone": run.milestone_id,
                    "next_run": next,
                    "roadmap": self.development.roadmap()?
                }))
            }
            "development.certification_plan" => {
                let discovery = discover_roadmap(self.workspace.root())?;
                let roadmap = discovery.roadmap.unwrap_or_else(|| {
                    let name = self
                        .workspace
                        .root()
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("project");
                    proposed_roadmap(name)
                });
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
                    .ok_or_else(|| "roadmap has no certifiable milestone".to_string())?;
                let profile = detect_project_profile(self.workspace.root());
                Ok(json!({"plan": certification_plan(milestone, &profile), "profile": profile}))
            }
            "build.project_checkpoint" => {
                let profile = detect_project_profile(self.workspace.root());
                let authority = profile.checkpoint_authority.clone();
                if !matches!(authority.kind, CheckpointAuthorityKind::ProjectNative) {
                    return Err("this project has no project-native checkpoint; use the generated quality profile stages instead".into());
                }
                let program = authority.program.clone().ok_or_else(|| {
                    "project-native checkpoint has no executable program".to_string()
                })?;
                let args = authority
                    .args
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                let result =
                    self.processes
                        .run_capture(self.workspace.root(), &program, &args, false)?;
                self.record_development_command(
                    "quality.checkpoint",
                    "Project-native checkpoint",
                    &result,
                )?;
                self.record_verification("checkpoint", &result);
                if let Some(candidate) = self.active_candidate.as_mut() {
                    candidate.validations = candidate.validations.saturating_add(1);
                    candidate.last_validation_success = result.success;
                    candidate.last_decision = if result.success {
                        "checkpoint_verified"
                    } else {
                        "checkpoint_failed"
                    };
                }
                let checkpoint_log = authority
                    .log_path
                    .as_ref()
                    .map(|path| self.workspace.root().join(path))
                    .filter(|path| path.is_file())
                    .and_then(|path| fs::read_to_string(path).ok())
                    .map(|text| tail_chars(&text, 128 * 1024));
                let mut value = self.command_result_with_revision(&result)?;
                if let Value::Object(object) = &mut value {
                    object.insert("checkpoint_authority".into(), json!(authority));
                    object.insert("checkpoint_log".into(), json!(checkpoint_log));
                    object.insert(
                        "quality_authority".into(),
                        Value::String("project_checkpoint".into()),
                    );
                }
                Ok(value)
            }
            "build.project_format" => {
                let result = self
                    .processes
                    .run_project_operation(self.workspace.root(), ProjectOperation::Format)?;
                self.record_development_command(
                    "quality.format",
                    "Project formatting verification",
                    &result,
                )?;
                self.record_verification("format", &result);
                self.command_result_with_revision(&result)
            }
            "build.project_validate" => {
                let result = self
                    .processes
                    .run_project_operation(self.workspace.root(), ProjectOperation::Validate)?;
                self.record_development_command(
                    "quality.validate",
                    "Project validation/check",
                    &result,
                )?;
                let quality = self.quality_result(&result);
                self.record_verification("validate", &result);
                let candidate = self.evaluate_active_candidate(&result)?;
                let mut value = self.command_result_with_revision(&result)?;
                if let Value::Object(object) = &mut value {
                    object.insert("cortex_quality".into(), quality);
                    object.insert("cortex_candidate".into(), candidate);
                }
                Ok(value)
            }
            "build.project_test" => {
                let result = self
                    .processes
                    .run_project_operation(self.workspace.root(), ProjectOperation::Test)?;
                self.record_development_command("quality.test", "Project test", &result)?;
                self.record_verification("test", &result);
                self.command_result_with_revision(&result)
            }
            "build.project_lint" => {
                let result = self
                    .processes
                    .run_project_operation(self.workspace.root(), ProjectOperation::Lint)?;
                self.record_development_command(
                    "quality.lint",
                    "Project lint/static analysis",
                    &result,
                )?;
                self.record_verification("lint", &result);
                self.command_result_with_revision(&result)
            }
            "build.project_build" => {
                let result = self
                    .processes
                    .run_project_operation(self.workspace.root(), ProjectOperation::Build)?;
                self.record_development_command("quality.build", "Project build", &result)?;
                self.record_verification("build", &result);
                self.command_result_with_revision(&result)
            }
            "runtime.launch_project" => {
                let plan = cortex_universal::plan_project(self.workspace.root(), true);
                let runtime = plan.runtime.clone().ok_or_else(|| {
                    format!(
                        "Cortex could not resolve a runnable artifact for the detected project profile: {:?}",
                        plan.profile
                    )
                })?;
                let mut args = runtime.args.clone();
                if let Some(extra) = call.arguments.get("args").and_then(Value::as_array) {
                    args.extend(extra.iter().filter_map(Value::as_str).map(str::to_string));
                }
                let (pid, executable) = spawn_universal_runtime(
                    &mut self.processes,
                    self.workspace.root(),
                    "Cortex project runtime",
                    &runtime,
                    &args,
                )?;
                self.record_development_evidence(
                    "runtime.launch",
                    "Built project runtime launched under Cortex ownership",
                    executable.clone(),
                    json!({
                        "pid": pid,
                        "program": runtime.program.clone(),
                        "args": args,
                        "expected_window": runtime.expected_window,
                        "evidence": runtime.evidence.clone(),
                    }),
                )?;
                Ok(json!({
                    "pid": pid,
                    "executable": executable,
                    "program": runtime.program.clone(),
                    "args": args,
                    "expected_window": runtime.expected_window,
                    "evidence": runtime.evidence.clone(),
                }))
            }
            "runtime.verify_project" => {
                let minimum_alive_ms = arg_u64(&call.arguments, "minimum_alive_ms")
                    .unwrap_or(1_500)
                    .clamp(100, 10_000);
                let keep_running = call
                    .arguments
                    .get("keep_running")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let require_title_change = call
                    .arguments
                    .get("require_title_change")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let title_sample_interval_ms = arg_u64(&call.arguments, "title_sample_interval_ms")
                    .unwrap_or(1_200)
                    .clamp(250, 5_000);

                let plan = cortex_universal::plan_project(self.workspace.root(), true);
                let runtime = plan.runtime.clone().ok_or_else(|| {
                    format!(
                        "the detected project profile has no resolvable runtime artifact: {:?}",
                        plan.profile
                    )
                })?;
                let require_window = call
                    .arguments
                    .get("require_window")
                    .and_then(Value::as_bool)
                    .unwrap_or(runtime.expected_window);

                let args = runtime.args.clone();
                let (pid, executable) = spawn_universal_runtime(
                    &mut self.processes,
                    self.workspace.root(),
                    "Cortex runtime acceptance",
                    &runtime,
                    &args,
                )?;

                std::thread::sleep(Duration::from_millis(minimum_alive_ms));
                let statuses = self.processes.statuses();
                let running = statuses.iter().any(|status| {
                    status.pid == pid && matches!(status.state, OwnedProcessState::Running)
                });
                let first_window = if running {
                    self.capture.inspect_process_window(pid).ok()
                } else {
                    None
                };
                let first_title = first_window.as_ref().map(|window| window.title.clone());
                let second_window = if running && require_title_change {
                    std::thread::sleep(Duration::from_millis(title_sample_interval_ms));
                    self.capture.inspect_process_window(pid).ok()
                } else {
                    None
                };
                let second_title = second_window.as_ref().map(|window| window.title.clone());
                let title_changed = if require_title_change {
                    matches!(
                        (&first_title, &second_title),
                        (Some(first), Some(second)) if !first.is_empty() && first != second
                    )
                } else {
                    true
                };
                let window_ok = !require_window
                    || first_window.as_ref().is_some_and(|window| {
                        window.has_window && window.visible && window.responsive
                    });
                let success = running && window_ok && title_changed;
                if !success || !keep_running {
                    let _ = self.processes.stop(pid);
                }
                self.record_development_evidence(
                    "runtime.acceptance",
                    if success {
                        "Runtime smoke acceptance passed"
                    } else {
                        "Runtime smoke acceptance failed"
                    },
                    executable.clone(),
                    json!({
                        "pid": pid,
                        "program": runtime.program.clone(),
                        "args": args,
                        "minimum_alive_ms": minimum_alive_ms,
                        "running": running,
                        "require_window": require_window,
                        "require_title_change": require_title_change,
                        "title_sample_interval_ms": title_sample_interval_ms,
                        "first_window": first_window,
                        "second_window": second_window,
                        "title_changed": title_changed,
                        "expected_window": runtime.expected_window,
                        "evidence": runtime.evidence.clone(),
                        "success": success
                    }),
                )?;
                Ok(json!({
                    "success": success,
                    "pid": pid,
                    "executable": executable,
                    "program": runtime.program.clone(),
                    "args": args,
                    "minimum_alive_ms": minimum_alive_ms,
                    "kept_running": success && keep_running,
                    "running": running,
                    "require_window": require_window,
                    "require_title_change": require_title_change,
                    "first_window": first_window,
                    "second_window": second_window,
                    "title_changed": title_changed,
                    "expected_window": runtime.expected_window,
                    "evidence": runtime.evidence.clone(),
                    "statuses": statuses
                }))
            }
            "runtime.process_status" => Ok(json!({
                "processes": self.processes.statuses(),
                "records": self.processes.managed_records()
            })),
            "runtime.window_info" => {
                let pid = required_u64(&call.arguments, "pid")? as u32;
                let inspection = self.capture.inspect_process_window(pid)?;
                self.record_development_evidence(
                    "runtime.window",
                    "Native runtime window inspection",
                    None,
                    serde_json::to_value(&inspection).map_err(|e| e.to_string())?,
                )?;
                Ok(serde_json::to_value(inspection).map_err(|e| e.to_string())?)
            }
            "runtime.process_stop" => {
                let pid = required_u64(&call.arguments, "pid")? as u32;
                Ok(json!({"pid": pid, "stopped": self.processes.stop(pid)?}))
            }
            "build.cargo_fmt_check" => {
                self.require_rust_workspace()?;
                let result = self.processes.run_capture(
                    self.workspace.root(),
                    "cargo",
                    &["fmt", "--all", "--", "--check"],
                    false,
                )?;
                Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
            }
            "build.cargo_check" => {
                self.require_rust_workspace()?;
                let result = self.processes.run_cargo(
                    self.workspace.root(),
                    &["check", "--workspace", "--message-format=json"],
                )?;
                Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
            }
            "build.cargo_test" => {
                self.require_rust_workspace()?;
                let result = self.processes.run_cargo(
                    self.workspace.root(),
                    &["test", "--workspace", "--no-run", "--message-format=json"],
                )?;
                Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
            }
            "build.cargo_test_run" => {
                self.require_rust_workspace()?;
                let result = self.processes.run_capture(
                    self.workspace.root(),
                    "cargo",
                    &["test", "--workspace"],
                    false,
                )?;
                Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
            }
            "build.clippy" => {
                self.require_rust_workspace()?;
                let result = self.processes.run_cargo(
                    self.workspace.root(),
                    &[
                        "clippy",
                        "--workspace",
                        "--all-targets",
                        "--message-format=json",
                        "--",
                        "-D",
                        "warnings",
                    ],
                )?;
                Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
            }
            "build.certify" => {
                self.require_rust_workspace()?;
                let mut stages = Vec::<Value>::new();
                let mut success = true;

                let fmt = self.processes.run_capture(
                    self.workspace.root(),
                    "cargo",
                    &["fmt", "--all", "--", "--check"],
                    false,
                )?;
                success &= fmt.success;
                stages.push(json!({"name":"fmt_check","result":fmt}));

                if success {
                    let check = self.processes.run_cargo(
                        self.workspace.root(),
                        &["check", "--workspace", "--message-format=json"],
                    )?;
                    success &= check.success;
                    stages.push(json!({"name":"cargo_check","result":check}));
                }

                if success {
                    let tests = self.processes.run_capture(
                        self.workspace.root(),
                        "cargo",
                        &["test", "--workspace"],
                        false,
                    )?;
                    success &= tests.success;
                    stages.push(json!({"name":"cargo_test","result":tests}));
                }

                if success {
                    let clippy = self.processes.run_cargo(
                        self.workspace.root(),
                        &[
                            "clippy",
                            "--workspace",
                            "--all-targets",
                            "--message-format=json",
                            "--",
                            "-D",
                            "warnings",
                        ],
                    )?;
                    success &= clippy.success;
                    stages.push(json!({"name":"clippy","result":clippy}));
                }

                Ok(json!({"success": success, "stages": stages}))
            }
            "capture.window" => {
                let pid = required_u64(&call.arguments, "pid")? as u32;
                let relative = arg_string(&call.arguments, "output")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        self.session
                            .directory
                            .strip_prefix(self.workspace.root())
                            .unwrap_or(self.session.directory.as_path())
                            .join("captures")
                            .join(format!("window-{pid}.png"))
                    });
                let capture = self.capture.capture_process_window(pid, &relative)?;
                self.session.log(
                    "capture",
                    "captured process window",
                    json!({"pid": pid, "path": capture.path}),
                )?;
                self.record_development_evidence(
                    "runtime.capture",
                    "Captured runtime window evidence",
                    Some(capture.path.clone()),
                    json!({"pid": pid, "bytes": capture.bytes}),
                )?;
                Ok(serde_json::to_value(capture).map_err(|e| e.to_string())?)
            }
            "vision.inspect" => {
                let relative = required_string(&call.arguments, "path")?;
                let prompt = arg_string(&call.arguments, "prompt").unwrap_or_else(|| {
                    "Inspect this application or game screenshot. Identify rendering, layout, missing-content, clipping, alignment, and obvious runtime regressions. Return concise structured findings.".into()
                });
                let model = arg_string(&call.arguments, "model");
                let provider = self
                    .vision_provider
                    .as_ref()
                    .ok_or_else(|| "no Cortex vision provider is configured".to_string())?;
                let absolute = if let Some(attachment) = relative.strip_prefix("attachment://") {
                    if attachment.is_empty()
                        || attachment.contains('/')
                        || attachment.contains('\\')
                        || attachment.contains("..")
                    {
                        return Err("unsafe Cortex attachment path".into());
                    }
                    self.workspace
                        .cortex_state_dir()
                        .join("attachments")
                        .join(attachment)
                } else {
                    self.workspace
                        .resolve(relative)
                        .map_err(|e| e.to_string())?
                };
                let image = self.capture.load_image_input(&absolute)?;
                let findings = provider
                    .inspect(&image, &prompt, model.as_deref())
                    .map_err(|e| e.to_string())?;
                self.record_development_evidence(
                    "runtime.vision",
                    "Visual/runtime evidence inspected by the configured vision provider",
                    Some(absolute.clone()),
                    json!({"provider": provider.provider_id(), "findings": findings}),
                )?;
                Ok(json!({"provider": provider.provider_id(), "findings": findings}))
            }
            "image.generate" => {
                let provider = self
                    .image_provider
                    .as_ref()
                    .ok_or_else(|| "no Cortex image provider is configured".to_string())?;
                let provider_id = provider.provider_id().to_string();
                let request = ImageGenerationRequest {
                    prompt: required_string(&call.arguments, "prompt")?.to_string(),
                    negative_prompt: arg_string(&call.arguments, "negative_prompt"),
                    model: arg_string(&call.arguments, "model"),
                    width: arg_u64(&call.arguments, "width")
                        .unwrap_or(512)
                        .clamp(16, 4096) as u32,
                    height: arg_u64(&call.arguments, "height")
                        .unwrap_or(512)
                        .clamp(16, 4096) as u32,
                    seed: arg_u64(&call.arguments, "seed"),
                    count: arg_u64(&call.arguments, "count").unwrap_or(1).clamp(1, 16) as u32,
                    workflow: arg_string(&call.arguments, "workflow"),
                    output_dir: generated_output_root_from_state(
                        &self.workspace.cortex_state_dir(),
                    ),
                    tags: call
                        .arguments
                        .get("tags")
                        .and_then(Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                let artifacts = provider.generate(&request).map_err(|e| e.to_string())?;
                let mut registered = Vec::new();
                for artifact in artifacts {
                    registered.push(self.image_catalog.register(
                        artifact,
                        &metadata_root_from_state(&self.workspace.cortex_state_dir()),
                    )?);
                }
                self.image_catalog
                    .save(&catalog_path_from_state(&self.workspace.cortex_state_dir()))?;
                Ok(json!({"provider": provider_id, "artifacts": registered}))
            }
            "image.promote" => {
                let id = required_string(&call.arguments, "id")?;
                let destination = required_string(&call.arguments, "destination")?;
                self.transactions
                    .checkpoint_path(destination)
                    .map_err(|e| e.to_string())?;
                let path = self.image_catalog.promote(
                    id,
                    self.workspace.root(),
                    Path::new(destination),
                )?;
                self.image_catalog
                    .save(&catalog_path_from_state(&self.workspace.cortex_state_dir()))?;
                Ok(json!({"promoted_to": path}))
            }
            "image.reject" => {
                let id = required_string(&call.arguments, "id")?;
                self.image_catalog
                    .set_status(id, ImageArtifactStatus::Rejected)?;
                self.image_catalog
                    .save(&catalog_path_from_state(&self.workspace.cortex_state_dir()))?;
                Ok(json!({"rejected": true}))
            }
            "image.archive" => {
                let id = required_string(&call.arguments, "id")?;
                self.image_catalog
                    .set_status(id, ImageArtifactStatus::Archived)?;
                self.image_catalog
                    .save(&catalog_path_from_state(&self.workspace.cortex_state_dir()))?;
                Ok(json!({"archived": true}))
            }
            "git.host.status" => Ok(local_git_host_status(
                &call.arguments,
                self.workspace.root(),
            )),
            "git.host.bootstrap" => {
                local_git_host_bootstrap(&call.arguments, self.workspace.root())
            }
            "git.host.ensure_ssh_key" => {
                local_git_host_ensure_ssh_key(&call.arguments, self.workspace.root())
            }
            "git.project.attach_local" => {
                let result = local_git_project_attach(&call.arguments, self.workspace.root())?;
                self.git = GitAdapter::detect(self.workspace.root());
                Ok(result)
            }
            "git.host.ensure_identity" => {
                local_git_host_ensure_identity(&call.arguments, self.workspace.root())
            }
            "git.provider.refresh" => forgejo_repository_refresh(&call.arguments, &self.workspace),
            "git.issue.create" => forgejo_issue_create(&call.arguments, &self.workspace),
            "git.changes.snapshot" => git_changes_snapshot(&call.arguments, &self.workspace),
            "git.stage" => git_stage(&call.arguments, &self.workspace),
            "git.unstage" => git_unstage(&call.arguments, &self.workspace),
            "git.commit" => git_commit(&call.arguments, &self.workspace),
            "git.branch.create" => git_branch_create(&call.arguments, &self.workspace),
            "git.branch.switch" => git_branch_switch(&call.arguments, &self.workspace),
            "git.safety.create" => git_safety_create(&call.arguments, &self.workspace),
            "git.safety.list" => git_safety_list(&call.arguments, &self.workspace),
            "git.fetch.local" => git_fetch_local(&call.arguments, &self.workspace),
            "git.sync.status" => git_sync_status(&call.arguments, &self.workspace),
            "git.push.local" => git_push_local(&call.arguments, &self.workspace),
            "git.host.backup" => local_git_host_backup(&call.arguments, self.workspace.root()),
            "git.vault.audit" => git_vault_audit(&call.arguments, &self.workspace),
            "git.vault.ingest" => git_vault_ingest(&call.arguments, &self.workspace),
            "git.vault.materialize" => git_vault_materialize(&call.arguments, &self.workspace),
            "git.release.snapshot" => git_release_snapshot(&call.arguments, &self.workspace),
            "git.repository.overview" => {
                let git = self
                    .git
                    .as_ref()
                    .ok_or_else(|| "Git adapter is not active for this workspace".to_string())?;
                let status = git.status()?;
                let limit = arg_u64(&call.arguments, "limit")
                    .unwrap_or(24)
                    .clamp(1, 100) as usize;
                let commits = git.log(limit)?;
                Ok(json!({
                    "authority": "cortex_repository_overview",
                    "workspace_root": self.workspace.root(),
                    "status": status,
                    "commits": commits,
                    "local_host": local_git_host_status(&call.arguments, self.workspace.root()),
                    "local_sync": git_sync_status(&call.arguments, &self.workspace).ok(),
                    "vault_policy": {
                        "authority": "cortex_vault_for_large_binary_payloads",
                        "git_role": "source_history_and_pointer_manifests",
                        "forgejo_lfs_role": "compatibility_fallback_not_primary_large_binary_authority"
                    }
                }))
            }
            "git.status" => {
                let git = self
                    .git
                    .as_ref()
                    .ok_or_else(|| "Git adapter is not active for this workspace".to_string())?;
                Ok(json!(git.status()?))
            }
            "git.diff" => {
                let git = self
                    .git
                    .as_ref()
                    .ok_or_else(|| "Git adapter is not active for this workspace".to_string())?;
                let paths = call
                    .arguments
                    .get("paths")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(PathBuf::from)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let staged = call
                    .arguments
                    .get("staged")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                Ok(json!({"diff":git.diff(&paths, staged)?}))
            }
            "git.log" => {
                let git = self
                    .git
                    .as_ref()
                    .ok_or_else(|| "Git adapter is not active for this workspace".to_string())?;
                let limit = arg_u64(&call.arguments, "limit")
                    .unwrap_or(20)
                    .clamp(1, 200) as usize;
                Ok(json!({"commits":git.log(limit)?}))
            }
            "vault.metrics" => {
                let registry = WorkspaceRegistry::open_default()?;
                let library = registry
                    .library_root()?
                    .ok_or_else(|| "Cortex Vault root is not configured".to_string())?;
                let mut database = LibraryMemoryDatabase::open(&library.root)?;
                let snapshot = database.record_usage_snapshot()?;
                let project_scope = registry
                    .resolve_for_root(self.workspace.root())?
                    .map(|record| record.id)
                    .unwrap_or_else(|| self.workspace.root().to_string_lossy().to_string());
                Ok(json!({
                    "library_root": library.root,
                    "policy": database.storage_policy.clone(),
                    "metrics": snapshot,
                    "history_samples": database.metrics_history.len(),
                    "knowledge": database.knowledge_status(Some(&project_scope))
                }))
            }
            "vault.policy_get" => {
                let registry = WorkspaceRegistry::open_default()?;
                let library = registry
                    .library_root()?
                    .ok_or_else(|| "Cortex Vault root is not configured".to_string())?;
                let database = LibraryMemoryDatabase::open(&library.root)?;
                Ok(json!({"library_root":library.root,"policy":database.storage_policy.clone()}))
            }
            "vault.policy_set" => {
                let registry = WorkspaceRegistry::open_default()?;
                let library = registry
                    .library_root()?
                    .ok_or_else(|| "Cortex Vault root is not configured".to_string())?;
                let mut database = LibraryMemoryDatabase::open(&library.root)?;
                let mut policy = database.storage_policy.clone();
                if let Some(max_bytes) = arg_u64(&call.arguments, "max_bytes") {
                    policy.max_bytes = (max_bytes > 0).then_some(max_bytes);
                }
                if let Some(warning) = arg_u64(&call.arguments, "warning_percent") {
                    policy.warning_percent = warning.min(99) as u8;
                }
                if let Some(critical) = arg_u64(&call.arguments, "critical_percent") {
                    policy.critical_percent = critical.min(100) as u8;
                }
                if let Some(categories) = call
                    .arguments
                    .get("category_budgets")
                    .and_then(Value::as_object)
                {
                    policy.category_budgets = categories
                        .iter()
                        .filter_map(|(key, value)| value.as_u64().map(|bytes| (key.clone(), bytes)))
                        .collect();
                }
                if let Some(projects) = call
                    .arguments
                    .get("project_budgets")
                    .and_then(Value::as_object)
                {
                    policy.project_budgets = projects
                        .iter()
                        .filter_map(|(key, value)| value.as_u64().map(|bytes| (key.clone(), bytes)))
                        .collect();
                }
                database.set_storage_policy(VaultStoragePolicy {
                    max_bytes: policy.max_bytes,
                    warning_percent: policy.warning_percent,
                    critical_percent: policy.critical_percent,
                    category_budgets: policy.category_budgets,
                    project_budgets: policy.project_budgets,
                })?;
                Ok(
                    json!({"library_root":library.root,"policy":database.storage_policy.clone(),"metrics":database.usage_snapshot()}),
                )
            }
            "vault.knowledge_status" => {
                let registry = WorkspaceRegistry::open_default()?;
                let library = registry
                    .library_root()?
                    .ok_or_else(|| "Cortex Vault root is not configured".to_string())?;
                let database = LibraryMemoryDatabase::open(&library.root)?;
                let query = arg_string(&call.arguments, "query").unwrap_or_default();
                let project_scope = registry
                    .resolve_for_root(self.workspace.root())?
                    .map(|record| record.id)
                    .unwrap_or_else(|| self.workspace.root().to_string_lossy().to_string());
                let context = database.compile_context(Some(&project_scope), &query, 12);
                Ok(json!({
                    "library_root":library.root,
                    "status":database.knowledge_status(Some(&project_scope)),
                    "compiled_context":context
                }))
            }
            "vault.status" => Ok(json!(self.vault.status())),
            "vault.rebuild" | "memory.rebuild" => {
                let files = self
                    .workspace
                    .list_files("", 20_000)
                    .map_err(|e| e.to_string())?;
                self.vault = Vault::build_lexical(self.workspace.root(), &files, 512 * 1024);
                let path = self
                    .workspace
                    .cortex_state_dir()
                    .join("vault")
                    .join("vault.json");
                self.vault.save(&path)?;
                Ok(json!({"status": self.vault.status(), "path": path}))
            }
            "vault.search" | "memory.search" => {
                let query = required_string(&call.arguments, "query")?;
                let limit = arg_u64(&call.arguments, "limit").unwrap_or(8).clamp(1, 50) as usize;
                let hits = self
                    .vault
                    .search(query, limit, self.embedding_provider.as_deref())?;
                Ok(json!({"hits": hits}))
            }
            "plugins.list" => Ok(json!({
                "plugins": PluginRegistry::discover(
                    self.workspace.root(),
                    &self.workspace.cortex_state_dir()
                )?
            })),
            "permissions.status" => Ok(json!(&self.permissions)),
            "vscode.workspace_info" => self.call_vscode("vscode.workspace_info", json!({})),
            "vscode.open_file" => {
                let path = required_string(&call.arguments, "path")?;
                let line = arg_u64(&call.arguments, "line").unwrap_or(1);
                match self.call_vscode("vscode.open_file", json!({"path": path, "line": line})) {
                    Ok(value) => Ok(value),
                    Err(_) => {
                        let absolute = self.workspace.resolve(path).map_err(|e| e.to_string())?;
                        let target = format!("{}:{line}", absolute.display());
                        let result = self.processes.run_capture(
                            self.workspace.root(),
                            "code",
                            &["-g", &target],
                            false,
                        )?;
                        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
                    }
                }
            }
            "pcc.status" => PccClient::discover(self.workspace.root())?.status(),
            "pcc.catalog" => PccClient::discover(self.workspace.root())?.catalog(),
            "pcc.doctor" => PccClient::discover(self.workspace.root())?.doctor(),
            "pcc.archive_audit" => {
                let archive = required_string(&call.arguments, "archive")?;
                let prefix = call.arguments.get("prefix").and_then(Value::as_str);
                PccClient::discover(self.workspace.root())?.archive_audit(archive, prefix)
            }
            "pcc.gate" => {
                let key = required_string(&call.arguments, "key")?;
                PccClient::discover(self.workspace.root())?.gate(key)
            }
            "pcc.run_readonly" => {
                let key = required_string(&call.arguments, "key")?;
                self.authorize_pcc_command(key)?.run_read_only(key)
            }
            "pcc.run" => {
                let key = required_string(&call.arguments, "key")?;
                self.authorize_pcc_command(key)?.run_mutating(key)
            }
            "vscode.get_diagnostics" => self.call_vscode("vscode.get_diagnostics", json!({})),
            "vscode.apply_workspace_edit" => {
                let edits = call
                    .arguments
                    .get("edits")
                    .cloned()
                    .ok_or_else(|| "missing edits argument".to_string())?;
                let items = edits
                    .as_array()
                    .ok_or_else(|| "edits must be an array".to_string())?;
                let mut touched_paths = BTreeSet::new();
                for item in items {
                    let requested_path = item
                        .get("path")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "each VS Code edit requires path".to_string())?;
                    let (path, _) = self.canonical_source_path(requested_path, true)?;
                    let new_text = item
                        .get("new_text")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "each VS Code edit requires new_text".to_string())?;
                    if !path.eq_ignore_ascii_case("Cargo.toml")
                        && !path.ends_with("/Cargo.toml")
                        && !path.ends_with(r"\Cargo.toml")
                    {
                        self.ensure_mutation_dependency_grounding(new_text)?;
                    }
                    self.transactions
                        .checkpoint_path(&path)
                        .map_err(|e| e.to_string())?;
                    touched_paths.insert(path);
                }
                let result =
                    self.call_vscode("vscode.apply_workspace_edit", json!({"edits": edits}))?;
                for path in touched_paths {
                    self.note_source_mutation(&path);
                }
                Ok(result)
            }
            "vscode.save_all" => self.call_vscode("vscode.save_all", json!({})),
            other => {
                for extension in &mut self.extensions {
                    if let Some(result) = extension.execute(call) {
                        return result;
                    }
                }
                Err(format!("unknown Cortex tool: {other}"))
            }
        }
    }
}

impl ToolExecutor for ToolBroker {
    fn definitions(&self) -> Vec<ToolDefinition> {
        let mut result = definitions();
        for extension in &self.extensions {
            result.extend(extension.definitions());
        }
        result
    }

    fn execute(&mut self, call: &ToolCall) -> ToolResultInput {
        // H67E: publish each structured tool transition into the existing project
        // events.jsonl authority so Desktop Activity and runtime bundles see the
        // real operation rather than only the final agent summary.
        let started_unix_ms = unix_ms();
        let mutating = self
            .definitions()
            .iter()
            .find(|definition| definition.name == call.name)
            .map(|definition| definition.mutating)
            .unwrap_or(true);
        let path = call
            .arguments
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let detail = if path.is_empty() {
            call.name.clone()
        } else {
            format!("{} → {path}", call.name)
        };
        let _ = self.events.append(
            EventKind::Tool,
            format!("{} started", call.name),
            &detail,
            None,
            json!({"call_id": call.call_id.clone(), "tool": call.name.clone(), "path": path.clone(), "mutating": mutating}),
        );

        match self.execute_inner(call) {
            Ok(output) => {
                let _ = self.events.append(
                    EventKind::Tool,
                    format!("{} completed", call.name),
                    &detail,
                    Some(true),
                    json!({
                        "call_id": call.call_id.clone(),
                        "tool": call.name.clone(),
                        "path": path.clone(),
                        "mutating": mutating,
                        "duration_ms": unix_ms().saturating_sub(started_unix_ms)
                    }),
                );
                ToolResultInput {
                    call_id: call.call_id.clone(),
                    output,
                    is_error: false,
                }
            }
            Err(error) => {
                let failure_class = classify_tool_failure(&call.name, &error);
                let fingerprint = tool_failure_fingerprint(&call.name, &call.arguments, &error);
                if self.failure_fingerprints.len() >= 512
                    && !self.failure_fingerprints.contains_key(&fingerprint)
                {
                    self.failure_fingerprints.clear();
                }
                let repeat_count = {
                    let count = self
                        .failure_fingerprints
                        .entry(fingerprint.clone())
                        .or_insert(0);
                    *count = count.saturating_add(1);
                    *count
                };
                let strategy_change_required = repeat_count > 1;
                let recovery_hint = tool_recovery_hint(&call.name, failure_class, repeat_count);
                let _ = self.events.append(
                    EventKind::Tool,
                    format!("{} failed", call.name),
                    &error,
                    Some(false),
                    json!({
                        "call_id": call.call_id.clone(),
                        "tool": call.name.clone(),
                        "path": path.clone(),
                        "mutating": mutating,
                        "duration_ms": unix_ms().saturating_sub(started_unix_ms),
                        "error": error.clone(),
                        "failure_class": failure_class,
                        "failure_fingerprint": fingerprint.clone(),
                        "repeat_count": repeat_count,
                        "strategy_change_required": strategy_change_required
                    }),
                );
                let missing_dependency_packages = if failure_class == "dependency_grounding" {
                    dependency_grounding_packages_from_error(&error)
                } else {
                    Vec::new()
                };
                ToolResultInput {
                    call_id: call.call_id.clone(),
                    output: json!({
                        "error": error.clone(),
                        "tool": call.name.clone(),
                        "path": path.clone(),
                        "failure_class": failure_class,
                        "failure_fingerprint": fingerprint,
                        "repeat_count": repeat_count,
                        "strategy_change_required": strategy_change_required,
                        "recovery_hint": recovery_hint,
                        "missing_dependency_packages": missing_dependency_packages
                    }),
                    is_error: true,
                }
            }
        }
    }
}

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        tool("workspace.status", "Read the authoritative project-inspection snapshot, including exact dependency resolution/source roots and current verification authority. HEALTHY may only be claimed from verification_authority evidence for the current source revision.", false, json!({"type":"object","properties":{}})),
        tool("project.status", "Compatibility alias for the authoritative workspace.status project-inspection snapshot.", false, json!({"type":"object","properties":{}})),
        tool("workspace.context_status", "Read the persisted metadata-only Cortex workspace inventory for the active folder.", false, json!({"type":"object","properties":{}})),
        tool("workspace.context_rebuild", "Scan the active folder without reading file bodies and persist a deterministic Cortex workspace inventory before deeper indexing.", false, json!({"type":"object","properties":{"max_files":{"type":"integer"}}})),
        tool("workspace.index_status", "Read the persisted incremental Cortex context index and freshness metadata for the active workspace.", false, json!({"type":"object","properties":{}})),
        tool("workspace.index_rebuild", "Incrementally rebuild the Cortex context index, reusing unchanged file hashes and returning change-set statistics.", false, json!({"type":"object","properties":{"max_files":{"type":"integer"},"max_hash_bytes":{"type":"integer"}}})),
        tool("workspace.registry_list", "List workspaces remembered by the global standalone Cortex registry outside individual repositories.", false, json!({"type":"object","properties":{}})),
        tool("workspace.storage_status", "Read the configured Cortex Storage Authority, stable Windows volume identity, managed layout, offsite-backup target and eligible fixed volumes. D: is only the first-run default.", false, json!({"type":"object","properties":{}})),
        tool("workspace.storage_volumes", "List eligible local storage volumes and stable identity metadata. Removable volumes are excluded unless explicitly requested.", false, json!({"type":"object","properties":{"include_removable":{"type":"boolean"}}})),
        tool("workspace.storage_set", "Provision/select the Cortex Storage Authority at any local drive/folder. This changes the configured authority but does not silently migrate an already-live Cortex installation.", true, json!({"type":"object","properties":{"root":{"type":"string"}},"required":["root"]})),
        tool("workspace.offsite_backup_set", "Configure or clear the offsite backup folder (for example a Google Drive for Desktop synchronized folder). It must remain outside the live Cortex Storage Authority.", true, json!({"type":"object","properties":{"root":{"type":"string"}},"required":["root"]})),
        tool("workspace.scan_storage", "Run the rich read-only project/file catalog across the volume containing the selected Cortex Storage Authority. Discovery never registers or moves projects.", false, json!({"type":"object","properties":{"max_directories":{"type":"integer"},"max_files":{"type":"integer"}}})),
        tool("workspace.scan_machine", "Explicitly scan eligible local PC volumes read-only for project families, nested projects, lineage and duplicate candidates. Fixed volumes only by default; removable volumes require opt-in.", false, json!({"type":"object","properties":{"include_removable":{"type":"boolean"},"max_directories":{"type":"integer"},"max_files":{"type":"integer"}}})),
        tool("workspace.migration_plan", "Create a dry-run migration plan for one exact catalog project id into the selected Cortex managed Projects root. No files are moved by planning.", false, json!({"type":"object","properties":{"project_id":{"type":"string"}},"required":["project_id"]})),
        tool("workspace.migration_apply", "Apply one approved/draft Cortex project migration plan as copy → byte verification → destination-side promotion → project-identity rebind. The original source is preserved and the promoted project remains pending quality validation.", true, json!({"type":"object","properties":{"plan_id":{"type":"string"}},"required":["plan_id"]})),
        tool("source.list", "List files under a specific safe workspace-relative directory. Prefer source.search for repository-wide discovery instead of dumping the workspace root.", false, json!({"type":"object","properties":{"path":{"type":"string"},"max_files":{"type":"integer"}}})),
        tool("source.read", "Read a bounded UTF-8 window from a source/config/document file. Large files return next_offset for deterministic continuation instead of failing on total file size.", false, json!({"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":0},"max_bytes":{"type":"integer"}},"required":["path"]})),
        tool("dependency.ground", "Deterministically ground one exact resolved dependency against its installed local source before model repair. This controller-friendly tool records Ground Before Write evidence without modifying project files.", false, json!({"type":"object","properties":{"package":{"type":"string"},"ecosystem":{"type":"string"},"max_hits":{"type":"integer"}},"required":["package"]})),
        tool("dependency.ensure", "Controller-owned transactional Cargo dependency declaration. Requires an active source transaction. With no version, Cortex may proceed only when exactly one installed local Cargo source version exists; otherwise it fails closed. Checkpoint/quality verification must resolve Cargo.lock before API use.", true, json!({"type":"object","properties":{"package":{"type":"string"},"version":{"type":"string"}},"required":["package"]})),
        tool("source.search", "Search textual workspace files or exact-version local dependency source. Set dependency_package and optional ecosystem to create package-specific Ground Before Write evidence before API-sensitive mutation or repair.", false, json!({"type":"object","properties":{"query":{"type":"string"},"roots":{"type":"array","items":{"type":"string"}},"max_hits":{"type":"integer"},"dependency_package":{"type":"string"},"ecosystem":{"type":"string"}},"required":["query"]})),
        tool("source.transaction_status", "Read the active durable Cortex source transaction, if any.", false, json!({"type":"object","properties":{}})),
        tool("source.transaction_files", "List files touched or created by the active Cortex source transaction.", false, json!({"type":"object","properties":{}})),
        tool("source.checkpoint", "Checkpoint one project-relative path into the active transaction before an external edit.", true, json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})),
        tool("source.begin_transaction", "Begin a reversible candidate source-edit transaction. Cortex captures a fresh project-validation baseline when the active adapter supports it; mutations remain provisional and later validation mechanically rolls back unchanged or worsening candidates.", true, json!({"type":"object","properties":{"label":{"type":"string"}}})),
        tool("source.write_text", "Write an entire decoded text file inside an active transaction. API-sensitive content referencing a declared external dependency is blocked until that exact package has local dependency-source grounding.  `content` must be the real file text after JSON decoding: use actual newline characters in the string value and never double-escape an entire file as literal backslash-n / backslash-quote sequences. Cortex will defensively decode one obviously double-escaped whole-file payload, but models should emit canonical text.", true, json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]})),
        tool("source.replace_text", "Guarded exact source replacement inside an active transaction. API-sensitive replacements are blocked until required exact dependency packages are locally grounded.  `old` and `new` are decoded source text, not JSON-within-JSON strings; do not double-escape multiline source snippets.", true, json!({"type":"object","properties":{"path":{"type":"string"},"old":{"type":"string"},"new":{"type":"string"},"expected_occurrences":{"type":"integer"}},"required":["path","old","new"]})),
        tool("source.commit", "Commit the active Cortex transaction. For candidates with an available quality baseline and source mutations, successful project validation is required before commit.", true, json!({"type":"object","properties":{}})),
        tool("source.rollback", "Roll back the active Cortex transaction.", true, json!({"type":"object","properties":{}})),
        tool("pcc.status", "Read Universal Python Project Control Center status for the active project.", false, json!({"type":"object","properties":{}})),
        tool("pcc.catalog", "Read the active project's canonical PCC command catalog and risk metadata.", false, json!({"type":"object","properties":{}})),
        tool("pcc.doctor", "Check Universal Python PCC requirements and active-project operational readiness.", false, json!({"type":"object","properties":{}})),
        tool("pcc.archive_audit", "Compare a project-relative donor ZIP against the active project file-by-file without importing it.", false, json!({"type":"object","properties":{"archive":{"type":"string"},"prefix":{"type":"string"}},"required":["archive"]})),
        tool("pcc.gate", "Run a named project quality gate through the Universal Python PCC. Gates may create bounded build/cache/artifact/log outputs but cannot mutate source.", false, json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"]})),
        tool("pcc.run_readonly", "Run the exact registered argv for one PCC command only when project.control.json marks it read_only. Arbitrary trailing argv is not accepted.", false, json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"]})),
        tool("pcc.run", "Run the exact registered argv for one PCC command with explicit mutation authority derived from project.control.json. Arbitrary trailing argv is not accepted.", true, json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"]})),
        tool("toolchain.status", "Inspect optional local open-source development integrations such as ripgrep, tree-sitter, rust-analyzer, Gitleaks, cargo-audit, cargo-deny, ScanCode, Wasmtime, Git and SearXNG configuration.", false, json!({"type":"object","properties":{}})),
        tool("project.profile", "Detect the active project adapter/profile and the quality/runtime capabilities Cortex can safely exercise.", false, json!({"type":"object","properties":{}})),
        tool("development.roadmap", "Discover and normalize an authoritative milestone roadmap. If none exists, return a generated proposal that requires approval before execution.", false, json!({"type":"object","properties":{}})),
        tool("development.run_status", "Read the durable active milestone-development run, repair budget, evidence, and resume state.", false, json!({"type":"object","properties":{}})),
        tool("development.run_begin", "Begin a durable generic milestone-development run. Generated roadmaps require explicit approval.", true, json!({"type":"object","properties":{"approve_generated_roadmap":{"type":"boolean"}}})),
        tool("development.run_advance", "Advance the active generic development state machine, or record a failure and consume one bounded repair attempt.", true, json!({"type":"object","properties":{"failure":{"type":"string"}}})),
        tool("development.milestone_complete", "Mark a fully completed durable milestone complete, persist roadmap progress, and begin the next dependency-ready milestone when one exists.", true, json!({"type":"object","properties":{}})),
        tool("development.certification_plan", "Generate capability-driven certification requirements from the active milestone and detected project profile.", false, json!({"type":"object","properties":{}})),
        tool("build.project_checkpoint", "Run the project's authoritative native checkpoint when one is detected. Project-native checkpoints outrank generated per-capability health claims and return their bounded checkpoint log as Repair evidence.", false, json!({"type":"object","properties":{}})),
        tool("build.project_format", "Run the detected project adapter's formatting verification stage.", false, json!({"type":"object","properties":{}})),
        tool("build.project_validate", "Run the detected project adapter's compile/check/validation stage. When a candidate transaction is active, compare against its fresh baseline: improved candidates remain provisional, unchanged or regressing candidates roll back mechanically, and repeated diagnostic fingerprints require a strategy change.", false, json!({"type":"object","properties":{}})),
        tool("build.project_test", "Run the detected project adapter's test stage.", false, json!({"type":"object","properties":{}})),
        tool("build.project_lint", "Run the detected project adapter's lint/static-analysis stage when supported.", false, json!({"type":"object","properties":{}})),
        tool("build.project_build", "Build the active project through its detected project adapter.", false, json!({"type":"object","properties":{}})),
        tool("runtime.launch_project", "Launch the runtime artifact resolved by Cortex's universal project plan under Cortex ownership. Project-local executables and approved interpreter runtimes are supported through the detected adapter/profile.", true, json!({"type":"object","properties":{"args":{"type":"array","items":{"type":"string"}}}})),
        tool("runtime.verify_project", "Controller-owned runtime completion gate for any project with a resolvable universal RuntimeArtifact. Proves the process remains alive and can require a visible responsive native window plus a changing title.", true, json!({"type":"object","properties":{"minimum_alive_ms":{"type":"integer"},"keep_running":{"type":"boolean"},"require_window":{"type":"boolean"},"require_title_change":{"type":"boolean"},"title_sample_interval_ms":{"type":"integer"}}})),
        tool("runtime.process_status", "Read Cortex-owned runtime process records and live states.", false, json!({"type":"object","properties":{}})),
        tool("runtime.window_info", "Inspect the native main window for a known runtime PID: title, visibility, responsiveness and bounds.", false, json!({"type":"object","properties":{"pid":{"type":"integer"}},"required":["pid"]})),
        tool("runtime.process_stop", "Stop a Cortex-owned runtime process by PID; arbitrary external processes are never terminated.", true, json!({"type":"object","properties":{"pid":{"type":"integer"}},"required":["pid"]})),
        tool("build.cargo_fmt_check", "Verify rustfmt formatting across the active Rust workspace.", false, json!({"type":"object","properties":{}})),
        tool("build.cargo_check", "Run cargo check for the active Rust workspace and return structured compiler diagnostics.", false, json!({"type":"object","properties":{}})),
        tool("build.cargo_test", "Compile all Rust workspace tests and return structured diagnostics without running tests.", false, json!({"type":"object","properties":{}})),
        tool("build.cargo_test_run", "Run all active Rust workspace tests.", false, json!({"type":"object","properties":{}})),
        tool("build.clippy", "Run Clippy across the active Rust workspace with warnings denied and return structured diagnostics.", false, json!({"type":"object","properties":{}})),
        tool("build.certify", "Run the CLI certification build gate: fmt-check, cargo check, workspace tests and Clippy with warnings denied.", false, json!({"type":"object","properties":{}})),
        tool("capture.window", "Capture a Windows application window owned by a known process.", false, json!({"type":"object","properties":{"pid":{"type":"integer"},"output":{"type":"string"}},"required":["pid"]})),
        tool("vision.inspect", "Inspect a workspace or managed attachment PNG/JPEG/WebP image using the configured vision model.", false, json!({"type":"object","properties":{"path":{"type":"string"},"prompt":{"type":"string"},"model":{"type":"string"}},"required":["path"]})),
        tool("image.generate", "Generate managed Cortex image artifacts through the configured image provider.", true, json!({"type":"object","properties":{"prompt":{"type":"string"},"negative_prompt":{"type":"string"},"model":{"type":"string"},"width":{"type":"integer"},"height":{"type":"integer"},"seed":{"type":"integer"},"count":{"type":"integer"},"workflow":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}}},"required":["prompt"]})),
        tool("image.promote", "Promote a generated Cortex image artifact into project content.", true, json!({"type":"object","properties":{"id":{"type":"string"},"destination":{"type":"string"}},"required":["id","destination"]})),
        tool("image.reject", "Mark a generated image artifact as rejected.", true, json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"]})),
        tool("image.archive", "Archive a generated image artifact.", true, json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"]})),
        tool("git.host.status", "Read the machine-local Cortex Forgejo host status, storage root, container runtime availability and loopback endpoints. This never changes project files or service state.", false, json!({"type":"object","properties":{"root":{"type":"string"},"runtime":{"type":"string"}}})),
        tool("git.host.bootstrap", "Provision the reusable machine-local Cortex Forgejo host under the configured Cortex Storage Authority, using Docker/Podman Compose, SQLite, localhost-only ports, private repositories and push-to-create. Existing host configuration is preserved unless force=true.", true, json!({"type":"object","properties":{"root":{"type":"string"},"runtime":{"type":"string"},"start":{"type":"boolean"},"force":{"type":"boolean"}}})),
        tool("git.host.ensure_ssh_key", "Create or read the dedicated Cortex local-Forgejo Ed25519 SSH keypair. Returns only the public key and paths; the private key is never returned in tool output. Add the public key once in the local Forgejo account before autonomous SSH pushes.", true, json!({"type":"object","properties":{"root":{"type":"string"},"force":{"type":"boolean"}}})),
        tool("git.project.attach_local", "Attach the active project to the machine-local Cortex Forgejo host without replacing existing remotes. Can initialize Git when missing, adds/updates the dedicated cortex-local remote, defaults ownership to the private Cortex service identity, and optionally performs a private push-to-create.", true, json!({"type":"object","properties":{"root":{"type":"string"},"owner":{"type":"string"},"repository":{"type":"string"},"remote_name":{"type":"string"},"initialize":{"type":"boolean"},"overwrite_remote":{"type":"boolean"},"push":{"type":"boolean"}}})),
        tool("git.host.ensure_identity", "Create or reuse the private Cortex service identity inside the machine-local Forgejo host, generate a scoped API token when needed, protect that token with the current Windows user's DPAPI context, ensure the dedicated SSH key exists, and register its public key. Secret material is never returned in tool output.", true, json!({"type":"object","properties":{"root":{"type":"string"},"runtime":{"type":"string"},"username":{"type":"string"},"email":{"type":"string"}}})),
        tool("git.provider.refresh", "Refresh the active project's private machine-local Forgejo repository snapshot off the GUI thread and cache Issues, Pull Requests, Releases and available Actions-run metadata in the project Cortex state directory.", false, json!({"type":"object","properties":{"root":{"type":"string"},"runtime":{"type":"string"},"owner":{"type":"string"},"repository":{"type":"string"},"remote_name":{"type":"string"},"limit":{"type":"integer"}}})),
        tool("git.issue.create", "Create an issue in the active project's private machine-local Forgejo repository through the scoped Cortex service identity.", true, json!({"type":"object","properties":{"root":{"type":"string"},"owner":{"type":"string"},"repository":{"type":"string"},"remote_name":{"type":"string"},"title":{"type":"string"},"body":{"type":"string"}},"required":["title"]})),
        tool("git.changes.snapshot", "Read branch, status, staged diff, unstaged diff and remotes for the selected project.", false, json!({"type":"object","properties":{"diff_max_chars":{"type":"integer"}}})),
        tool("git.stage", "Stage selected project-relative paths or all changes. 25 MiB+ files are surfaced for Vault review and 100 MiB+ files require explicit allow_large_git=true.", true, json!({"type":"object","properties":{"paths":{"type":"array","items":{"type":"string"}},"all":{"type":"boolean"},"allow_large_git":{"type":"boolean"}}})),
        tool("git.unstage", "Unstage selected project-relative paths or all staged changes without discarding working-tree content.", true, json!({"type":"object","properties":{"paths":{"type":"array","items":{"type":"string"}},"all":{"type":"boolean"}}})),
        tool("git.commit", "Commit the currently staged changes with an explicit message. This tool never auto-stages.", true, json!({"type":"object","properties":{"message":{"type":"string"},"author_name":{"type":"string"},"author_email":{"type":"string"}},"required":["message"]})),
        tool("git.branch.create", "Create a local branch, optionally switching to it.", true, json!({"type":"object","properties":{"name":{"type":"string"},"switch":{"type":"boolean"}},"required":["name"]})),
        tool("git.branch.switch", "Switch to an existing local branch. Git itself remains the dirty-tree safety authority.", true, json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]})),
        tool("git.safety.create", "Create a hidden Cortex recovery commit under refs/cortex/checkpoints/* using an alternate temporary index, leaving the user's branch and real index unchanged.", true, json!({"type":"object","properties":{"label":{"type":"string"},"id":{"type":"string"}}})),
        tool("git.safety.list", "List hidden Cortex safety checkpoints.", false, json!({"type":"object","properties":{"limit":{"type":"integer"}}})),
        tool("git.fetch.local", "Fetch the dedicated cortex-local Forgejo remote with the Cortex SSH identity, pruning stale remote-tracking refs without switching branches or changing working-tree files.", true, json!({"type":"object","properties":{"remote_name":{"type":"string"},"prune":{"type":"boolean"}}})),
        tool("git.sync.status", "Read current-branch synchronization state against cortex-local: local/remote commit ids, ahead/behind counts, divergence, upstream-ref presence and dirty working-tree state.", false, json!({"type":"object","properties":{"remote_name":{"type":"string"}}})),
        tool("git.push.local", "Push the current branch to the dedicated cortex-local Forgejo remote with the Cortex SSH identity. This tool never force-pushes.", true, json!({"type":"object","properties":{"remote_name":{"type":"string"},"set_upstream":{"type":"boolean"}}})),
        tool("git.host.backup", "Create a complete Forgejo files/database dump inside the local container, copy it into the Cortex Git backup root, verify non-zero bytes and SHA-256, then remove the temporary container dump. Restore is deliberately separate and not automated by this tool.", true, json!({"type":"object","properties":{"root":{"type":"string"},"runtime":{"type":"string"}}})),
        tool("git.vault.audit", "Audit Git-visible project files for Cortex Vault candidates using configurable review/hard-guard thresholds, and cache the result for the Repositories Insights workspace.", false, json!({"type":"object","properties":{"review_bytes":{"type":"integer"},"hard_guard_bytes":{"type":"integer"},"limit":{"type":"integer"}}})),
        tool("git.vault.ingest", "Copy one project file into the Cortex Vault SHA-256 content-addressed object store and write a Git-trackable .cortex-vault pointer manifest. The working payload is not removed.", true, json!({"type":"object","properties":{"path":{"type":"string"},"logical_path":{"type":"string"},"license":{"type":"string"},"source_url":{"type":"string"}},"required":["path"]})),
        tool("git.vault.materialize", "Materialize a .cortex-vault pointer from the Cortex Vault object store back to its project logical path. Existing files are not overwritten unless overwrite=true.", true, json!({"type":"object","properties":{"pointer":{"type":"string"},"overwrite":{"type":"boolean"}},"required":["pointer"]})),
        tool("git.release.snapshot", "Create a deterministic project release-state descriptor binding the current Git commit/branch to the current set of .cortex-vault pointer manifests. This does not publish or upload a release.", true, json!({"type":"object","properties":{"version":{"type":"string"},"notes":{"type":"string"}},"required":["version"]})),
        tool("git.repository.overview", "Read the active repository overview for Cortex Repositories: working-tree status, recent commits, machine-local Forgejo host state and the Git/Vault authority contract.", false, json!({"type":"object","properties":{"limit":{"type":"integer"},"root":{"type":"string"},"runtime":{"type":"string"}}})),
        tool("git.status", "Read Git branch and working-tree status.", false, json!({"type":"object","properties":{}})),
        tool("git.diff", "Read Git working-tree or staged diff.", false, json!({"type":"object","properties":{"paths":{"type":"array","items":{"type":"string"}},"staged":{"type":"boolean"}}})),
        tool("git.log", "Read recent Git commit history.", false, json!({"type":"object","properties":{"limit":{"type":"integer"}}})),
        tool("vault.metrics", "Read and persist Cortex Vault managed-storage metrics: quota use, project/category accounting, reclaimable bytes, duplicate-byte opportunity, history samples and project knowledge status.", false, json!({"type":"object","properties":{}})),
        tool("vault.policy_get", "Read the assigned Cortex Vault storage quota and warning/critical thresholds.", false, json!({"type":"object","properties":{}})),
        tool("vault.policy_set", "Set Cortex Vault logical storage quota, warning/critical thresholds, and optional category/project budgets. A zero max_bytes clears the hard logical quota.", true, json!({"type":"object","properties":{"max_bytes":{"type":"integer"},"warning_percent":{"type":"integer"},"critical_percent":{"type":"integer"},"category_budgets":{"type":"object","additionalProperties":{"type":"integer"}},"project_budgets":{"type":"object","additionalProperties":{"type":"integer"}}}})),
        tool("vault.knowledge_status", "Read the Vault-backed project knowledge graph/context status and compile a small project-scoped context sample for an optional query.", false, json!({"type":"object","properties":{"query":{"type":"string"}}})),
        tool("vault.status", "Read Cortex Vault lexical/vector index status.", false, json!({"type":"object","properties":{}})),
        tool("vault.rebuild", "Rebuild Cortex Vault lexical source index.", false, json!({"type":"object","properties":{}})),
        tool("vault.search", "Retrieve hybrid lexical/vector context from Cortex Vault.", false, json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"}},"required":["query"]})),
        tool("memory.rebuild", "Compatibility alias for vault.rebuild.", false, json!({"type":"object","properties":{}})),
        tool("memory.search", "Compatibility alias for vault.search.", false, json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"}},"required":["query"]})),
        tool("plugins.list", "List built-in and workspace Cortex plugin manifests.", false, json!({"type":"object","properties":{}})),
        tool("permissions.status", "Read current Cortex permissions; agents cannot grant themselves access.", false, json!({"type":"object","properties":{}})),
        tool("vscode.workspace_info", "Read active VS Code workspace, file and selection context from the Cortex VS Code bridge.", false, json!({"type":"object","properties":{}})),
        tool("vscode.open_file", "Open and reveal a project source file in VS Code; falls back to the code CLI when bridge is unavailable.", false, json!({"type":"object","properties":{"path":{"type":"string"},"line":{"type":"integer"}},"required":["path"]})),
        tool("vscode.get_diagnostics", "Read current VS Code/rust-analyzer diagnostics for the active project.", false, json!({"type":"object","properties":{}})),
        tool("vscode.apply_workspace_edit", "Apply grouped project-relative text edits through VS Code WorkspaceEdit. Ground Before Write applies to dependency-sensitive new_text just as it does to native Cortex source mutations.", true, json!({
            "type":"object",
            "properties":{
                "edits":{
                    "type":"array",
                    "items":{
                        "type":"object",
                        "properties":{
                            "path":{"type":"string"},
                            "start":{
                                "type":"object",
                                "properties":{
                                    "line":{"type":"integer"},
                                    "character":{"type":"integer"}
                                }
                            },
                            "end":{
                                "type":"object",
                                "properties":{
                                    "line":{"type":"integer"},
                                    "character":{"type":"integer"}
                                }
                            },
                            "new_text":{"type":"string"}
                        },
                        "required":["path","new_text"]
                    }
                }
            },
            "required":["edits"]
        })),
        tool("vscode.save_all", "Save dirty VS Code workspace documents.", true, json!({"type":"object","properties":{}})),
    ]
}

fn cargo_workspace_summary(cargo_toml: &str) -> Value {
    let members = parse_toml_string_array(cargo_toml, "members");
    let excluded = parse_toml_string_array(cargo_toml, "exclude");
    let apps = members
        .iter()
        .filter(|p| p.starts_with("apps/"))
        .cloned()
        .collect::<Vec<_>>();
    let crates = members
        .iter()
        .filter(|p| p.starts_with("crates/"))
        .cloned()
        .collect::<Vec<_>>();
    let generic_cortex_authorities = members
        .iter()
        .filter(|p| p.starts_with("crates/cortex_"))
        .cloned()
        .collect::<Vec<_>>();
    let open2d_cortex_compatibility = members
        .iter()
        .filter(|p| p.starts_with("crates/open2d_cortex_"))
        .cloned()
        .collect::<Vec<_>>();
    let reference_members = members
        .iter()
        .filter(|p| {
            let lower = p.to_ascii_lowercase();
            lower.contains("reference") || lower.contains("legacy") || lower.contains("prototype")
        })
        .cloned()
        .collect::<Vec<_>>();

    json!({
        "member_count": members.len(),
        "members": members,
        "apps": apps,
        "crates": crates,
        "generic_cortex_authorities": generic_cortex_authorities,
        "open2d_cortex_compatibility": open2d_cortex_compatibility,
        "reference_members": reference_members,
        "excluded": excluded
    })
}

fn parse_toml_string_array(document: &str, key: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut collecting = false;

    for line in document.lines() {
        let trimmed = line.trim();
        if !collecting {
            let starts_key = trimmed.starts_with(key)
                && trimmed
                    .get(key.len()..)
                    .map(str::trim_start)
                    .is_some_and(|rest| rest.starts_with('='));
            if starts_key && trimmed.contains('[') {
                collecting = true;
                collect_quoted_strings(trimmed, &mut values);
                if trimmed.contains(']') {
                    break;
                }
            }
            continue;
        }

        collect_quoted_strings(trimmed, &mut values);
        if trimmed.contains(']') {
            break;
        }
    }

    values
}

fn collect_quoted_strings(line: &str, output: &mut Vec<String>) {
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('"') else {
            break;
        };
        output.push(after_start[..end].to_string());
        rest = &after_start[end + 1..];
    }
}

fn source_manifest_summary(manifest: &Value) -> Value {
    let files = manifest
        .get("files")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut rust_files = 0usize;
    let mut png_files = 0usize;
    let mut json_files = 0usize;
    let mut markdown_files = 0usize;
    let mut toml_files = 0usize;

    for entry in files {
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if path.ends_with(".rs") {
            rust_files += 1;
        } else if path.ends_with(".png") {
            png_files += 1;
        } else if path.ends_with(".json") {
            json_files += 1;
        } else if path.ends_with(".md") {
            markdown_files += 1;
        } else if path.ends_with(".toml") {
            toml_files += 1;
        }
    }

    json!({
        "schema_version": manifest.get("schema_version").cloned().unwrap_or(Value::Null),
        "pass": manifest.get("pass").cloned().unwrap_or(Value::Null),
        "baseline": manifest.get("baseline").cloned().unwrap_or(Value::Null),
        "purpose": manifest.get("purpose").cloned().unwrap_or(Value::Null),
        "build_status": manifest.get("build_status").cloned().unwrap_or(Value::Null),
        "file_count": files.len(),
        "counts": {
            "rust": rust_files,
            "png": png_files,
            "json": json_files,
            "markdown": markdown_files,
            "toml": toml_files
        }
    })
}

fn checkpoint_summary(root: &Path) -> Value {
    let path = root
        .join("logs")
        .join("sessions")
        .join("LATEST_CORTEX_CHECKPOINT.log");
    let Ok(bytes) = std::fs::read(&path) else {
        return json!({"available": false, "path": path});
    };
    let text = String::from_utf8_lossy(&bytes);
    let failed = text.contains("Cortex checkpoint FAILED");
    let explicit_pass =
        text.contains("Cortex checkpoint PASSED") || text.contains("CORTEX CHECKPOINT PASSED");

    json!({
        "available": true,
        "path": path,
        "status": if failed { "failed" } else if explicit_pass { "passed" } else { "unknown" },
        "normalization_clean": text.contains("Cortex normalization closure: CLEAN"),
        "root_cleanliness_pass": text.contains("Open2D root cleanliness: PASS"),
        "static_certification_pass": text.contains("Cortex static certification: PASS"),
        "desktop_certification_ready": text.contains("Cortex Desktop certification: READY"),
        "workspace_member_closure_79_of_79": text.contains("Cargo workspace member closure: 79 / 79"),
        "tool_catalog_74": text.contains("Cortex tool catalog matches Rust authority: 74 tools")
    })
}

fn authority_audit_summary(path: &Path) -> Value {
    let Ok(bytes) = std::fs::read(path) else {
        return json!({"available": false, "path": path});
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return json!({"available": true, "path": path, "parse_error": true});
    };

    json!({
        "available": true,
        "path": path,
        "schema_version": value.get("schema_version").cloned().unwrap_or(Value::Null),
        "pass": value.get("pass").cloned().unwrap_or(Value::Null),
        "result": value.get("result").cloned().unwrap_or(Value::Null),
        "status": value.get("status").cloned().unwrap_or(Value::Null),
        "normalization": value.get("normalization").cloned().unwrap_or(Value::Null)
    })
}

fn non_workspace_app_manifests(root: &Path, member_values: &[Value]) -> Vec<String> {
    let active = member_values
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<std::collections::BTreeSet<_>>();
    let apps_root = root.join("apps");
    let Ok(entries) = std::fs::read_dir(apps_root) else {
        return Vec::new();
    };

    let mut out = entries
        .flatten()
        .filter_map(|entry| {
            if !entry.file_type().ok()?.is_dir() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let relative = format!("apps/{name}");
            if active.contains(&relative) || !entry.path().join("Cargo.toml").is_file() {
                return None;
            }
            Some(relative)
        })
        .collect::<Vec<_>>();
    out.sort();
    out
}

fn tool(name: &str, description: &str, mutating: bool, parameters: Value) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        parameters,
        mutating,
    }
}

fn normalize_multiline_text_argument(content: &str) -> (String, bool) {
    // Some local tool templates can double-escape an entire multiline file payload
    // (for example `[package]\\nname = \\"hello3d\\"`) even though the outer
    // function-call JSON has already been decoded.  LM Studio masks more of these
    // template differences in its compatibility layer; Cortex owns that final-mile
    // normalization for Native Models so malformed escape text never reaches disk.
    if content.contains('\n') || !(content.contains("\\n") || content.contains("\\r")) {
        return (content.to_string(), false);
    }

    // Treat the already-decoded argument as a JSON string body exactly once.  This
    // only succeeds when the complete payload is still escaped (quotes included),
    // which avoids rewriting legitimate source such as `println!("a\\\\nb")`.
    let wrapped = format!("\"{content}\"");
    let Ok(decoded) = serde_json::from_str::<String>(&wrapped) else {
        return (content.to_string(), false);
    };
    if decoded.contains('\n') {
        (decoded, true)
    } else {
        (content.to_string(), false)
    }
}

fn classify_tool_failure(tool: &str, error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("m11 grounded regeneration required") {
        "grounded_regeneration"
    } else if lower.contains("ground before write")
        || lower.contains("dependency-source evidence is missing")
    {
        "dependency_grounding"
    } else if lower.contains("unsafe workspace path")
        || lower.contains("outside workspace")
        || lower.contains("escapes workspace")
        || lower.contains("parent path")
    {
        "path_authority"
    } else if lower.contains("read limit")
        || lower.contains("read budget")
        || lower.contains("max_bytes")
        || lower.contains("file is too large")
    {
        "read_budget"
    } else if tool == "source.replace_text"
        && (lower.contains("occurrence")
            || lower.contains("ambiguous")
            || lower.contains("replacement")
            || lower.contains("anchor"))
    {
        "deterministic_mutation_guard"
    } else if lower.contains("permission denied")
        || lower.contains("access is denied")
        || lower.contains("not permitted")
    {
        "permission"
    } else if lower.contains("not found")
        || lower.contains("no such file")
        || lower.contains("executable")
        || lower.contains("toolchain")
    {
        "environment"
    } else {
        "unknown"
    }
}

fn normalized_failure_signature(error: &str) -> String {
    error
        .chars()
        .take(2_048)
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn project_topology(workspace: &Workspace) -> Result<Value, String> {
    let inventory = workspace
        .scan_inventory(50_000)
        .map_err(|e| e.to_string())?;
    let root_name = workspace
        .root()
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_string();
    let mut root_manifests = Vec::new();
    let mut nested = BTreeSet::<PathBuf>::new();
    for manifest in &inventory.manifests {
        let parent = manifest.parent().unwrap_or_else(|| Path::new(""));
        if parent.as_os_str().is_empty() {
            root_manifests.push(manifest.clone())
        } else {
            nested.insert(parent.to_path_buf());
        }
    }
    let repeated = workspace.root().join(&root_name);
    let exists = repeated.is_dir();
    let registered = nested.contains(Path::new(&root_name));
    Ok(
        json!({"root":workspace.root(),"root_name":root_name,"profile":workspace.profile(),"root_manifests":root_manifests,"nested_project_roots":nested,"protected_roots":[".git",".cortex",".open2d","target","build","builds","dist","node_modules","vendor",".venv","venv"],"repeated_root_directory":{"exists":exists,"registered_nested_project":registered,"review_as_duplicate_residue":exists&&!registered}}),
    )
}

fn quality_capability_label(capability: QualityCapability) -> &'static str {
    match capability {
        QualityCapability::Format => "format",
        QualityCapability::Validate => "validate",
        QualityCapability::Test => "test",
        QualityCapability::Lint => "lint",
        QualityCapability::Build => "build",
        QualityCapability::Package => "package",
    }
}

fn direct_dependency_names(root: &Path) -> BTreeSet<String> {
    if root.join("Cargo.toml").is_file() {
        if let Ok(text) = fs::read_to_string(root.join("Cargo.toml")) {
            return cargo_manifest_dependency_names(&text).into_keys().collect();
        }
    }
    if root.join("package.json").is_file() {
        if let Ok(text) = fs::read_to_string(root.join("package.json")) {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                let mut names = BTreeSet::new();
                for section in ["dependencies", "devDependencies", "peerDependencies"] {
                    if let Some(object) = value.get(section).and_then(Value::as_object) {
                        names.extend(object.keys().cloned());
                    }
                }
                return names;
            }
        }
    }
    BTreeSet::new()
}

fn canonical_dependency_name(root: &Path, requested: &str) -> Option<String> {
    let requested_lower = requested.to_ascii_lowercase();
    direct_dependency_names(root).into_iter().find(|name| {
        let crate_ident = name.replace('-', "_");
        name.eq_ignore_ascii_case(requested)
            || crate_ident.eq_ignore_ascii_case(requested)
            || name.to_ascii_lowercase() == requested_lower
    })
}

fn dependency_references_in_text(root: &Path, text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for package in direct_dependency_names(root) {
        let rust_ident = package.replace('-', "_");
        let needles = [
            format!("use {rust_ident}::"),
            format!("pub use {rust_ident}::"),
            format!("{rust_ident}::"),
            format!("extern crate {rust_ident}"),
            format!("from '{package}'"),
            format!("from \"{package}\""),
            format!("require('{package}')"),
            format!("require(\"{package}\")"),
        ];
        if needles.iter().any(|needle| text.contains(needle)) {
            found.insert(package);
        }
    }
    found
}

fn diagnostic_dependency_packages(root: &Path, result: &CommandResult) -> BTreeSet<String> {
    let direct = direct_dependency_names(root);
    let mut found = BTreeSet::new();
    for diagnostic in &result.diagnostics {
        let haystack = format!(
            "{} {}",
            diagnostic.code.as_deref().unwrap_or_default(),
            diagnostic.message
        )
        .to_ascii_lowercase();
        for package in &direct {
            let rust_ident = package.replace('-', "_").to_ascii_lowercase();
            if haystack.contains(&package.to_ascii_lowercase()) || haystack.contains(&rust_ident) {
                found.insert(package.clone());
            }
        }
    }
    found
}

fn project_dependency_grounding(
    root: &Path,
    processes: Option<&ProcessService>,
) -> Result<Value, String> {
    let mut ecosystems = Vec::new();
    if root.join("Cargo.toml").is_file() {
        ecosystems.push(cargo_dependency_grounding(root, processes)?);
    }
    if root.join("package.json").is_file() {
        ecosystems.push(node_dependency_grounding(root)?);
    }
    let additional = [
        "pyproject.toml",
        "requirements.txt",
        "CMakeLists.txt",
        "vcpkg.json",
        "conanfile.txt",
        "conanfile.py",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "go.mod",
    ]
    .iter()
    .filter(|n| root.join(n).is_file())
    .map(|n| (*n).to_string())
    .collect::<Vec<_>>();
    Ok(json!({
        "authority":"project_manifests_lockfiles_and_exact_local_sources",
        "root":root,
        "ecosystems":ecosystems,
        "additional_manifest_evidence":additional,
        "instruction":"Ground Before Write: API-sensitive source mutations must be backed by exact resolved local dependency-source evidence. Workspace dependency inventory alone does not satisfy API grounding; use source.search with dependency_package for the package being changed."
    }))
}

fn cargo_dependency_grounding(
    root: &Path,
    processes: Option<&ProcessService>,
) -> Result<Value, String> {
    let manifest = fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let direct = cargo_manifest_dependency_names(&manifest);
    let lock = cargo_lock_packages(root)?;
    let metadata = processes.and_then(|service| cargo_metadata_dependency_map(root, service).ok());
    let mut deps = Vec::new();
    for (name, requested) in direct {
        let resolved = lock.get(&name).cloned().unwrap_or_default();
        let metadata_records = metadata
            .as_ref()
            .and_then(|records| records.get(&name))
            .cloned()
            .unwrap_or_default();
        let mut local = metadata_records
            .iter()
            .filter_map(|record| record.get("source_root"))
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        if local.is_empty() {
            local.extend(
                resolved
                    .iter()
                    .flat_map(|version| local_cargo_source_roots(&name, version)),
            );
        }
        local.sort();
        local.dedup();
        deps.push(json!({
            "name":name,
            "requested":requested,
            "resolved_versions":resolved,
            "local_source_roots":local,
            "cargo_metadata_records":metadata_records
        }));
    }
    Ok(json!({
        "ecosystem":"cargo",
        "manifest":"Cargo.toml",
        "lockfile":root.join("Cargo.lock").is_file().then_some("Cargo.lock"),
        "metadata_mode": if metadata.is_some() { "cargo_metadata_locked_offline" } else { "lockfile_fallback" },
        "dependencies":deps
    }))
}

fn cargo_metadata_dependency_map(
    root: &Path,
    processes: &ProcessService,
) -> Result<BTreeMap<String, Vec<Value>>, String> {
    if !root.join("Cargo.lock").is_file() {
        return Err("Cargo.lock is absent; Cortex will not run cargo metadata in a way that could create or rewrite dependency state during read-only grounding".into());
    }
    let result = processes.run_cargo(
        root,
        &["metadata", "--locked", "--offline", "--format-version=1"],
    )?;
    if !result.success {
        return Err(format!(
            "cargo metadata --locked --offline failed: {}",
            tail_chars(&format!("{}\n{}", result.stdout, result.stderr), 2_000)
        ));
    }
    let metadata: Value = serde_json::from_str(result.stdout.trim()).map_err(|error| {
        format!("cargo metadata returned invalid JSON during dependency grounding: {error}")
    })?;
    let direct = direct_dependency_names(root);
    let mut out = BTreeMap::<String, Vec<Value>>::new();
    for package in metadata
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(name) = package.get("name").and_then(Value::as_str) else {
            continue;
        };
        if !direct.contains(name) {
            continue;
        }
        let manifest_path = package
            .get("manifest_path")
            .and_then(Value::as_str)
            .map(PathBuf::from);
        let source_root = manifest_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(Path::to_path_buf);
        out.entry(name.to_string()).or_default().push(json!({
            "version": package.get("version").cloned().unwrap_or(Value::Null),
            "source": package.get("source").cloned().unwrap_or(Value::Null),
            "manifest_path": manifest_path,
            "source_root": source_root
        }));
    }
    Ok(out)
}

fn ensure_cargo_dependency_text(
    manifest: &str,
    package: &str,
    version: &str,
) -> Result<(String, bool), String> {
    let valid_package = !package.is_empty()
        && package
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    let valid_version = !version.is_empty()
        && version.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '.' | '-' | '+' | '*' | '^' | '~' | '=')
        });
    if !valid_package || !valid_version {
        return Err(
            "dependency.ensure requires a simple Cargo package name and version requirement".into(),
        );
    }
    if cargo_manifest_dependency_names(manifest).contains_key(package) {
        return Ok((manifest.to_string(), false));
    }
    let declaration = format!("{package} = \"{version}\"");
    let mut lines = manifest.lines().map(str::to_string).collect::<Vec<_>>();
    if let Some(index) = lines
        .iter()
        .position(|line| line.trim() == "[dependencies]")
    {
        lines.insert(index + 1, declaration);
    } else {
        if lines.last().is_none_or(|line| !line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("[dependencies]".into());
        lines.push(declaration);
    }
    let mut updated = lines.join("\n");
    if manifest.ends_with('\n') {
        updated.push('\n');
    }
    Ok((updated, true))
}

fn cargo_manifest_dependency_names(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut active = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            let s = line.trim_matches(&['[', ']'][..]).to_ascii_lowercase();
            active = s == "dependencies"
                || s == "dev-dependencies"
                || s == "build-dependencies"
                || s.ends_with(".dependencies");
            continue;
        }
        if !active || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((n, v)) = line.split_once('=') {
            let n = n.trim().trim_matches('"').to_string();
            if !n.is_empty() {
                out.entry(n).or_insert_with(|| v.trim().to_string());
            }
        }
    }
    out
}
fn cargo_lock_packages(root: &Path) -> Result<BTreeMap<String, Vec<String>>, String> {
    let path = root.join("Cargo.lock");
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut out = BTreeMap::<String, Vec<String>>::new();
    let mut name = None;
    let mut version = None;
    let flush = |name: &mut Option<String>,
                 version: &mut Option<String>,
                 out: &mut BTreeMap<String, Vec<String>>| {
        if let (Some(n), Some(v)) = (name.take(), version.take()) {
            let list = out.entry(n).or_default();
            if !list.contains(&v) {
                list.push(v);
            }
        }
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line == "[[package]]" {
            flush(&mut name, &mut version, &mut out);
            continue;
        }
        if let Some(v) = line.strip_prefix("name = ") {
            name = Some(v.trim_matches('"').to_string())
        } else if let Some(v) = line.strip_prefix("version = ") {
            version = Some(v.trim_matches('"').to_string())
        }
    }
    flush(&mut name, &mut version, &mut out);
    for v in out.values_mut() {
        v.sort();
    }
    Ok(out)
}
fn cargo_home() -> Option<PathBuf> {
    env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| {
        env::var_os("USERPROFILE")
            .or_else(|| env::var_os("HOME"))
            .map(PathBuf::from)
            .map(|h| h.join(".cargo"))
    })
}
fn local_cargo_source_roots(package: &str, version: &str) -> Vec<PathBuf> {
    let Some(home) = cargo_home() else {
        return Vec::new();
    };
    let Ok(indexes) = fs::read_dir(home.join("registry").join("src")) else {
        return Vec::new();
    };
    let dir = format!("{package}-{version}");
    indexes
        .filter_map(Result::ok)
        .map(|e| e.path().join(&dir))
        .filter(|p| p.is_dir())
        .collect()
}
fn locally_installed_cargo_versions(package: &str) -> Vec<String> {
    let Some(home) = cargo_home() else {
        return Vec::new();
    };
    let Ok(indexes) = fs::read_dir(home.join("registry").join("src")) else {
        return Vec::new();
    };
    let prefix = format!("{package}-");
    let mut versions = BTreeSet::new();
    for index in indexes.filter_map(Result::ok) {
        let Ok(entries) = fs::read_dir(index.path()) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(version) = name
                .strip_prefix(&prefix)
                .filter(|version| !version.is_empty())
            {
                versions.insert(version.to_string());
            }
        }
    }
    versions.into_iter().collect()
}
fn node_dependency_grounding(root: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(root.join("package.json")).map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut deps = BTreeMap::<String, String>::new();
    for section in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(obj) = value.get(section).and_then(Value::as_object) {
            for (n, v) in obj {
                if let Some(v) = v.as_str() {
                    deps.entry(n.clone()).or_insert_with(|| v.to_string());
                }
            }
        }
    }
    Ok(json!({"ecosystem":"node","manifest":"package.json","dependencies":deps}))
}

fn dependency_api_search(
    root: &Path,
    processes: Option<&ProcessService>,
    ecosystem: &str,
    package: &str,
    query: &str,
    max_hits: usize,
) -> Result<Value, String> {
    if query.trim().is_empty() {
        return Err("dependency API search query is empty".into());
    }
    let ecosystem = ecosystem.to_ascii_lowercase();
    let mut roots = Vec::new();
    let exts: &[&str] = match ecosystem.as_str() {
        "cargo" | "rust" => {
            let canonical =
                canonical_dependency_name(root, package).unwrap_or_else(|| package.to_string());
            if let Some(service) = processes {
                if let Ok(records) = cargo_metadata_dependency_map(root, service) {
                    if let Some(packages) = records.get(&canonical) {
                        roots.extend(
                            packages
                                .iter()
                                .filter_map(|record| record.get("source_root"))
                                .filter_map(Value::as_str)
                                .map(PathBuf::from)
                                .filter(|path| path.is_dir()),
                        );
                    }
                }
            }
            if roots.is_empty() {
                let lock = cargo_lock_packages(root)?;
                for version in lock.get(&canonical).cloned().unwrap_or_default() {
                    roots.extend(local_cargo_source_roots(&canonical, &version));
                }
            }
            roots.sort();
            roots.dedup();
            &["rs"]
        }
        "node" | "npm" | "javascript" | "typescript" => {
            let p = root.join("node_modules").join(package);
            if p.is_dir() {
                roots.push(p)
            };
            &["js", "jsx", "ts", "tsx", "mjs", "cjs"]
        }
        other => {
            return Err(format!(
                "dependency.api_search has no local-source adapter for ecosystem `{other}` yet"
            ))
        }
    };
    if roots.is_empty() {
        return Err(format!("no locally installed source root found for dependency `{package}` in ecosystem `{ecosystem}`"));
    }
    let mut hits = Vec::new();
    for source in &roots {
        search_dependency_tree(source, source, query, exts, max_hits, &mut hits)?;
        if hits.len() >= max_hits {
            break;
        }
    }
    Ok(
        json!({"authority":"local_dependency_source","ecosystem":ecosystem,"package":package,"query":query,"source_roots":roots,"hits":hits}),
    )
}
fn search_dependency_tree(
    source: &Path,
    dir: &Path,
    query: &str,
    exts: &[&str],
    max: usize,
    hits: &mut Vec<Value>,
) -> Result<(), String> {
    if hits.len() >= max {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        if hits.len() >= max {
            break;
        }
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if ty.is_symlink() {
            continue;
        }
        if ty.is_dir() {
            search_dependency_tree(source, &path, query, exts, max, hits)?;
            continue;
        }
        let ext = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !exts.iter().any(|e| *e == ext) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if line.contains(query) {
                hits.push(json!({"path":path.strip_prefix(source).unwrap_or(&path),"line":i+1,"preview":line.trim().chars().take(320).collect::<String>()}));
                if hits.len() >= max {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn quality_snapshot(result: &CommandResult) -> QualitySnapshot {
    let mut s = QualitySnapshot::default();
    let mut api = 0;
    for d in &result.diagnostics {
        let level = d.level.to_ascii_lowercase();
        if level == "error" {
            s.errors += 1
        } else if level == "warning" {
            s.warnings += 1
        }
        let fp = format!(
            "{}|{}|{}|{}|{}",
            level,
            d.code.as_deref().unwrap_or(""),
            d.file
                .as_deref()
                .map(Path::to_string_lossy)
                .unwrap_or_default(),
            d.line_start.unwrap_or(0),
            d.message
        );
        s.fingerprints.insert(fp);
        let m = d.message.to_ascii_lowercase();
        if level == "error"
            && (m.contains("unresolved import")
                || m.contains("cannot find")
                || m.contains("not found")
                || m.contains("no field named")
                || m.contains("no method named")
                || m.contains("mismatched types"))
        {
            api += 1
        }
    }
    s.likely_api_version_mismatch = s.errors >= 3 && api >= 3;
    s
}
fn quality_snapshot_json(s: &QualitySnapshot) -> Value {
    json!({"errors":s.errors,"warnings":s.warnings,"diagnostic_fingerprints":s.fingerprints,"likely_api_version_mismatch":s.likely_api_version_mismatch})
}
fn quality_delta(previous: Option<&QualitySnapshot>, current: &QualitySnapshot) -> Value {
    let Some(p) = previous else {
        return json!({"baseline":true,"introduced":0,"resolved":0,"regressed":false,"improved":false,"unchanged":false,"catastrophic_regression":false});
    };
    let introduced = current.fingerprints.difference(&p.fingerprints).count();
    let resolved = p.fingerprints.difference(&current.fingerprints).count();
    let regressed = current.errors > p.errors
        || (current.errors == p.errors && current.warnings > p.warnings)
        || (introduced > resolved && current.errors >= p.errors);
    let improved = current.errors < p.errors
        || (current.errors == p.errors && current.warnings < p.warnings)
        || (resolved > introduced && current.errors <= p.errors);
    json!({"baseline":false,"previous_errors":p.errors,"current_errors":current.errors,"previous_warnings":p.warnings,"current_warnings":current.warnings,"introduced":introduced,"resolved":resolved,"regressed":regressed,"improved":improved,"unchanged":current.errors==p.errors&&current.warnings==p.warnings&&introduced==0&&resolved==0,"catastrophic_regression":current.errors>=p.errors.saturating_add(5)||(p.errors<=2&&current.errors>=10)})
}

fn candidate_decision(
    baseline: Option<&QualitySnapshot>,
    current: &QualitySnapshot,
    current_success: bool,
) -> CandidateDecision {
    if current_success {
        return CandidateDecision::KeepVerified;
    }
    let Some(baseline) = baseline else {
        return CandidateDecision::ObserveWithoutBaseline;
    };
    let delta = quality_delta(Some(baseline), current);
    if delta
        .get("regressed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        CandidateDecision::RollbackRegressed
    } else if delta
        .get("improved")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        CandidateDecision::KeepImproved
    } else {
        CandidateDecision::RollbackUnchanged
    }
}

fn quality_fingerprint(snapshot: &QualitySnapshot, success: bool) -> String {
    let signature = format!(
        "{}\0{}\0{}\0{}",
        success,
        snapshot.errors,
        snapshot.warnings,
        snapshot
            .fingerprints
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    format!("{:016x}", fnv1a64(signature.as_bytes()))
}

fn tool_failure_fingerprint(tool: &str, arguments: &Value, error: &str) -> String {
    let signature = format!(
        "{tool}\0{}\0{}\0{}",
        arguments,
        classify_tool_failure(tool, error),
        normalized_failure_signature(error)
    );
    format!("{:016x}", fnv1a64(signature.as_bytes()))
}

fn tool_recovery_hint(tool: &str, failure_class: &str, repeat_count: u32) -> String {
    let prefix = if repeat_count > 1 {
        format!(
            "The identical tool failure fingerprint has repeated {repeat_count} times. Do not repeat the same arguments; change recovery strategy now. "
        )
    } else {
        String::new()
    };
    let guidance = match failure_class {
        "path_authority" => {
            "Resolve the target through the active workspace authority and retry with a canonical workspace-relative path. Do not create a nested copy of the project to work around path validation."
        }
        "read_budget" => {
            "Use source.read as a bounded window. Continue with the returned next_offset, or use source.search to locate the required region before reading another bounded window."
        }
        "deterministic_mutation_guard" => {
            "Re-read the current bounded source region and use current exact text. If a guarded replacement is no longer unique, choose a narrower unique anchor or a deliberate transaction-safe whole-file write."
        }
        "dependency_grounding" => {
            "Do not retry the mutation yet. Inspect workspace.status for exact resolved dependencies, then use source.search with dependency_package against every required package before attempting API-sensitive source changes again."
        }
        "grounded_regeneration" => {
            "The controller already grounded the exact local dependency source and rejected the stale pre-grounding mutation. Regenerate the source change from the returned evidence before issuing another mutation; do not repeat the blocked payload."
        }
        "permission" => {
            "Stop repeating the operation. Verify the project/tool permission boundary or request the required governed permission before retrying."
        }
        "environment" => {
            "Verify the referenced file, executable, toolchain, or process exists in the active project environment before retrying."
        }
        _ if tool == "source.replace_text" => {
            "Do not repeat identical replacement arguments. Re-read the file and switch to current exact text or a different transaction-safe mutation strategy."
        }
        _ => {
            "Inspect the reported tool error and choose a different bounded recovery action rather than blindly repeating the same failed call."
        }
    };
    format!("{prefix}{guidance}")
}

fn dependency_grounding_packages_from_error(error: &str) -> Vec<String> {
    let marker = "Exact dependency-source evidence is missing for:";
    let Some(rest) = error.split_once(marker).map(|(_, rest)| rest) else {
        return Vec::new();
    };
    let package_text = rest.split('.').next().unwrap_or(rest);
    let mut packages = package_text
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    packages.sort();
    packages.dedup();
    packages
}

fn local_git_host_root(arguments: &Value, workspace_root: &Path) -> PathBuf {
    if let Some(root) = arguments.get("root").and_then(Value::as_str) {
        let trimmed = root.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Some(root) = env::var_os("CORTEX_LOCAL_GIT_ROOT") {
        if !root.is_empty() {
            return PathBuf::from(root);
        }
    }
    if let Ok(registry) = WorkspaceRegistry::open_default() {
        if let Ok(Some(record)) = registry.library_root() {
            return configured_local_git_root(&record.root);
        }
    }
    if cfg!(windows) {
        let d_drive = PathBuf::from(r"D:\");
        if d_drive.is_dir() {
            return d_drive.join("Cortex").join("Git");
        }
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local_app_data).join("Cortex").join("Git");
        }
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("cortex")
            .join("git");
    }
    workspace_root.join(".cortex").join("machine-local-git")
}

fn configured_local_git_root(storage_root: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        if storage_root.components().count() <= 2 {
            return storage_root.join("Cortex").join("Git");
        }
    }
    storage_root.join("Git")
}

fn process_succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn run_local_git_command(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
) -> Result<String, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
        .output()
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("{program} failed: {stdout}")
        } else {
            format!("{program} failed: {stderr}")
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn local_git_runtime(arguments: &Value) -> Result<String, String> {
    let requested = arguments
        .get("runtime")
        .and_then(Value::as_str)
        .unwrap_or("auto")
        .trim()
        .to_ascii_lowercase();
    match requested.as_str() {
        "auto" => {
            if process_succeeds("docker", &["compose", "version"]) {
                Ok("docker".into())
            } else if process_succeeds("podman", &["compose", "version"]) {
                Ok("podman".into())
            } else {
                Err(
                    "No supported local container runtime was detected. Install Docker Desktop or Podman Desktop, then retry git.host.bootstrap."
                        .into(),
                )
            }
        }
        "docker" if process_succeeds("docker", &["compose", "version"]) => Ok("docker".into()),
        "podman" if process_succeeds("podman", &["compose", "version"]) => Ok("podman".into()),
        "docker" | "podman" => Err(format!(
            "Requested container runtime `{requested}` is not available with Compose support."
        )),
        other => Err(format!(
            "Unsupported local Git host runtime `{other}`. Expected auto, docker or podman."
        )),
    }
}

fn local_git_compose(runtime: &str, forgejo_root: &Path, args: &[&str]) -> Result<String, String> {
    let mut command_args = vec!["compose".to_string(), "-f".into(), "compose.yaml".into()];
    command_args.extend(args.iter().map(|value| (*value).to_string()));
    run_local_git_command(runtime, &command_args, Some(forgejo_root))
}

fn cortex_forgejo_compose() -> &'static str {
    r#"services:
  forgejo:
    image: codeberg.org/forgejo/forgejo:15.0.7
    container_name: cortex-forgejo
    restart: unless-stopped
    environment:
      - USER_UID=1000
      - USER_GID=1000
      - FORGEJO__server__DOMAIN=127.0.0.1
      - FORGEJO__server__ROOT_URL=http://127.0.0.1:3000/
      - FORGEJO__server__SSH_DOMAIN=127.0.0.1
      - FORGEJO__server__SSH_PORT=2222
      - FORGEJO__service__DISABLE_REGISTRATION=true
      - FORGEJO__actions__ENABLED=false
      - FORGEJO__repository__DEFAULT_BRANCH=main
      - FORGEJO__repository__ENABLE_PUSH_CREATE_USER=true
      - FORGEJO__repository__ENABLE_PUSH_CREATE_ORG=true
      - FORGEJO__repository__DEFAULT_PUSH_CREATE_PRIVATE=true
      - FORGEJO__repository__FORCE_PRIVATE=true
    volumes:
      - ./data:/data
    ports:
      - "127.0.0.1:3000:3000"
      - "127.0.0.1:2222:22"
"#
}

fn local_git_host_status(arguments: &Value, workspace_root: &Path) -> Value {
    let root = local_git_host_root(arguments, workspace_root);
    let forgejo_root = root.join("forgejo");
    let compose = forgejo_root.join("compose.yaml");
    let data = forgejo_root.join("data");
    let requested_runtime = arguments
        .get("runtime")
        .and_then(Value::as_str)
        .unwrap_or("auto")
        .to_string();
    let runtime = local_git_runtime(arguments).ok();
    let running = if let Some(runtime) = runtime.as_deref() {
        run_local_git_command(
            runtime,
            &[
                "ps".into(),
                "--filter".into(),
                "name=cortex-forgejo".into(),
                "--format".into(),
                "{{.Names}}".into(),
            ],
            None,
        )
        .map(|output| output.lines().any(|line| line.trim() == "cortex-forgejo"))
        .unwrap_or(false)
    } else {
        false
    };
    json!({
        "authority": "cortex_machine_local_git_host",
        "root": root,
        "forgejo_root": forgejo_root,
        "compose": compose,
        "data": data,
        "configured": compose.is_file(),
        "runtime_requested": requested_runtime,
        "runtime_detected": runtime,
        "running": running,
        "web_url": "http://127.0.0.1:3000/",
        "ssh_endpoint": "ssh://git@127.0.0.1:2222/",
        "visibility": "private",
        "remote_default": "cortex-local",
        "provider": "forgejo"
    })
}

fn local_git_host_bootstrap(arguments: &Value, workspace_root: &Path) -> Result<Value, String> {
    let root = local_git_host_root(arguments, workspace_root);
    let forgejo_root = root.join("forgejo");
    let data = forgejo_root.join("data");
    let keys = root.join("keys");
    let start = arguments
        .get("start")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let runtime = if start {
        Some(local_git_runtime(arguments)?)
    } else {
        local_git_runtime(arguments).ok()
    };

    fs::create_dir_all(&data).map_err(|error| error.to_string())?;
    fs::create_dir_all(&keys).map_err(|error| error.to_string())?;

    let compose = forgejo_root.join("compose.yaml");
    let force = arguments
        .get("force")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let wrote_compose = if !compose.is_file() || force {
        fs::write(&compose, cortex_forgejo_compose()).map_err(|error| error.to_string())?;
        true
    } else {
        false
    };

    let start_output = if start {
        let runtime = runtime
            .as_deref()
            .ok_or_else(|| "local Git host runtime disappeared before startup".to_string())?;
        Some(local_git_compose(runtime, &forgejo_root, &["up", "-d"])?)
    } else {
        None
    };

    Ok(json!({
        "authority": "cortex_machine_local_git_host",
        "provider": "forgejo",
        "root": root,
        "forgejo_root": forgejo_root,
        "data": data,
        "compose": compose,
        "compose_written": wrote_compose,
        "runtime": runtime,
        "started": start,
        "start_output": start_output,
        "web_url": "http://127.0.0.1:3000/",
        "ssh_endpoint": "ssh://git@127.0.0.1:2222/",
        "onboarding": "Forgejo is loopback-only, registration-disabled, private-by-default, and Actions are disabled until Cortex provisions a repository-scoped trust-gated runner. Cortex can create its own local service identity through git.host.ensure_identity; manual web onboarding is only a fallback/admin path.",
        "next": "Run git.host.ensure_identity to create/reuse the Cortex Forgejo service account, protect its scoped API token with Windows user DPAPI, ensure/register its SSH key, then use git.project.attach_local."
    }))
}

fn local_git_host_ensure_ssh_key(
    arguments: &Value,
    workspace_root: &Path,
) -> Result<Value, String> {
    let root = local_git_host_root(arguments, workspace_root);
    let keys = root.join("keys");
    fs::create_dir_all(&keys).map_err(|error| error.to_string())?;
    let private_key = keys.join("cortex_forgejo_ed25519");
    let public_key = keys.join("cortex_forgejo_ed25519.pub");
    let known_hosts = keys.join("known_hosts");
    let force = arguments
        .get("force")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if force {
        if private_key.is_file() {
            fs::remove_file(&private_key).map_err(|error| error.to_string())?;
        }
        if public_key.is_file() {
            fs::remove_file(&public_key).map_err(|error| error.to_string())?;
        }
    }

    let created = if private_key.is_file() && public_key.is_file() {
        false
    } else {
        let args = vec![
            "-t".into(),
            "ed25519".into(),
            "-f".into(),
            private_key.to_string_lossy().to_string(),
            "-N".into(),
            String::new(),
            "-C".into(),
            "cortex-local-forgejo".into(),
        ];
        run_local_git_command("ssh-keygen", &args, None)?;
        true
    };

    let public_text = fs::read_to_string(&public_key)
        .map_err(|error| format!("failed to read generated Forgejo public key: {error}"))?;
    Ok(json!({
        "authority": "cortex_machine_local_git_host",
        "created": created,
        "private_key_path": private_key,
        "public_key_path": public_key,
        "known_hosts_path": known_hosts,
        "public_key": public_text.trim(),
        "private_key_exposed": false,
        "add_key_url": "http://127.0.0.1:3000/user/settings/keys",
        "next": "git.host.ensure_identity registers this key automatically for the Cortex service account. The web key page is retained only as an administrative fallback. Cortex can then perform SSH push-to-create without storing repository passwords."
    }))
}

fn local_git_slug(value: &str) -> Result<String, String> {
    let slug = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let slug = slug.trim_matches(|ch| ch == '-' || ch == '.').to_string();
    if slug.is_empty() || slug == "." || slug == ".." {
        Err(format!(
            "invalid local Forgejo repository identifier: {value}"
        ))
    } else {
        Ok(slug)
    }
}

fn local_git_command_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn local_git_project_attach(arguments: &Value, workspace_root: &Path) -> Result<Value, String> {
    let root = local_git_host_root(arguments, workspace_root);
    let owner = local_git_slug(
        arguments
            .get("owner")
            .and_then(Value::as_str)
            .unwrap_or("cortex"),
    )?;
    let default_repository = workspace_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    let repository = local_git_slug(
        arguments
            .get("repository")
            .and_then(Value::as_str)
            .unwrap_or(default_repository),
    )?;
    let remote_name = arguments
        .get("remote_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("cortex-local")
        .to_string();
    let initialize = arguments
        .get("initialize")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let overwrite_remote = arguments
        .get("overwrite_remote")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let push = arguments
        .get("push")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let existing_top_level = run_local_git_command(
        "git",
        &["rev-parse".into(), "--show-toplevel".into()],
        Some(workspace_root),
    )
    .ok();

    let initialized = if let Some(top_level) = existing_top_level {
        let project = fs::canonicalize(workspace_root).map_err(|error| error.to_string())?;
        let repository_root =
            fs::canonicalize(PathBuf::from(top_level.trim())).map_err(|error| error.to_string())?;
        if project != repository_root {
            return Err(format!(
                "Active project `{}` is nested inside Git repository `{}`. Cortex will not initialize or repoint a nested repository; attach the owning repository instead.",
                project.display(),
                repository_root.display()
            ));
        }
        false
    } else if initialize {
        run_local_git_command(
            "git",
            &["init".into(), "-b".into(), "main".into()],
            Some(workspace_root),
        )?;
        true
    } else {
        return Err(
            "Active project is not a Git repository. Set initialize=true to initialize it.".into(),
        );
    };

    let remote_url = format!("ssh://git@127.0.0.1:2222/{owner}/{repository}.git");
    let existing_remote = run_local_git_command(
        "git",
        &["remote".into(), "get-url".into(), remote_name.clone()],
        Some(workspace_root),
    )
    .ok();

    let remote_changed = match existing_remote.as_deref() {
        Some(existing) if existing.trim() == remote_url => false,
        Some(existing) if !overwrite_remote => {
            return Err(format!(
                "Git remote `{remote_name}` already points to `{existing}`. Cortex will not replace it unless overwrite_remote=true."
            ))
        }
        Some(_) => {
            run_local_git_command(
                "git",
                &[
                    "remote".into(),
                    "set-url".into(),
                    remote_name.clone(),
                    remote_url.clone(),
                ],
                Some(workspace_root),
            )?;
            true
        }
        None => {
            run_local_git_command(
                "git",
                &[
                    "remote".into(),
                    "add".into(),
                    remote_name.clone(),
                    remote_url.clone(),
                ],
                Some(workspace_root),
            )?;
            true
        }
    };

    let push_output = if push {
        let branch = run_local_git_command(
            "git",
            &["branch".into(), "--show-current".into()],
            Some(workspace_root),
        )?;
        if branch.trim().is_empty() {
            return Err(
                "Cannot push the local Forgejo remote because the project has no current branch/commit yet. Review the project, create the first commit, then retry with push=true."
                    .into(),
            );
        }

        let private_key = root.join("keys").join("cortex_forgejo_ed25519");
        let known_hosts = root.join("keys").join("known_hosts");
        if !private_key.is_file() {
            return Err(
                "The Cortex local-Forgejo SSH key does not exist. Run git.host.ensure_ssh_key and add its public key to Forgejo before push=true."
                    .into(),
            );
        }
        let ssh_command = format!(
            "ssh -i \"{}\" -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=\"{}\" -p 2222",
            local_git_command_path(&private_key),
            local_git_command_path(&known_hosts)
        );

        Some(run_local_git_command(
            "git",
            &[
                "-c".into(),
                format!("core.sshCommand={ssh_command}"),
                "push".into(),
                "-o".into(),
                "repo.private=true".into(),
                "-u".into(),
                remote_name.clone(),
                branch.trim().to_string(),
            ],
            Some(workspace_root),
        )?)
    } else {
        None
    };

    Ok(json!({
        "authority": "cortex_machine_local_git_host",
        "project_root": workspace_root,
        "initialized_git": initialized,
        "owner": owner,
        "repository": repository,
        "remote_name": remote_name,
        "remote_url": remote_url,
        "remote_changed": remote_changed,
        "pushed": push,
        "push_output": push_output,
        "existing_remotes_preserved": true,
        "private_by_default": true,
        "next": if push {
            "The project is attached to the local Forgejo authority."
        } else {
            "Review/commit the project, ensure the Cortex Forgejo SSH public key is added to the local account, then retry git.project.attach_local with push=true."
        }
    }))
}

fn forgejo_secret_path(root: &Path) -> PathBuf {
    root.join("secrets").join("forgejo_api_token.dpapi")
}

fn protect_windows_user_secret(path: &Path, secret: &str) -> Result<(), String> {
    if !cfg!(windows) {
        return Err(
            "Cortex local Forgejo API secret protection currently requires Windows DPAPI.".into(),
        );
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let script = r#"$plain=[Console]::In.ReadToEnd(); $secure=ConvertTo-SecureString $plain -AsPlainText -Force; ConvertFrom-SecureString $secure | Set-Content -LiteralPath $args[0] -Encoding UTF8"#;
    let mut child = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start DPAPI protection: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "DPAPI protection stdin was unavailable".to_string())?
        .write_all(secret.as_bytes())
        .map_err(|error| error.to_string())?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("DPAPI protection failed: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "DPAPI protection failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn unprotect_windows_user_secret(path: &Path) -> Result<String, String> {
    if !cfg!(windows) {
        return Err(
            "Cortex local Forgejo API secret protection currently requires Windows DPAPI.".into(),
        );
    }
    if !path.is_file() {
        return Err(format!(
            "Cortex local Forgejo API identity is not initialized: {}",
            path.display()
        ));
    }
    let script = r#"$encrypted=Get-Content -LiteralPath $args[0] -Raw; $secure=ConvertTo-SecureString $encrypted; $credential=New-Object System.Management.Automation.PSCredential('cortex',$secure); [Console]::Out.Write($credential.GetNetworkCredential().Password)"#;
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("failed to start DPAPI unprotection: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "DPAPI unprotection failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let secret = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if secret.is_empty() {
        Err("DPAPI unprotection returned an empty Forgejo token".into())
    } else {
        Ok(secret)
    }
}

fn forgejo_decode_chunked(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut cursor = 0usize;
    let mut decoded = Vec::new();
    loop {
        let line_end = body[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .map(|offset| cursor + offset)
            .ok_or_else(|| "invalid chunked Forgejo response".to_string())?;
        let size_text = String::from_utf8_lossy(&body[cursor..line_end]);
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|error| format!("invalid Forgejo chunk size: {error}"))?;
        cursor = line_end + 2;
        if size == 0 {
            break;
        }
        let end = cursor.saturating_add(size);
        if end > body.len() {
            return Err("truncated chunked Forgejo response".into());
        }
        decoded.extend_from_slice(&body[cursor..end]);
        cursor = end.saturating_add(2);
        if cursor > body.len() {
            return Err("invalid chunk terminator in Forgejo response".into());
        }
    }
    Ok(decoded)
}

fn forgejo_http_request(
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> Result<(u16, Value), String> {
    if !path.starts_with('/') || path.contains('\r') || path.contains('\n') {
        return Err("invalid local Forgejo API path".into());
    }
    let mut stream = TcpStream::connect_timeout(
        &"127.0.0.1:3000"
            .parse()
            .map_err(|error| format!("invalid loopback Forgejo socket: {error}"))?,
        Duration::from_secs(3),
    )
    .map_err(|error| format!("local Forgejo API connect failed: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(8)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(8)))
        .map_err(|error| error.to_string())?;

    let body_bytes = match body {
        Some(value) => serde_json::to_vec(value).map_err(|error| error.to_string())?,
        None => Vec::new(),
    };
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:3000\r\nAccept: application/json\r\nConnection: close\r\nContent-Length: {}\r\n",
        body_bytes.len()
    );
    if let Some(token) = token {
        request.push_str("Authorization: token ");
        request.push_str(token);
        request.push_str("\r\n");
    }
    if body.is_some() {
        request.push_str("Content-Type: application/json\r\n");
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|error| error.to_string())?;
    if !body_bytes.is_empty() {
        stream
            .write_all(&body_bytes)
            .map_err(|error| error.to_string())?;
    }
    stream.flush().map_err(|error| error.to_string())?;

    let mut bytes = Vec::new();
    stream
        .take(8 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "invalid Forgejo HTTP response".to_string())?;
    let header_text = String::from_utf8_lossy(&bytes[..split]);
    let status = header_text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| "Forgejo response did not contain an HTTP status".to_string())?
        .parse::<u16>()
        .map_err(|error| error.to_string())?;
    let chunked = header_text.lines().any(|line| {
        line.to_ascii_lowercase()
            .starts_with("transfer-encoding: chunked")
    });
    let raw_body = &bytes[split + 4..];
    let body_bytes = if chunked {
        forgejo_decode_chunked(raw_body)?
    } else {
        raw_body.to_vec()
    };
    let value = if body_bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body_bytes)
            .unwrap_or_else(|_| json!({"text": String::from_utf8_lossy(&body_bytes).to_string()}))
    };
    Ok((status, value))
}

fn forgejo_api_json(
    method: &str,
    path: &str,
    token: &str,
    body: Option<&Value>,
) -> Result<Value, String> {
    let (status, value) = forgejo_http_request(method, path, Some(token), body)?;
    if (200..300).contains(&status) {
        Ok(value)
    } else {
        Err(format!(
            "Forgejo API {method} {path} failed with HTTP {status}: {}",
            compact_json_chars(&value, 2_000)
        ))
    }
}

fn local_git_remote_identity(
    arguments: &Value,
    workspace_root: &Path,
) -> Result<(String, String, String), String> {
    let remote_name = arguments
        .get("remote_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("cortex-local")
        .to_string();
    if let (Some(owner), Some(repository)) = (
        arguments.get("owner").and_then(Value::as_str),
        arguments.get("repository").and_then(Value::as_str),
    ) {
        return Ok((
            local_git_slug(owner)?,
            local_git_slug(repository)?,
            remote_name,
        ));
    }
    let remote = run_local_git_command(
        "git",
        &["remote".into(), "get-url".into(), remote_name.clone()],
        Some(workspace_root),
    )
    .map_err(|_| {
        format!(
            "Local Forgejo remote `{remote_name}` is not attached. Run git.project.attach_local first."
        )
    })?;
    let normalized = remote.trim().trim_end_matches(".git");
    let marker = "127.0.0.1:2222/";
    let suffix = normalized
        .split_once(marker)
        .map(|(_, suffix)| suffix)
        .ok_or_else(|| {
            format!("Remote `{remote_name}` is not a Cortex local-Forgejo SSH URL: {remote}")
        })?;
    let mut parts = suffix.split('/').filter(|part| !part.trim().is_empty());
    let owner = parts
        .next()
        .ok_or_else(|| "local Forgejo remote is missing owner".to_string())?;
    let repository = parts
        .next()
        .ok_or_else(|| "local Forgejo remote is missing repository".to_string())?;
    Ok((
        local_git_slug(owner)?,
        local_git_slug(repository)?,
        remote_name,
    ))
}

fn local_git_host_ensure_identity(
    arguments: &Value,
    workspace_root: &Path,
) -> Result<Value, String> {
    let root = local_git_host_root(arguments, workspace_root);
    let runtime = local_git_runtime(arguments)?;
    let host = local_git_host_status(arguments, workspace_root);
    if !host
        .get("running")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(
            "The Cortex local Forgejo host is not running. Run git.host.bootstrap first.".into(),
        );
    }

    let username = local_git_slug(
        arguments
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or("cortex"),
    )?;
    let email = arguments
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or("cortex@local.invalid")
        .trim()
        .to_string();
    let token_path = forgejo_secret_path(&root);
    let mut created_user = false;
    let mut created_token = false;

    let users = run_local_git_command(
        &runtime,
        &[
            "exec".into(),
            "cortex-forgejo".into(),
            "forgejo".into(),
            "admin".into(),
            "user".into(),
            "list".into(),
        ],
        None,
    )?;
    let user_exists = users.lines().any(|line| {
        line.split_whitespace()
            .any(|field| field.eq_ignore_ascii_case(&username))
    });
    if !user_exists {
        run_local_git_command(
            &runtime,
            &[
                "exec".into(),
                "cortex-forgejo".into(),
                "forgejo".into(),
                "admin".into(),
                "user".into(),
                "create".into(),
                "--username".into(),
                username.clone(),
                "--email".into(),
                email.clone(),
                "--random-password".into(),
                "--random-password-length".into(),
                "32".into(),
                "--must-change-password=false".into(),
            ],
            None,
        )?;
        created_user = true;
    }

    let token = if token_path.is_file() {
        unprotect_windows_user_secret(&token_path)?
    } else {
        let token_name = format!(
            "cortex-desktop-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        );
        let token = run_local_git_command(
            &runtime,
            &[
                "exec".into(),
                "cortex-forgejo".into(),
                "forgejo".into(),
                "admin".into(),
                "user".into(),
                "generate-access-token".into(),
                "--username".into(),
                username.clone(),
                "--token-name".into(),
                token_name,
                "--raw".into(),
                "--scopes".into(),
                "write:repository,write:issue,write:user".into(),
            ],
            None,
        )?;
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err("Forgejo did not return a service API token".into());
        }
        protect_windows_user_secret(&token_path, &token)?;
        created_token = true;
        token
    };

    let key = local_git_host_ensure_ssh_key(arguments, workspace_root)?;
    let public_key = key
        .get("public_key")
        .and_then(Value::as_str)
        .ok_or_else(|| "Cortex Forgejo public key was not returned".to_string())?;
    let key_body = json!({
        "title": "Cortex local Git",
        "key": public_key,
        "read_only": false
    });
    let (key_status, key_value) =
        forgejo_http_request("POST", "/api/v1/user/keys", Some(&token), Some(&key_body))?;
    let key_registered = matches!(key_status, 200 | 201);
    let key_already_present = key_status == 422;
    if !key_registered && !key_already_present {
        return Err(format!(
            "Forgejo rejected the Cortex SSH public key with HTTP {key_status}: {}",
            compact_json_chars(&key_value, 2_000)
        ));
    }

    let current = forgejo_api_json("GET", "/api/v1/user", &token, None)?;
    Ok(json!({
        "authority": "cortex_machine_local_git_identity",
        "username": username,
        "email": email,
        "created_user": created_user,
        "created_token": created_token,
        "token_storage": {
            "kind": "windows_dpapi_current_user",
            "path": token_path,
            "secret_returned": false
        },
        "ssh_key_registered": key_registered,
        "ssh_key_already_present": key_already_present,
        "forgejo_user": current,
        "ready": true
    }))
}

fn forgejo_optional_endpoint(path: &str, token: &str) -> Value {
    match forgejo_http_request("GET", path, Some(token), None) {
        Ok((status, value)) if (200..300).contains(&status) => {
            json!({"available": true, "status": status, "value": value})
        }
        Ok((status, value)) if status == 404 => {
            json!({"available": false, "status": status, "reason": "endpoint_or_repository_not_available", "value": value})
        }
        Ok((status, value)) => {
            json!({"available": false, "status": status, "reason": "forgejo_api_error", "value": value})
        }
        Err(error) => {
            json!({"available": false, "status": 0, "reason": error})
        }
    }
}

fn forgejo_repository_refresh(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    let root = local_git_host_root(arguments, workspace.root());
    let token = unprotect_windows_user_secret(&forgejo_secret_path(&root))?;
    let (owner, repository, remote_name) = local_git_remote_identity(arguments, workspace.root())?;
    let limit = arg_u64(arguments, "limit").unwrap_or(40).clamp(1, 100);
    let repository_path = format!("/api/v1/repos/{owner}/{repository}");
    let repository_value = forgejo_api_json("GET", &repository_path, &token, None)?;
    let issues = forgejo_optional_endpoint(
        &format!("/api/v1/repos/{owner}/{repository}/issues?state=open&type=issues&limit={limit}"),
        &token,
    );
    let pulls = forgejo_optional_endpoint(
        &format!("/api/v1/repos/{owner}/{repository}/pulls?state=open&limit={limit}"),
        &token,
    );
    let releases = forgejo_optional_endpoint(
        &format!("/api/v1/repos/{owner}/{repository}/releases?limit={limit}"),
        &token,
    );
    let actions = forgejo_optional_endpoint(
        &format!("/api/v1/repos/{owner}/{repository}/actions/runs?limit={limit}"),
        &token,
    );
    let snapshot = json!({
        "schema_version": 1,
        "authority": "cortex_forgejo_provider_snapshot",
        "refreshed_unix_ms": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        "owner": owner,
        "repository_name": repository,
        "remote_name": remote_name,
        "web_url": format!("http://127.0.0.1:3000/{owner}/{repository}"),
        "repository": repository_value,
        "issues": issues,
        "pulls": pulls,
        "actions": actions,
        "releases": releases
    });
    let cache = workspace
        .cortex_state_dir()
        .join("repositories")
        .join("forgejo-provider-snapshot.json");
    if let Some(parent) = cache.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = cache.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(&snapshot).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::rename(&temporary, &cache).map_err(|error| error.to_string())?;
    Ok(json!({
        "snapshot": snapshot,
        "cache": cache,
        "secret_returned": false
    }))
}

fn forgejo_issue_create(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    let root = local_git_host_root(arguments, workspace.root());
    let token = unprotect_windows_user_secret(&forgejo_secret_path(&root))?;
    let (owner, repository, _) = local_git_remote_identity(arguments, workspace.root())?;
    let title = required_string(arguments, "title")?.trim();
    if title.is_empty() {
        return Err("Forgejo issue title cannot be empty".into());
    }
    let body = arguments
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let created = forgejo_api_json(
        "POST",
        &format!("/api/v1/repos/{owner}/{repository}/issues"),
        &token,
        Some(&json!({"title": title, "body": body})),
    )?;
    let _ = forgejo_repository_refresh(arguments, workspace);
    Ok(json!({
        "authority": "cortex_forgejo_provider",
        "owner": owner,
        "repository": repository,
        "issue": created
    }))
}

fn git_branch_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty()
        || value.starts_with('-')
        || value.contains("..")
        || value.contains("@{")
        || value.ends_with('.')
        || value.ends_with('/')
        || value.chars().any(|ch| {
            ch.is_control() || matches!(ch, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
    {
        return Err(format!("invalid Git branch name: {value}"));
    }
    Ok(value.to_string())
}

fn git_current_branch(root: &Path) -> Result<String, String> {
    let value = run_local_git_command(
        "git",
        &["branch".into(), "--show-current".into()],
        Some(root),
    )?;
    let value = value.trim().to_string();
    if value.is_empty() {
        Err("Git repository is in detached-HEAD state".into())
    } else {
        Ok(value)
    }
}

fn git_requested_paths(arguments: &Value, workspace: &Workspace) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    if let Some(values) = arguments.get("paths").and_then(Value::as_array) {
        for value in values {
            let relative = value
                .as_str()
                .ok_or_else(|| "Git paths must be strings".to_string())?;
            let resolved = workspace
                .resolve(relative)
                .map_err(|error| error.to_string())?;
            let relative = resolved
                .strip_prefix(workspace.root())
                .map_err(|_| "Git path escaped the active project".to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if relative.is_empty() {
                return Err("Git path cannot be the project root".into());
            }
            paths.push(relative);
        }
    }
    Ok(paths)
}

fn git_large_file_review(
    workspace: &Workspace,
    requested: &[String],
    all: bool,
) -> Result<Vec<Value>, String> {
    let candidates = if all {
        run_local_git_command(
            "git",
            &[
                "ls-files".into(),
                "-co".into(),
                "--exclude-standard".into(),
                "-z".into(),
            ],
            Some(workspace.root()),
        )?
        .split('\0')
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>()
    } else {
        requested.to_vec()
    };

    let mut review = Vec::new();
    for relative in candidates {
        let Ok(path) = workspace.resolve(&relative) else {
            continue;
        };
        let Ok(metadata) = fs::metadata(path) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() < 25 * 1024 * 1024 {
            continue;
        }
        review.push(json!({
            "path": relative,
            "bytes": metadata.len(),
            "vault_review": true,
            "hard_git_guard": metadata.len() >= 100 * 1024 * 1024
        }));
    }
    Ok(review)
}

fn git_changes_snapshot(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    GitAdapter::detect(workspace.root())
        .ok_or_else(|| "Git is not active for this project".to_string())?;
    let max_chars = arg_u64(arguments, "diff_max_chars")
        .unwrap_or(120_000)
        .clamp(2_000, 500_000) as usize;
    let status = run_local_git_command(
        "git",
        &["status".into(), "--porcelain=v1".into(), "--branch".into()],
        Some(workspace.root()),
    )?;
    let staged = run_local_git_command(
        "git",
        &["diff".into(), "--cached".into(), "--no-ext-diff".into()],
        Some(workspace.root()),
    )?;
    let unstaged = run_local_git_command(
        "git",
        &["diff".into(), "--no-ext-diff".into()],
        Some(workspace.root()),
    )?;
    let remotes = run_local_git_command(
        "git",
        &["remote".into(), "-v".into()],
        Some(workspace.root()),
    )
    .unwrap_or_default();
    Ok(json!({
        "authority": "cortex_local_git_changes",
        "branch": git_current_branch(workspace.root()).ok(),
        "status": status,
        "staged_diff": staged.chars().take(max_chars).collect::<String>(),
        "unstaged_diff": unstaged.chars().take(max_chars).collect::<String>(),
        "staged_diff_truncated": staged.chars().count() > max_chars,
        "unstaged_diff_truncated": unstaged.chars().count() > max_chars,
        "remotes": remotes
    }))
}

fn git_stage(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    let all = arguments
        .get("all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let paths = git_requested_paths(arguments, workspace)?;
    if !all && paths.is_empty() {
        return Err("git.stage requires paths or all=true".into());
    }
    let review = git_large_file_review(workspace, &paths, all)?;
    let hard_guard = review.iter().any(|entry| {
        entry
            .get("hard_git_guard")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    });
    let allow_large_git = arguments
        .get("allow_large_git")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if hard_guard && !allow_large_git {
        return Err(format!(
            "Cortex blocked direct Git staging for one or more 100 MiB+ files. Store them in Cortex Vault and commit pointer metadata, or explicitly approve allow_large_git=true.\n{}",
            compact_json_chars(&Value::Array(review), 8_000)
        ));
    }

    let mut args = vec!["add".to_string()];
    if all {
        args.push("-A".into());
    } else {
        args.push("--".into());
        args.extend(paths.clone());
    }
    run_local_git_command("git", &args, Some(workspace.root()))?;
    Ok(json!({
        "authority": "cortex_local_git",
        "staged": if all { json!("all") } else { json!(paths) },
        "large_file_review": review,
        "large_git_override": allow_large_git
    }))
}

fn git_unstage(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    let all = arguments
        .get("all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let paths = git_requested_paths(arguments, workspace)?;
    if !all && paths.is_empty() {
        return Err("git.unstage requires paths or all=true".into());
    }
    let head_exists = run_local_git_command(
        "git",
        &["rev-parse".into(), "--verify".into(), "HEAD".into()],
        Some(workspace.root()),
    )
    .is_ok();

    let mut args = if head_exists {
        vec!["restore".into(), "--staged".into()]
    } else {
        vec![
            "rm".into(),
            "--cached".into(),
            "-r".into(),
            "--ignore-unmatch".into(),
        ]
    };
    if all {
        args.push(".".into());
    } else {
        if head_exists {
            args.push("--".into());
        }
        args.extend(paths.clone());
    }
    run_local_git_command("git", &args, Some(workspace.root()))?;
    Ok(json!({
        "authority": "cortex_local_git",
        "unstaged": if all { json!("all") } else { json!(paths) },
        "working_tree_preserved": true
    }))
}

fn git_commit(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    let message = required_string(arguments, "message")?.trim();
    if message.is_empty() {
        return Err("Git commit message cannot be empty".into());
    }

    let staged_changes_exist = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(workspace.root())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("failed to inspect staged Git changes: {error}"))?
        .code()
        == Some(1);
    if !staged_changes_exist {
        return Err("No staged changes are available to commit".into());
    }

    let configured_name = run_local_git_command(
        "git",
        &["config".into(), "--get".into(), "user.name".into()],
        Some(workspace.root()),
    )
    .ok()
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty());
    let configured_email = run_local_git_command(
        "git",
        &["config".into(), "--get".into(), "user.email".into()],
        Some(workspace.root()),
    )
    .ok()
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty());

    let name = arguments
        .get("author_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or(configured_name)
        .unwrap_or_else(|| "Cortex User".into());
    let email = arguments
        .get("author_email")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or(configured_email)
        .unwrap_or_else(|| "cortex@local.invalid".into());

    let output = run_local_git_command(
        "git",
        &[
            "-c".into(),
            format!("user.name={name}"),
            "-c".into(),
            format!("user.email={email}"),
            "commit".into(),
            "-m".into(),
            message.to_string(),
        ],
        Some(workspace.root()),
    )?;
    let commit = run_local_git_command(
        "git",
        &["rev-parse".into(), "HEAD".into()],
        Some(workspace.root()),
    )?;
    Ok(json!({
        "authority": "cortex_local_git",
        "commit": commit.trim(),
        "message": message,
        "auto_staged": false,
        "output": output
    }))
}

fn git_branch_create(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    let name = git_branch_name(required_string(arguments, "name")?)?;
    let switch = arguments
        .get("switch")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let args = if switch {
        vec!["switch".into(), "-c".into(), name.clone()]
    } else {
        vec!["branch".into(), name.clone()]
    };
    run_local_git_command("git", &args, Some(workspace.root()))?;
    Ok(json!({"authority":"cortex_local_git","branch":name,"switched":switch}))
}

fn git_branch_switch(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    let name = git_branch_name(required_string(arguments, "name")?)?;
    run_local_git_command(
        "git",
        &["switch".into(), name.clone()],
        Some(workspace.root()),
    )?;
    Ok(json!({"authority":"cortex_local_git","branch":name}))
}

fn run_git_alternate_index(
    workspace_root: &Path,
    index_file: &Path,
    args: &[String],
    checkpoint_identity: bool,
) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(workspace_root)
        .env("GIT_INDEX_FILE", index_file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if checkpoint_identity {
        command
            .env("GIT_AUTHOR_NAME", "Cortex Safety Checkpoint")
            .env("GIT_AUTHOR_EMAIL", "cortex@local.invalid")
            .env("GIT_COMMITTER_NAME", "Cortex Safety Checkpoint")
            .env("GIT_COMMITTER_EMAIL", "cortex@local.invalid");
    }
    let output = command
        .output()
        .map_err(|error| format!("failed to run Git checkpoint command: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_safety_create(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    GitAdapter::detect(workspace.root())
        .ok_or_else(|| "Git is not active for this project".to_string())?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let checkpoint_id = match arguments.get("id").and_then(Value::as_str) {
        Some(value) if !value.trim().is_empty() => local_git_slug(value)?,
        _ => now.to_string(),
    };
    let label = arguments
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("Cortex safety checkpoint")
        .trim();
    let ref_name = format!("refs/cortex/checkpoints/{checkpoint_id}");

    let recovery_dir = workspace
        .cortex_state_dir()
        .join("recovery")
        .join("git-indexes");
    fs::create_dir_all(&recovery_dir).map_err(|error| error.to_string())?;
    let alternate_index = recovery_dir.join(format!("{checkpoint_id}.index"));
    let _ = fs::remove_file(&alternate_index);

    let head = run_local_git_command(
        "git",
        &["rev-parse".into(), "--verify".into(), "HEAD".into()],
        Some(workspace.root()),
    )
    .ok()
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty());

    if head.is_some() {
        run_git_alternate_index(
            workspace.root(),
            &alternate_index,
            &["read-tree".into(), "HEAD".into()],
            false,
        )?;
    } else {
        run_git_alternate_index(
            workspace.root(),
            &alternate_index,
            &["read-tree".into(), "--empty".into()],
            false,
        )?;
    }

    run_git_alternate_index(
        workspace.root(),
        &alternate_index,
        &["add".into(), "-A".into()],
        false,
    )?;
    let tree = run_git_alternate_index(
        workspace.root(),
        &alternate_index,
        &["write-tree".into()],
        false,
    )?;

    let mut commit_args = vec!["commit-tree".into(), tree.clone()];
    if let Some(parent) = &head {
        commit_args.push("-p".into());
        commit_args.push(parent.clone());
    }
    commit_args.push("-m".into());
    commit_args.push(format!("{label} [{checkpoint_id}]"));

    let commit = run_git_alternate_index(workspace.root(), &alternate_index, &commit_args, true)?;
    run_local_git_command(
        "git",
        &["update-ref".into(), ref_name.clone(), commit.clone()],
        Some(workspace.root()),
    )?;
    let _ = fs::remove_file(&alternate_index);

    Ok(json!({
        "authority": "cortex_git_safety_checkpoint",
        "id": checkpoint_id,
        "ref": ref_name,
        "commit": commit,
        "tree": tree,
        "parent": head,
        "user_branch_unchanged": true,
        "user_index_unchanged": true
    }))
}

fn git_safety_list(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    let limit = arg_u64(arguments, "limit").unwrap_or(32).clamp(1, 200) as usize;
    let output = run_local_git_command(
        "git",
        &[
            "for-each-ref".into(),
            "--sort=-creatordate".into(),
            "--format=%(refname)%1f%(objectname)%1f%(creatordate:iso8601)%1f%(subject)".into(),
            "refs/cortex/checkpoints/".into(),
        ],
        Some(workspace.root()),
    )?;
    let checkpoints = output
        .lines()
        .take(limit)
        .filter_map(|line| {
            let mut fields = line.split('\x1f');
            Some(json!({
                "ref": fields.next()?,
                "commit": fields.next()?,
                "created": fields.next()?,
                "subject": fields.next().unwrap_or("")
            }))
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "authority": "cortex_git_safety_checkpoint",
        "checkpoints": checkpoints
    }))
}

fn git_local_transport(
    arguments: &Value,
    workspace: &Workspace,
) -> Result<(String, String), String> {
    let remote = arguments
        .get("remote_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("cortex-local")
        .to_string();
    let root = local_git_host_root(arguments, workspace.root());
    let private_key = root.join("keys").join("cortex_forgejo_ed25519");
    let known_hosts = root.join("keys").join("known_hosts");
    if !private_key.is_file() {
        return Err("Cortex Forgejo SSH identity is not initialized".into());
    }
    let ssh_command = format!(
        "ssh -i \"{}\" -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=\"{}\" -p 2222",
        local_git_command_path(&private_key),
        local_git_command_path(&known_hosts)
    );
    Ok((remote, ssh_command))
}

fn git_fetch_local(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    let (remote, ssh_command) = git_local_transport(arguments, workspace)?;
    let prune = arguments
        .get("prune")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut args = vec![
        "-c".into(),
        format!("core.sshCommand={ssh_command}"),
        "fetch".into(),
    ];
    if prune {
        args.push("--prune".into());
    }
    args.push(remote.clone());
    let output = run_local_git_command("git", &args, Some(workspace.root()))?;
    Ok(json!({
        "authority":"cortex_local_forgejo_git_transport",
        "operation":"fetch",
        "remote":remote,
        "prune":prune,
        "working_tree_mutated":false,
        "output":output,
        "sync":git_sync_status(arguments, workspace)?
    }))
}

fn git_sync_status(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    let remote = arguments
        .get("remote_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("cortex-local")
        .to_string();
    let branch = git_current_branch(workspace.root())?;
    let local_commit = run_local_git_command(
        "git",
        &["rev-parse".into(), "HEAD".into()],
        Some(workspace.root()),
    )?;
    let remote_ref = format!("refs/remotes/{remote}/{branch}");
    let remote_commit = run_local_git_command(
        "git",
        &["rev-parse".into(), "--verify".into(), remote_ref.clone()],
        Some(workspace.root()),
    )
    .ok();
    let (ahead, behind) = if remote_commit.is_some() {
        let counts = run_local_git_command(
            "git",
            &[
                "rev-list".into(),
                "--left-right".into(),
                "--count".into(),
                format!("HEAD...{remote_ref}"),
            ],
            Some(workspace.root()),
        )?;
        let mut fields = counts.split_whitespace();
        let ahead = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let behind = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        (ahead, behind)
    } else {
        (0, 0)
    };
    let dirty = !run_local_git_command(
        "git",
        &["status".into(), "--porcelain".into()],
        Some(workspace.root()),
    )?
    .trim()
    .is_empty();
    Ok(json!({
        "authority":"cortex_git_sync_status",
        "remote":remote,
        "branch":branch,
        "local_commit":local_commit.trim(),
        "remote_ref":remote_ref,
        "remote_commit":remote_commit.as_deref().map(str::trim),
        "remote_tracking_present":remote_commit.is_some(),
        "ahead":ahead,
        "behind":behind,
        "diverged":ahead > 0 && behind > 0,
        "dirty":dirty,
        "safe_push_candidate":remote_commit.is_none() || behind == 0
    }))
}

fn local_git_host_backup(arguments: &Value, workspace_root: &Path) -> Result<Value, String> {
    let root = local_git_host_root(arguments, workspace_root);
    let runtime = local_git_runtime(arguments)?;
    let host = local_git_host_status(arguments, workspace_root);
    if !host
        .get("running")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err("The Cortex local Forgejo host is not running. Start it before backup.".into());
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let file_name = format!("forgejo-dump-{stamp}.zip");
    let container_path = format!("/tmp/{file_name}");
    let registry = WorkspaceRegistry::open_default().ok();
    let backup_root = registry
        .as_ref()
        .and_then(|registry| registry.library_layout().ok().flatten())
        .map(|layout| layout.backups.join("Forgejo"))
        .unwrap_or_else(|| root.join("backups").join("forgejo"));
    fs::create_dir_all(&backup_root).map_err(|error| error.to_string())?;
    let destination = backup_root.join(&file_name);

    let dump_result = run_local_git_command(
        &runtime,
        &[
            "exec".into(),
            "cortex-forgejo".into(),
            "forgejo".into(),
            "dump".into(),
            "--file".into(),
            container_path.clone(),
            "--quiet".into(),
        ],
        None,
    );
    if let Err(error) = dump_result {
        let _ = run_local_git_command(
            &runtime,
            &[
                "exec".into(),
                "cortex-forgejo".into(),
                "rm".into(),
                "-f".into(),
                container_path.clone(),
            ],
            None,
        );
        return Err(error);
    }

    let copy_result = run_local_git_command(
        &runtime,
        &[
            "cp".into(),
            format!("cortex-forgejo:{container_path}"),
            destination.to_string_lossy().to_string(),
        ],
        None,
    );
    let _ = run_local_git_command(
        &runtime,
        &[
            "exec".into(),
            "cortex-forgejo".into(),
            "rm".into(),
            "-f".into(),
            container_path,
        ],
        None,
    );
    copy_result?;

    let metadata = fs::metadata(&destination).map_err(|error| error.to_string())?;
    if metadata.len() == 0 {
        let _ = fs::remove_file(&destination);
        return Err("Forgejo dump copy produced an empty backup file".into());
    }
    let sha256 = windows_sha256(&destination)?;
    let offsite = registry
        .as_ref()
        .and_then(|registry| registry.offsite_backup_root().ok().flatten())
        .map(|root| -> Result<Value, String> {
            let offsite_root = root.join("Forgejo");
            fs::create_dir_all(&offsite_root).map_err(|error| error.to_string())?;
            let offsite_destination = offsite_root.join(&file_name);
            fs::copy(&destination, &offsite_destination).map_err(|error| error.to_string())?;
            let offsite_sha256 = windows_sha256(&offsite_destination)?;
            if offsite_sha256 != sha256 {
                let _ = fs::remove_file(&offsite_destination);
                return Err("Forgejo offsite backup copy failed SHA-256 verification".into());
            }
            Ok(json!({
                "path":offsite_destination,
                "sha256":offsite_sha256,
                "local_sync_folder_verified":true,
                "cloud_upload_confirmed":false
            }))
        })
        .transpose()?;
    Ok(json!({
        "authority":"cortex_forgejo_backup",
        "backup":destination,
        "bytes":metadata.len(),
        "sha256":sha256,
        "provider":"forgejo",
        "complete_dump":true,
        "offsite":offsite,
        "restore_performed":false,
        "restore_policy":"offline_explicit_approval_required"
    }))
}

fn git_push_local(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    let (remote, ssh_command) = git_local_transport(arguments, workspace)?;
    let branch = git_current_branch(workspace.root())?;

    let mut args = vec![
        "-c".into(),
        format!("core.sshCommand={ssh_command}"),
        "push".into(),
    ];
    if arguments
        .get("set_upstream")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        args.push("-u".into());
    }
    args.push(remote.clone());
    args.push(branch.clone());
    let output = run_local_git_command("git", &args, Some(workspace.root()))?;
    Ok(json!({
        "authority":"cortex_local_forgejo_git_transport",
        "remote":remote,
        "branch":branch,
        "force":false,
        "output":output,
        "sync":git_sync_status(arguments, workspace)?
    }))
}

fn cortex_library_root() -> Result<PathBuf, String> {
    let registry = WorkspaceRegistry::open_default()?;
    registry
        .library_root()?
        .map(|record| record.root)
        .ok_or_else(|| "Cortex Vault root is not configured".to_string())
}

fn windows_sha256(path: &Path) -> Result<String, String> {
    if !cfg!(windows) {
        return Err("Cortex Vault SHA-256 ingestion currently requires Windows PowerShell.".into());
    }
    let script = r#"[Console]::Out.Write((Get-FileHash -Algorithm SHA256 -LiteralPath $args[0]).Hash.ToLowerInvariant())"#;
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("failed to calculate SHA-256: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "SHA-256 calculation failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let hash = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_ascii_lowercase();
    if hash.len() != 64 || !hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("PowerShell returned an invalid SHA-256 value".into());
    }
    Ok(hash)
}

fn git_vault_pointer_root(workspace: &Workspace) -> PathBuf {
    workspace.root().join(".cortex-vault").join("pointers")
}

fn git_vault_pointer_path(workspace: &Workspace, logical_path: &str) -> Result<PathBuf, String> {
    let resolved = workspace
        .resolve(logical_path)
        .map_err(|error| error.to_string())?;
    let relative = resolved
        .strip_prefix(workspace.root())
        .map_err(|_| "Vault logical path escaped the project".to_string())?;
    let mut pointer = git_vault_pointer_root(workspace).join(relative);
    let file_name = pointer
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Vault logical path does not have a valid file name".to_string())?
        .to_string();
    pointer.set_file_name(format!("{file_name}.cortex-vault.json"));
    Ok(pointer)
}

fn git_vault_object_path(vault_root: &Path, sha256: &str) -> Result<PathBuf, String> {
    if sha256.len() != 64 || !sha256.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("invalid Cortex Vault SHA-256 object identity".into());
    }
    Ok(vault_root
        .join("objects")
        .join("sha256")
        .join(&sha256[..2])
        .join(sha256))
}

fn git_visible_files(workspace: &Workspace) -> Result<Vec<String>, String> {
    let output = run_local_git_command(
        "git",
        &[
            "ls-files".into(),
            "-co".into(),
            "--exclude-standard".into(),
            "-z".into(),
        ],
        Some(workspace.root()),
    )?;
    Ok(output
        .split('\0')
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .collect())
}

fn git_vault_audit(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    let review_bytes = arg_u64(arguments, "review_bytes").unwrap_or(25 * 1024 * 1024);
    let hard_guard_bytes = arg_u64(arguments, "hard_guard_bytes").unwrap_or(100 * 1024 * 1024);
    if review_bytes == 0 || hard_guard_bytes < review_bytes {
        return Err("Vault audit thresholds are invalid".into());
    }
    let limit = arg_u64(arguments, "limit")
        .unwrap_or(2_000)
        .clamp(1, 20_000) as usize;
    let tracked = run_local_git_command(
        "git",
        &["ls-files".into(), "-z".into()],
        Some(workspace.root()),
    )?
    .split('\0')
    .filter(|value| !value.is_empty())
    .map(str::to_string)
    .collect::<BTreeSet<_>>();

    let mut candidates = Vec::new();
    let mut scanned = 0usize;
    for relative in git_visible_files(workspace)? {
        if scanned >= limit {
            break;
        }
        scanned += 1;
        let Ok(path) = workspace.resolve(&relative) else {
            continue;
        };
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() < review_bytes {
            continue;
        }
        let pointer = git_vault_pointer_path(workspace, &relative)
            .ok()
            .map(|path| {
                path.strip_prefix(workspace.root())
                    .unwrap_or(&path)
                    .display()
                    .to_string()
            });
        candidates.push(json!({
            "path": relative,
            "bytes": metadata.len(),
            "tracked": tracked.contains(&relative),
            "vault_review": true,
            "hard_git_guard": metadata.len() >= hard_guard_bytes,
            "pointer": pointer
        }));
    }

    let snapshot = json!({
        "schema_version": 1,
        "authority": "cortex_git_vault_audit",
        "review_bytes": review_bytes,
        "hard_guard_bytes": hard_guard_bytes,
        "scanned": scanned,
        "truncated": scanned >= limit,
        "candidate_count": candidates.len(),
        "candidates": candidates
    });
    let cache = workspace
        .cortex_state_dir()
        .join("repositories")
        .join("vault-git-audit.json");
    if let Some(parent) = cache.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = cache.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(&snapshot).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::rename(&temporary, &cache).map_err(|error| error.to_string())?;
    Ok(json!({"snapshot":snapshot,"cache":cache}))
}

fn git_vault_ingest(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    let requested = required_string(arguments, "path")?;
    let source = workspace
        .resolve(requested)
        .map_err(|error| error.to_string())?;
    if !source.is_file() {
        return Err(format!(
            "Vault ingest source is not a file: {}",
            source.display()
        ));
    }
    let logical = arguments
        .get("logical_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(requested);
    let logical_resolved = workspace
        .resolve(logical)
        .map_err(|error| error.to_string())?;
    let logical_relative = logical_resolved
        .strip_prefix(workspace.root())
        .map_err(|_| "Vault logical path escaped the project".to_string())?
        .to_string_lossy()
        .replace('\\', "/");

    let sha256 = windows_sha256(&source)?;
    let bytes = fs::metadata(&source)
        .map_err(|error| error.to_string())?
        .len();
    let vault_root = cortex_library_root()?;
    let object = git_vault_object_path(&vault_root, &sha256)?;
    let object_created = if object.is_file() {
        false
    } else {
        if let Some(parent) = object.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let temporary = object.with_extension("partial");
        fs::copy(&source, &temporary).map_err(|error| error.to_string())?;
        let copied_hash = windows_sha256(&temporary)?;
        if copied_hash != sha256 {
            let _ = fs::remove_file(&temporary);
            return Err("Cortex Vault copied object failed SHA-256 verification".into());
        }
        fs::rename(&temporary, &object).map_err(|error| error.to_string())?;
        true
    };

    let pointer = git_vault_pointer_path(workspace, &logical_relative)?;
    if let Some(parent) = pointer.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let manifest = json!({
        "schema_version": 1,
        "authority": "cortex_vault_pointer",
        "logical_path": logical_relative,
        "sha256": sha256,
        "bytes": bytes,
        "vault_object": object,
        "source_path_at_ingest": source
            .strip_prefix(workspace.root())
            .unwrap_or(&source)
            .to_string_lossy()
            .replace('\\', "/"),
        "license": arguments.get("license").and_then(Value::as_str),
        "source_url": arguments.get("source_url").and_then(Value::as_str),
        "ingested_unix_ms": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    });
    fs::write(
        &pointer,
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    Ok(json!({
        "authority":"cortex_vault_cas",
        "object_created":object_created,
        "object":object,
        "pointer":pointer,
        "working_payload_removed":false,
        "manifest":manifest
    }))
}

fn git_vault_materialize(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    let pointer_arg = required_string(arguments, "pointer")?;
    let pointer = workspace
        .resolve(pointer_arg)
        .map_err(|error| error.to_string())?;
    if !pointer.is_file() {
        return Err(format!(
            "Vault pointer is not a file: {}",
            pointer.display()
        ));
    }
    let manifest: Value =
        serde_json::from_slice(&fs::read(&pointer).map_err(|error| error.to_string())?)
            .map_err(|error| format!("invalid Cortex Vault pointer: {error}"))?;
    let logical = manifest
        .get("logical_path")
        .and_then(Value::as_str)
        .ok_or_else(|| "Vault pointer is missing logical_path".to_string())?;
    let sha256 = manifest
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| "Vault pointer is missing sha256".to_string())?;
    let vault_root = cortex_library_root()?;
    let object = git_vault_object_path(&vault_root, sha256)?;
    if !object.is_file() {
        return Err(format!(
            "Cortex Vault object is missing: {}",
            object.display()
        ));
    }
    let target = workspace
        .resolve(logical)
        .map_err(|error| error.to_string())?;
    let overwrite = arguments
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if target.exists() && !overwrite {
        return Err(format!(
            "Vault materialization target already exists: {}. Use overwrite=true only after review.",
            target.display()
        ));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = target.with_extension("cortex-materialize.partial");
    fs::copy(&object, &temporary).map_err(|error| error.to_string())?;
    let materialized_hash = windows_sha256(&temporary)?;
    if !materialized_hash.eq_ignore_ascii_case(sha256) {
        let _ = fs::remove_file(&temporary);
        return Err("Materialized Cortex Vault object failed SHA-256 verification".into());
    }
    if target.exists() {
        fs::remove_file(&target).map_err(|error| error.to_string())?;
    }
    fs::rename(&temporary, &target).map_err(|error| error.to_string())?;
    Ok(json!({
        "authority":"cortex_vault_materialization",
        "pointer":pointer,
        "object":object,
        "target":target,
        "sha256":sha256,
        "verified":true
    }))
}

fn git_release_snapshot(arguments: &Value, workspace: &Workspace) -> Result<Value, String> {
    let version = required_string(arguments, "version")?.trim();
    if version.is_empty()
        || version.chars().any(|ch| {
            ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
    {
        return Err("invalid release snapshot version".into());
    }
    let commit = run_local_git_command(
        "git",
        &["rev-parse".into(), "HEAD".into()],
        Some(workspace.root()),
    )?
    .trim()
    .to_string();
    let branch = git_current_branch(workspace.root()).ok();

    let pointer_root = git_vault_pointer_root(workspace);
    let mut pointers = Vec::new();
    if pointer_root.is_dir() {
        let mut stack = vec![pointer_root.clone()];
        while let Some(directory) = stack.pop() {
            for entry in fs::read_dir(&directory).map_err(|error| error.to_string())? {
                let entry = entry.map_err(|error| error.to_string())?;
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".cortex-vault.json"))
                {
                    let value: Value = serde_json::from_slice(
                        &fs::read(&path).map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| {
                        format!("invalid Vault pointer {}: {error}", path.display())
                    })?;
                    pointers.push(json!({
                        "pointer": path
                            .strip_prefix(workspace.root())
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .replace('\\', "/"),
                        "logical_path": value.get("logical_path"),
                        "sha256": value.get("sha256"),
                        "bytes": value.get("bytes")
                    }));
                }
            }
        }
    }
    pointers.sort_by(|left, right| {
        left.get("pointer")
            .and_then(Value::as_str)
            .unwrap_or("")
            .cmp(right.get("pointer").and_then(Value::as_str).unwrap_or(""))
    });

    let descriptor = json!({
        "schema_version": 1,
        "authority": "cortex_release_state",
        "version": version,
        "project_root": workspace.root(),
        "git": {
            "commit": commit,
            "branch": branch
        },
        "vault": {
            "pointer_count": pointers.len(),
            "pointers": pointers
        },
        "notes": arguments.get("notes").and_then(Value::as_str).unwrap_or(""),
        "created_unix_ms": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    });

    let release_dir = workspace.cortex_state_dir().join("releases");
    fs::create_dir_all(&release_dir).map_err(|error| error.to_string())?;
    let path = release_dir.join(format!("{version}.release-state.json"));
    fs::write(
        &path,
        serde_json::to_vec_pretty(&descriptor).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    Ok(json!({
        "authority":"cortex_release_state",
        "path":path,
        "descriptor":descriptor,
        "published":false
    }))
}

fn compact_json_chars(value: &Value, max_chars: usize) -> String {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "<unavailable>".into());
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut compact = text.chars().take(max_chars).collect::<String>();
    compact.push_str("…<truncated>");
    compact
}

fn spawn_universal_runtime(
    processes: &mut ProcessService,
    workspace_root: &Path,
    label: &str,
    runtime: &cortex_universal::RuntimeArtifact,
    args: &[String],
) -> Result<(u32, Option<PathBuf>), String> {
    let program = &runtime.program;
    let project_candidate = if program.is_absolute() {
        program.clone()
    } else {
        workspace_root.join(program)
    };

    if project_candidate.is_file() {
        let pid = processes.spawn_project_executable(
            workspace_root,
            label,
            None,
            &project_candidate,
            args,
        )?;
        return Ok((pid, Some(project_candidate)));
    }

    let program_text = program.to_string_lossy().to_string();
    if program.components().count() == 1 {
        let pid = processes.spawn_owned(workspace_root, label, &program_text, args)?;
        return Ok((pid, None));
    }

    Err(format!(
        "Cortex resolved runtime artifact `{}` but it does not exist under the active project root",
        project_candidate.display()
    ))
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string argument: {key}"))
}
fn arg_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}
fn arg_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}
fn required_u64(value: &Value, key: &str) -> Result<u64, String> {
    arg_u64(value, key).ok_or_else(|| format!("missing integer argument: {key}"))
}

fn tail_chars(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    text.chars().skip(total - max_chars).collect()
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod schema_tests {
    use super::*;

    fn assert_array_items(tool_name: &str, path: &str, schema: &Value) {
        if schema.get("type").and_then(Value::as_str) == Some("array") {
            assert!(
                schema.get("items").is_some(),
                "tool {tool_name} contains array schema without items at {path}"
            );
        }

        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (name, child) in properties {
                assert_array_items(tool_name, &format!("{path}.properties.{name}"), child);
            }
        }
        if let Some(items) = schema.get("items") {
            assert_array_items(tool_name, &format!("{path}.items"), items);
        }
        for keyword in ["oneOf", "anyOf", "allOf"] {
            if let Some(children) = schema.get(keyword).and_then(Value::as_array) {
                for (index, child) in children.iter().enumerate() {
                    assert_array_items(tool_name, &format!("{path}.{keyword}[{index}]"), child);
                }
            }
        }
    }

    #[test]
    fn double_escaped_whole_file_payload_is_decoded_once() {
        let input = r#"[package]\nname = \"hello3d\"\nversion = \"0.1.0\"\n"#;
        let (decoded, normalized) = normalize_multiline_text_argument(input);
        assert!(normalized);
        assert_eq!(
            decoded,
            "[package]\nname = \"hello3d\"\nversion = \"0.1.0\"\n"
        );
    }

    #[test]
    fn legitimate_literal_escape_source_is_not_rewritten() {
        let input = r#"fn main() { println!("a\nb"); }"#;
        let (decoded, normalized) = normalize_multiline_text_argument(input);
        assert!(!normalized);
        assert_eq!(decoded, input);
    }

    #[test]
    fn candidate_decision_keeps_improvement_and_verified_results() {
        let mut baseline = QualitySnapshot {
            errors: 4,
            ..Default::default()
        };
        baseline.fingerprints.extend([
            "error|E1|src/main.rs|1|one".into(),
            "error|E2|src/main.rs|2|two".into(),
            "error|E3|src/main.rs|3|three".into(),
            "error|E4|src/main.rs|4|four".into(),
        ]);
        let mut improved = QualitySnapshot {
            errors: 2,
            ..Default::default()
        };
        improved.fingerprints.extend([
            "error|E1|src/main.rs|1|one".into(),
            "error|E2|src/main.rs|2|two".into(),
        ]);
        assert_eq!(
            candidate_decision(Some(&baseline), &improved, false),
            CandidateDecision::KeepImproved
        );
        assert_eq!(
            candidate_decision(Some(&baseline), &QualitySnapshot::default(), true),
            CandidateDecision::KeepVerified
        );
    }

    #[test]
    fn candidate_decision_rolls_back_unchanged_and_regressing_results() {
        let mut baseline = QualitySnapshot {
            errors: 1,
            ..Default::default()
        };
        baseline
            .fingerprints
            .insert("error|E1|src/main.rs|1|old".into());
        let unchanged = baseline.clone();
        assert_eq!(
            candidate_decision(Some(&baseline), &unchanged, false),
            CandidateDecision::RollbackUnchanged
        );

        let mut regressed = QualitySnapshot {
            errors: 13,
            ..Default::default()
        };
        for index in 0..13 {
            regressed
                .fingerprints
                .insert(format!("error|E{index}|src/main.rs|{}|new", index + 1));
        }
        assert_eq!(
            candidate_decision(Some(&baseline), &regressed, false),
            CandidateDecision::RollbackRegressed
        );
        assert!(CandidateDecision::RollbackRegressed.should_rollback());
    }

    #[test]
    fn candidate_without_validation_baseline_remains_reversible_without_false_comparison() {
        let current = QualitySnapshot {
            errors: 3,
            ..Default::default()
        };
        assert_eq!(
            candidate_decision(None, &current, false),
            CandidateDecision::ObserveWithoutBaseline
        );
        assert!(!CandidateDecision::ObserveWithoutBaseline.should_rollback());
    }

    #[test]
    fn quality_delta_rejects_catastrophic_regression() {
        let mut previous = QualitySnapshot {
            errors: 1,
            ..Default::default()
        };
        previous
            .fingerprints
            .insert("error|E1|src/main.rs|1|old".into());
        let mut current = QualitySnapshot {
            errors: 13,
            ..Default::default()
        };
        for index in 0..13 {
            current
                .fingerprints
                .insert(format!("error|E{index}|src/main.rs|{}|new", index + 1));
        }
        let delta = quality_delta(Some(&previous), &current);
        assert_eq!(delta.get("regressed").and_then(Value::as_bool), Some(true));
        assert_eq!(
            delta
                .get("catastrophic_regression")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn dependency_ensure_inserts_one_explicit_cargo_requirement() {
        let manifest = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n\n[dependencies]\nwgpu = \"0.19.4\"\n";
        let (updated, changed) =
            ensure_cargo_dependency_text(manifest, "winit", "0.29.15").unwrap();
        assert!(changed);
        assert!(updated.contains("winit = \"0.29.15\""));
        let (again, changed_again) =
            ensure_cargo_dependency_text(&updated, "winit", "0.29.15").unwrap();
        assert!(!changed_again);
        assert_eq!(again.matches("winit =").count(), 1);
    }

    #[test]
    fn cargo_lock_grounding_finds_exact_versions() {
        let root = std::env::temp_dir().join(format!(
            "cortex-tools-grounding-{}-{}",
            std::process::id(),
            unix_ms()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Cargo.toml"),"[package]\nname=\"fixture\"\nversion=\"0.1.0\"\n[dependencies]\nwgpu=\"0.19\"\nwinit={version=\"0.29\"}\n").unwrap();
        fs::write(root.join("Cargo.lock"),"[[package]]\nname = \"wgpu\"\nversion = \"0.19.4\"\n\n[[package]]\nname = \"winit\"\nversion = \"0.29.15\"\n").unwrap();
        let text = cargo_dependency_grounding(&root, None).unwrap().to_string();
        assert!(text.contains("0.19.4") && text.contains("0.29.15"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dependency_references_detect_declared_rust_packages() {
        let root = std::env::temp_dir().join(format!(
            "cortex-tools-ground-before-write-{}-{}",
            std::process::id(),
            unix_ms()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\n[dependencies]\nwgpu=\"0.19\"\nfutures-lite=\"2\"\n",
        )
        .unwrap();
        let references = dependency_references_in_text(
            &root,
            "use wgpu::Backends;\nlet _ = futures_lite::future::block_on(async {});",
        );
        assert!(references.contains("wgpu"));
        assert!(references.contains("futures-lite"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dependency_references_ignore_unrelated_local_code() {
        let root = std::env::temp_dir().join(format!(
            "cortex-tools-ground-before-write-local-{}-{}",
            std::process::id(),
            unix_ms()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\n[dependencies]\nwgpu=\"0.19\"\n",
        )
        .unwrap();
        let references = dependency_references_in_text(
            &root,
            "fn update_clock(title: &mut String) { title.push_str(\"tick\"); }",
        );
        assert!(references.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_read_schema_exposes_bounded_continuation_offset() {
        let definition = definitions()
            .into_iter()
            .find(|tool| tool.name == "source.read")
            .expect("source.read must exist");
        assert!(definition
            .parameters
            .pointer("/properties/offset")
            .is_some());
        assert_eq!(
            definition
                .parameters
                .pointer("/properties/offset/minimum")
                .and_then(Value::as_u64),
            Some(0)
        );
    }

    #[test]
    fn repeated_failures_have_stable_fingerprints_and_force_strategy_change() {
        let arguments = json!({"path": "src/main.rs", "max_bytes": 65536});
        let error = "unsafe workspace path: \\\\?\\C:\\Project\\src\\main.rs";
        let first = tool_failure_fingerprint("source.read", &arguments, error);
        let second = tool_failure_fingerprint("source.read", &arguments, error);
        assert_eq!(first, second);
        assert_eq!(
            classify_tool_failure("source.read", error),
            "path_authority"
        );
        let hint = tool_recovery_hint("source.read", "path_authority", 2);
        assert!(hint.contains("repeated 2 times"));
        assert!(hint.contains("change recovery strategy"));
        assert!(hint.contains("workspace-relative path"));
    }

    #[test]
    fn development_run_advance_is_non_failing_when_no_roadmap_run_owns_request() {
        let source = include_str!("lib.rs");
        assert!(source.contains("\"reason\": \"no_active_development_run\""));
        assert!(source.contains("project-quality repair may continue"));
    }

    #[test]
    fn begin_transaction_contract_is_idempotent_for_active_candidate_reuse() {
        let definition = definitions()
            .into_iter()
            .find(|tool| tool.name == "source.begin_transaction")
            .expect("source.begin_transaction definition");
        assert!(definition.description.contains("reversible candidate"));
        let source = include_str!("lib.rs");
        assert!(source
            .contains("A Cortex source transaction is already active. Reuse this transaction"));
        assert!(source.contains("\"reused\": true"));
    }

    #[test]
    fn ground_before_write_failure_routes_to_dependency_recovery() {
        let error = "Ground Before Write blocked this API-sensitive mutation. Exact dependency-source evidence is missing for: wgpu.";
        assert_eq!(
            classify_tool_failure("source.write_text", error),
            "dependency_grounding"
        );
        let hint = tool_recovery_hint("source.write_text", "dependency_grounding", 1);
        assert!(hint.contains("source.search"));
        assert!(hint.contains("dependency_package"));
    }

    #[test]
    fn dependency_grounding_error_exposes_every_missing_package() {
        let error = "Ground Before Write blocked this API-sensitive mutation. Exact dependency-source evidence is missing for: chrono, futures-lite, tokio, wgpu, winit. Use source.search with dependency_package for each package.";
        assert_eq!(
            dependency_grounding_packages_from_error(error),
            vec![
                "chrono".to_string(),
                "futures-lite".to_string(),
                "tokio".to_string(),
                "wgpu".to_string(),
                "winit".to_string(),
            ]
        );
    }

    #[test]
    fn grounded_regeneration_is_not_treated_as_missing_grounding() {
        let error = "M11 grounded regeneration required. Exact local dependency evidence is now grounded for [wgpu]. The blocked API-sensitive mutation was NOT replayed.";
        assert_eq!(
            classify_tool_failure("source.write_text", error),
            "grounded_regeneration"
        );
        let hint = tool_recovery_hint("source.write_text", "grounded_regeneration", 1);
        assert!(hint.contains("Regenerate"));
        assert!(hint.contains("do not repeat"));
    }

    #[test]
    fn stale_pre_grounding_mutations_are_never_replayed_automatically() {
        let source = include_str!("lib.rs");
        assert!(source.contains("blocked API-sensitive mutation was NOT replayed"));

        // Build the forbidden replay signature dynamically so this regression test
        // does not match its own source text.
        let forbidden_replay = ["match self.", "execute_inner_once(call)"].concat();
        assert!(!source.contains(&forbidden_replay));
    }

    #[test]
    fn read_budget_failure_recommends_window_continuation() {
        let hint = tool_recovery_hint("source.read", "read_budget", 1);
        assert!(hint.contains("next_offset"));
        assert!(hint.contains("source.search"));
    }

    #[test]
    fn tail_chars_preserves_recent_diagnostics() {
        assert_eq!(tail_chars("abcdef", 4), "cdef");
        assert_eq!(tail_chars("abc", 4), "abc");
        assert_eq!(tail_chars("aβγδ", 2), "γδ");
    }

    #[test]
    fn local_forgejo_host_is_loopback_private_and_provider_neutral() {
        let compose = cortex_forgejo_compose();
        assert!(compose.contains("codeberg.org/forgejo/forgejo:15.0.7"));
        assert!(compose.contains("127.0.0.1:3000:3000"));
        assert!(compose.contains("127.0.0.1:2222:22"));
        assert!(compose.contains("ENABLE_PUSH_CREATE_USER=true"));
        assert!(compose.contains("ENABLE_PUSH_CREATE_ORG=true"));
        assert!(compose.contains("DEFAULT_PUSH_CREATE_PRIVATE=true"));
        assert!(compose.contains("FORCE_PRIVATE=true"));
        assert!(compose.contains("FORGEJO__actions__ENABLED=false"));

        let names = definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<BTreeSet<_>>();
        assert!(names.contains("git.host.status"));
        assert!(names.contains("git.host.bootstrap"));
        assert!(names.contains("git.host.ensure_ssh_key"));
        assert!(names.contains("git.project.attach_local"));
    }

    #[test]
    fn repository_overview_and_local_forgejo_contract_are_registered() {
        let names = definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<BTreeSet<_>>();
        assert!(names.contains("git.repository.overview"));
        assert!(names.contains("git.host.status"));
        assert!(names.contains("git.host.bootstrap"));
        assert!(names.contains("git.host.ensure_ssh_key"));
        assert!(names.contains("git.project.attach_local"));
        assert!(cortex_forgejo_compose().contains("codeberg.org/forgejo/forgejo:15.0.7"));
        assert!(cortex_forgejo_compose().contains("127.0.0.1:3000:3000"));
        assert!(cortex_forgejo_compose().contains("127.0.0.1:2222:22"));
        assert!(cortex_forgejo_compose().contains("FORGEJO__repository__FORCE_PRIVATE=true"));
    }

    #[test]
    fn forgejo_provider_tools_and_secret_contract_are_registered() {
        let names = definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<BTreeSet<_>>();
        assert!(names.contains("git.host.ensure_identity"));
        assert!(names.contains("git.provider.refresh"));
        assert!(names.contains("git.issue.create"));
        assert_eq!(
            local_git_slug("Open2D Foundry").expect("slug"),
            "open2d-foundry"
        );
        let decoded = forgejo_decode_chunked(b"4\r\ntest\r\n0\r\n\r\n").expect("chunked body");
        assert_eq!(decoded, b"test");
    }

    #[test]
    fn git_operations_and_safety_refs_are_registered() {
        let names = definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<BTreeSet<_>>();
        for expected in [
            "git.changes.snapshot",
            "git.stage",
            "git.unstage",
            "git.commit",
            "git.branch.create",
            "git.branch.switch",
            "git.safety.create",
            "git.safety.list",
            "git.fetch.local",
            "git.sync.status",
            "git.push.local",
            "git.host.backup",
        ] {
            assert!(names.contains(expected), "missing {expected}");
        }
        assert!(git_branch_name("feature/repositories").is_ok());
        assert!(git_branch_name("bad branch").is_err());
        assert!(git_branch_name("../bad").is_err());
    }

    #[test]
    fn git_vault_bridge_tools_are_registered() {
        let names = definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<BTreeSet<_>>();
        for expected in [
            "git.vault.audit",
            "git.vault.ingest",
            "git.vault.materialize",
            "git.release.snapshot",
        ] {
            assert!(names.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn every_builtin_array_schema_declares_items() {
        for definition in definitions() {
            assert_array_items(&definition.name, "$", &definition.parameters);
        }
    }

    #[test]
    fn cargo_workspace_summary_reports_build_authority() {
        let summary = cargo_workspace_summary(
            r#"
[workspace]
members = [
  "apps/open2d_foundry",
  "apps/cortex_desktop",
  "crates/cortex_core",
  "crates/open2d_cortex_core",
  "crates/open2d_havenwild_reference"
]
exclude = [
  "crates/open2d_ai"
]
"#,
        );
        assert_eq!(summary.get("member_count").and_then(Value::as_u64), Some(5));
        assert_eq!(
            summary
                .get("generic_cortex_authorities")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            summary
                .get("open2d_cortex_compatibility")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            summary
                .get("reference_members")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            summary
                .get("excluded")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn source_manifest_summary_uses_manifest_inventory_counts() {
        let summary = source_manifest_summary(&json!({
            "schema_version": 2,
            "pass": "TEST",
            "files": [
                {"path":"a.rs"},
                {"path":"b.rs"},
                {"path":"image.png"},
                {"path":"Cargo.toml"},
                {"path":"README.md"}
            ]
        }));
        assert_eq!(summary.get("file_count").and_then(Value::as_u64), Some(5));
        assert_eq!(
            summary.pointer("/counts/rust").and_then(Value::as_u64),
            Some(2)
        );
        assert_eq!(
            summary.pointer("/counts/png").and_then(Value::as_u64),
            Some(1)
        );
    }

    #[test]
    fn vscode_workspace_edits_publish_structured_item_schema() {
        let definition = definitions()
            .into_iter()
            .find(|tool| tool.name == "vscode.apply_workspace_edit")
            .expect("VS Code workspace edit tool must exist");
        let items = definition
            .parameters
            .pointer("/properties/edits/items")
            .expect("edits array must define items");
        assert_eq!(items.get("type").and_then(Value::as_str), Some("object"));
        assert!(items.pointer("/properties/path").is_some());
        assert!(items.pointer("/properties/new_text").is_some());
    }
}

// M11_UNIVERSAL_FOUNDATION_BRIDGE
// Additive M11A-T bridge. M10 remains the execution/lifecycle authority.
pub mod universal_m11 {
    pub use cortex_universal::*;

    #[cfg(test)]
    mod m11u2_tests {
        #[test]
        fn grounding_is_not_revoked_by_repeated_api_mismatch_validation() {
            let source = include_str!("lib.rs");
            assert!(
                source.contains("compiler diagnostics do not invalidate exact-version evidence")
            );
            assert!(source.contains("changing the manifest invalidates evidence, not the identity"));
        }

        #[test]
        fn source_mutation_runs_controller_verification_immediately() {
            let source = include_str!("lib.rs");
            assert!(source.contains("m11u2_controller_post_mutation_verifier"));
            assert!(source.contains("The controller already formatted/validated this mutation"));
        }

        #[test]
        fn runtime_acceptance_supports_window_title_change() {
            let source = include_str!("lib.rs");
            assert!(source.contains("require_title_change"));
            assert!(source.contains("title_sample_interval_ms"));
            assert!(source.contains("title_changed"));
        }
    }
}
