use forge_state::{ForgeStateStore, TakeoverStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub const CERTIFICATION_SCHEMA: &str = "forge.takeover.certification.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerSpec {
    pub id: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticComparison {
    pub key: String,
    pub left_runner: String,
    pub right_runner: String,
    pub equivalent: bool,
    pub left_exit: i32,
    pub right_exit: i32,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificationReport {
    pub schema: String,
    pub unix_ms: u64,
    pub comparisons: Vec<SemanticComparison>,
    pub required_checks_verified: bool,
    pub takeover_ready: bool,
}

pub fn compare_json_values(left: &Value, right: &Value, ignored_keys: &BTreeSet<String>) -> bool {
    normalize_json(left, ignored_keys) == normalize_json(right, ignored_keys)
}

pub fn compare_runners(
    key: &str,
    left: &RunnerSpec,
    right: &RunnerSpec,
    ignored_keys: &BTreeSet<String>,
) -> Result<SemanticComparison, String> {
    let left_result = run_runner(left)?;
    let right_result = run_runner(right)?;
    let left_json = serde_json::from_str::<Value>(left_result.stdout.trim()).ok();
    let right_json = serde_json::from_str::<Value>(right_result.stdout.trim()).ok();
    let equivalent = left_result.exit == right_result.exit
        && match (&left_json, &right_json) {
            (Some(left), Some(right)) => compare_json_values(left, right, ignored_keys),
            _ => left_result.stdout.trim() == right_result.stdout.trim(),
        };
    Ok(SemanticComparison {
        key: key.to_owned(),
        left_runner: left.id.clone(),
        right_runner: right.id.clone(),
        equivalent,
        left_exit: left_result.exit,
        right_exit: right_result.exit,
        detail: if equivalent { "semantic outputs match".to_owned() } else { "semantic outputs differ".to_owned() },
    })
}

pub fn finalize_report(
    store: &mut ForgeStateStore,
    comparisons: Vec<SemanticComparison>,
) -> Result<CertificationReport, String> {
    let all_comparisons = !comparisons.is_empty() && comparisons.iter().all(|item| item.equivalent);
    store.set_takeover_check(
        "takeover_harness",
        if all_comparisons { TakeoverStatus::Verified } else { TakeoverStatus::Blocked },
        if all_comparisons { "ForgePY and Rust Forge semantic certification comparisons passed" } else { "takeover parity comparisons are incomplete or divergent" },
    )?;
    let required_checks_verified = store.snapshot().takeover.checks.iter().all(|check| {
        matches!(check.status, TakeoverStatus::Verified | TakeoverStatus::NotApplicable)
    });
    Ok(CertificationReport {
        schema: CERTIFICATION_SCHEMA.to_owned(),
        unix_ms: unix_ms(),
        comparisons,
        required_checks_verified,
        takeover_ready: store.snapshot().takeover.ready_for_takeover(),
    })
}

pub fn evidence_summary(store: &ForgeStateStore) -> BTreeMap<String, String> {
    store.snapshot().takeover.checks.iter().map(|check| {
        (check.id.clone(), format!("{:?}: {}", check.status, check.evidence))
    }).collect()
}

struct RunnerResult { exit: i32, stdout: String }

fn run_runner(spec: &RunnerSpec) -> Result<RunnerResult, String> {
    let output = Command::new(&spec.program)
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .output()
        .map_err(|error| format!("failed to run certification runner {}: {error}", spec.id))?;
    Ok(RunnerResult {
        exit: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
    })
}

fn normalize_json(value: &Value, ignored: &BTreeSet<String>) -> Value {
    match value {
        Value::Object(map) => {
            let normalized = map.iter().filter(|(key, _)| !ignored.contains(key.as_str())).map(|(key, value)| {
                (key.clone(), normalize_json(value, ignored))
            }).collect();
            Value::Object(normalized)
        }
        Value::Array(values) => Value::Array(values.iter().map(|value| normalize_json(value, ignored)).collect()),
        _ => value.clone(),
    }
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn semantic_json_can_ignore_timestamps() {
        let left = serde_json::json!({"status":"ok","unix_ms":1,"nested":{"value":2}});
        let right = serde_json::json!({"status":"ok","unix_ms":9,"nested":{"value":2}});
        let ignored = ["unix_ms".to_owned()].into_iter().collect();
        assert!(compare_json_values(&left, &right, &ignored));
    }
}
