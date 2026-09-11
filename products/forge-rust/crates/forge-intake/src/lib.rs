use forge_artifacts::{ArtifactCentral, ArtifactClass, ArtifactEvidence};
use forge_state::{ForgeStateStore, PatchDisposition, PatchEvidence, PatchLineageRecord};
use forge_update::PatchPackage;
use forge_vcs;
use forge_watch::{capture, diff, load_snapshot, save_snapshot, WatchChange, WatchConfig};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntakeItem {
    pub path: PathBuf,
    pub project_id: String,
    pub patch_id: String,
    pub valid: bool,
    pub error: String,
    pub lineage: Option<PatchLineageRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntakeReport {
    pub scanned_roots: Vec<PathBuf>,
    pub items: Vec<IntakeItem>,
    pub pending: usize,
    pub classified: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntakeWatchReport {
    pub changes: Vec<WatchChange>,
    pub intake: IntakeReport,
}

pub struct IntakeScanner;

impl IntakeScanner {
    pub fn scan_once(
        store: &mut ForgeStateStore,
        active_project_root: &Path,
    ) -> Result<IntakeReport, String> {
        let roots = store.snapshot().settings.intake_roots.clone();
        let current_commit = forge_vcs::inspect(active_project_root).ok().map(|state| state.head);
        let existing = store.snapshot().patch_lineage.clone();
        let active_id = store.snapshot().active_project_id.clone().unwrap_or_else(|| "unknown".to_owned());
        let mut items = Vec::new();

        for root in &roots {
            if !root.is_dir() { continue; }
            for entry in fs::read_dir(root).map_err(|error| error.to_string())?.flatten() {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("patch") || !path.is_file() { continue; }
                match PatchPackage::open(&path) {
                    Ok(package) => {
                        let patch_id = package.manifest().patch_id.clone();
                        let project_id = package.manifest().target.project_id.clone();
                        let applied = existing.iter().any(|record| record.patch_id == patch_id && record.project_id == project_id && record.disposition == PatchDisposition::Applied);
                        let superseded = existing.iter().any(|record| record.patch_id == patch_id && record.project_id == project_id && record.disposition == PatchDisposition::Superseded);
                        let created = file_mtime_ms(&path);
                        let lineage = store.classify_patch(PatchEvidence {
                            patch_id: patch_id.clone(), project_id: project_id.clone(), source_path: path.clone(),
                            created_unix_ms: created, base_commit: package.validation().base_commit.clone(),
                            applied, superseded, valid_manifest: true,
                        }, current_commit.as_deref())?;
                        items.push(IntakeItem { path, project_id, patch_id, valid: true, error: String::new(), lineage: Some(lineage) });
                    }
                    Err(error) => {
                        let patch_id = path.file_stem().and_then(|value| value.to_str()).unwrap_or("invalid-patch").to_owned();
                        let lineage = store.classify_patch(PatchEvidence {
                            patch_id: patch_id.clone(), project_id: active_id.clone(), source_path: path.clone(),
                            created_unix_ms: file_mtime_ms(&path), base_commit: None, applied: false, superseded: false, valid_manifest: false,
                        }, current_commit.as_deref()).ok();
                        items.push(IntakeItem { path, project_id: active_id.clone(), patch_id, valid: false, error, lineage });
                    }
                }
            }
        }
        let pending = items.iter().filter(|item| item.lineage.as_ref().is_some_and(|lineage| lineage.disposition == PatchDisposition::Pending)).count();
        Ok(IntakeReport { scanned_roots: roots, classified: items.len(), items, pending })
    }

    pub fn scan_changed(
        store: &mut ForgeStateStore,
        active_project_root: &Path,
    ) -> Result<IntakeWatchReport, String> {
        let roots = store.snapshot().settings.intake_roots.clone();
        let config = WatchConfig {
            roots,
            recursive: false,
            max_entries: 25_000,
            ..WatchConfig::default()
        };
        let current = capture(&config)?;
        let snapshot_path = store.root().join("intake-watch.json");
        let changes = load_snapshot(&snapshot_path)?
            .map(|previous| diff(&previous, &current).changes)
            .unwrap_or_default();
        save_snapshot(&snapshot_path, &current)?;
        let relevant_change = changes.iter().any(|change| {
            change
                .path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("patch"))
        });
        let intake = if relevant_change {
            Self::scan_once(store, active_project_root)?
        } else {
            IntakeReport {
                scanned_roots: config.roots,
                items: Vec::new(),
                pending: 0,
                classified: 0,
            }
        };
        Ok(IntakeWatchReport { changes, intake })
    }

    pub fn approve_and_promote(
        store: &mut ForgeStateStore,
        patch_id: &str,
    ) -> Result<ArtifactEvidence, String> {
        let record = store.snapshot().patch_lineage.iter().find(|record| record.patch_id == patch_id).cloned()
            .ok_or_else(|| format!("patch lineage record not found: {patch_id}"))?;
        if record.disposition != PatchDisposition::Pending {
            return Err(format!("patch is not pending approval: {patch_id} ({:?})", record.disposition));
        }
        let root = store.snapshot().settings.artifact_central_root.clone()
            .ok_or_else(|| "Artifact Central is not configured".to_owned())?;
        let central = ArtifactCentral::open(root)?;
        central.promote(&record.project_id, ArtifactClass::PatchPending, &record.source_path, None)
    }
}

fn file_mtime_ms(path: &Path) -> Option<u64> {
    fs::metadata(path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|value| value.as_millis().min(u128::from(u64::MAX)) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_mtime_is_nonfatal() {
        assert!(file_mtime_ms(Path::new("definitely-not-a-real-file.patch")).is_none());
    }
}
