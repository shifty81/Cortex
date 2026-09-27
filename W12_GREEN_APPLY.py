#!/usr/bin/env python3
"""W12 source-only updater from certified Cortex W11 GREEN (GitHub main 71c63c7).

No project source moves, destructive operations, commits or network access.
Content-hash preflight runs for *all* source targets before the first write.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile
from datetime import datetime, timezone

GREEN_SOURCE_BLOBS = {
    "crates/cortex_cli/src/lib.rs": "0087a5d625fe546765ed8f2a4fcdb83744c905c0",
    "crates/cortex_core/src/lib.rs": "44a26025cadf9a69aa435796f47778185c386e02",
}
EXPECTED_GREEN_COMMIT = "71c63c7c8a5ae037b3c8294acb4b751580ea10fa"


def git_blob_hash(data: bytes) -> str:
    return hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()


def unique(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise ValueError(f"W12 anchor {label}: expected once, observed {count}")
    return text.replace(old, new, 1)


def unique_regex(text: str, pattern: str, repl, label: str) -> str:
    result, count = re.subn(pattern, repl, text, count=0, flags=re.MULTILINE | re.DOTALL)
    if count != 1:
        raise ValueError(f"W12 regex anchor {label}: expected once, observed {count}")
    return result


CORE_TEST_MODULE = '\n#[cfg(test)]\nmod w12_evidence_tests {\n    use super::*;\n\n    #[test]\n    fn receipt_exposes_only_bounded_path_and_fixed_error_class() {\n        let call = ToolCall {\n            call_id: "redacted-call".into(),\n            name: "source.replace_text".into(),\n            arguments: json!({"path":"Cargo.toml","new":"secret-value-in-source"}),\n        };\n        let result = ToolResultInput {\n            call_id: call.call_id.clone(),\n            output: json!({"error":"expected 1 occurrence of secret-value-in-source; found 0"}),\n            is_error: true,\n        };\n        let evidence = AgentToolEvidence::from_result(&call, true, &result);\n        assert_eq!(evidence.target_path.as_deref(), Some("Cargo.toml"));\n        assert_eq!(evidence.failure_class.as_deref(), Some("exact_replace_mismatch"));\n        assert!(!serde_json::to_string(&evidence).unwrap().contains("secret-value"));\n        assert!(safe_evidence_relative_path("C:\\\\private\\\\secret.txt").is_none());\n        assert!(safe_evidence_relative_path("../token.txt").is_none());\n    }\n\n    #[test]\n    fn successful_mutation_receipt_records_controller_rollback() {\n        let call = ToolCall {\n            call_id: "candidate-edit".into(),\n            name: "source.write_text".into(),\n            arguments: json!({"path":"Cargo.toml"}),\n        };\n        let result = ToolResultInput {\n            call_id: call.call_id.clone(),\n            output: json!({"controller_verification":{"candidate":{"decision":"rollback_unchanged"}}}),\n            is_error: false,\n        };\n        let evidence = AgentToolEvidence::from_result(&call, true, &result);\n        assert_eq!(evidence.candidate_decision.as_deref(), Some("rollback_unchanged"));\n    }\n}\n'

def patch_core(text: str) -> str:
    old = '''#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentToolEvidence {
    pub tool: String,
    pub call_id: String,
    pub mutating: bool,
    pub is_error: bool,
}'''
    new = '''#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AgentToolEvidence {
    pub tool: String,
    pub call_id: String,
    pub mutating: bool,
    pub is_error: bool,
    /// Project-relative only; no source content, absolute paths, or command arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_path: Option<String>,
    /// Controller-owned fixed vocabulary; never serialize unrestricted error text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<String>,
    /// Records automatic candidate disposition even when the mutation itself succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_decision: Option<String>,
}

impl AgentToolEvidence {
    fn from_result(call: &ToolCall, mutating: bool, result: &ToolResultInput) -> Self {
        let target_path = if call.name.starts_with("source.") {
            call.arguments
                .get("path")
                .and_then(Value::as_str)
                .and_then(safe_evidence_relative_path)
        } else {
            None
        };
        let failure_class = result.is_error.then(|| {
            let error = result
                .output
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            let class = if error.contains("occurrence") || error.contains("exact replacement") {
                "exact_replace_mismatch"
            } else if error.contains("ground") || error.contains("dependency") {
                "dependency_grounding_required"
            } else if error.contains("transaction") || error.contains("candidate") {
                "transaction_not_active"
            } else if error.contains("path") || error.contains("outside") {
                "path_rejected"
            } else if error.contains("timeout") || error.contains("timed out") {
                "timeout"
            } else if error.contains("toml") || error.contains("manifest") {
                "manifest_rejected"
            } else {
                "tool_rejected"
            };
            class.to_string()
        });
        let candidate_decision = result
            .output
            .pointer("/controller_verification/candidate/decision")
            .and_then(Value::as_str)
            .or_else(|| result.output.pointer("/candidate/decision").and_then(Value::as_str))
            .filter(|decision| matches!(*decision,
                "keep_verified" | "keep_improved" | "rollback_unchanged" |
                "rollback_regressed" | "observe_without_baseline" | "no_active_candidate"))
            .map(str::to_string);
        Self {
            tool: call.name.clone(),
            call_id: call.call_id.clone(),
            mutating,
            is_error: result.is_error,
            target_path,
            failure_class,
            candidate_decision,
        }
    }
}

fn safe_evidence_relative_path(candidate: &str) -> Option<String> {
    if candidate.is_empty() || candidate.len() > 240 || candidate.contains(':') ||
        candidate.starts_with('/') || candidate.starts_with('\\\\') ||
        candidate.chars().any(|ch| ch.is_control()) {
        return None;
    }
    let normalized = candidate.replace('\\\\', "/");
    if normalized.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        return None;
    }
    Some(normalized)
}'''
    text = unique(text, old, new, 'evidence struct')
    text = unique(text, '''collected_evidence.push(AgentToolEvidence {
                    tool: call.name.clone(),
                    call_id: call.call_id.clone(),
                    mutating,
                    is_error: result.is_error,
                });''', '''collected_evidence.push(AgentToolEvidence::from_result(call, mutating, &result));''', 'main evidence')
    text = unique(text, '''collected_evidence.push(AgentToolEvidence {
                        tool: grounding_call.name.clone(),
                        call_id: grounding_call.call_id.clone(),
                        mutating: false,
                        is_error: grounding_result.is_error,
                    });''', '''collected_evidence.push(AgentToolEvidence::from_result(
                        &grounding_call,
                        false,
                        &grounding_result,
                    ));''', 'grounding evidence')
    text = unique(text, '''collected_evidence.push(AgentToolEvidence {
            tool: call.name.clone(),
            call_id: call.call_id.clone(),
            mutating: false,
            is_error: result.is_error,
        });''', '''collected_evidence.push(AgentToolEvidence::from_result(&call, false, &result));''', 'preflight evidence')
    text += CORE_TEST_MODULE
    return text


def patch_cli(text: str) -> str:
    # Pure helper stays separate from the live controller, making fixture tests possible.
    anchor = 'fn project_agent_command(\n'
    helper = '''/// Fail closed *before* the expensive project-native FULL gate when a candidate
/// was already rejected by post-mutation validation or the model left a tool call
/// as unexecuted prose. Never mistake a successful tool invocation for a retained edit.
fn repair_candidate_gate_decision(
    transaction_id: Option<&str>,
    status: &cortex_protocol::ToolResultInput,
    files: &cortex_protocol::ToolResultInput,
    agent: &AgentTurnResult,
) -> Result<(), &'static str> {
    if !has_successful_project_file_mutation(agent) {
        return Err("no_successful_project_file_mutation");
    }
    if status.is_error || files.is_error {
        return Err("transaction_precheck_failed");
    }
    let active_id = status.output.pointer("/transaction/id").and_then(Value::as_str);
    if transaction_id.is_none() || active_id != transaction_id {
        return Err("candidate_not_active_after_model");
    }
    if files.output.get("transaction_id").and_then(Value::as_str) != transaction_id {
        return Err("candidate_file_ledger_mismatch");
    }
    let touched = files.output.get("touched").and_then(Value::as_array).map_or(0, Vec::len);
    let created = files.output.get("created").and_then(Value::as_array).map_or(0, Vec::len);
    if touched + created == 0 {
        return Err("candidate_has_no_recorded_files");
    }
    if agent.text.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("<tool_call>") || line.starts_with("<function_call>")
    }) {
        return Err("unexecuted_tool_call_in_model_report");
    }
    Ok(())
}

'''
    text = unique(text, anchor, helper + anchor, 'precheck helper')
    # W11 returned early on zero-mutating evidence before persisting a proper receipt.
    text = unique(text,
        'if mutating && !has_successful_project_file_mutation(&result) {',
        'if mutating && action != "repair" && !has_successful_project_file_mutation(&result) {',
        'non-repair missing mutations')
    start = '''    let validation_tool = mutating.then(|| {
        let root = service.engine.tools().workspace_root();
        project_validation_tool(root)
    });
    let verification = if let Some(tool) = validation_tool {
        if output == OutputMode::Human {
            eprintln!(
                "[INFO] Controller verification: {tool} (project-owned checkpoint takes precedence)"
            );
        }
        Some(execute_named_tool(service, tool, json!({}), "post-agent-check"))
    } else {
        None
    };
'''
    replacement = '''    // The mutation tool can validate and roll back a candidate *inside* the model
    // turn. Check the actual durable ledger before invoking a project-owned FULL.
    let candidate_precheck_status = if action == "repair" {
        Some(execute_named_tool(service, "source.transaction_status", json!({}), "pre-gate-tx-status"))
    } else {
        None
    };
    let candidate_precheck_files = if action == "repair" {
        Some(execute_named_tool(service, "source.transaction_files", json!({}), "pre-gate-tx-files"))
    } else {
        None
    };
    let checkpoint_skip_reason = if action == "repair" {
        match (candidate_precheck_status.as_ref(), candidate_precheck_files.as_ref()) {
            (Some(status), Some(files)) => repair_candidate_gate_decision(
                tx_id.as_deref(), status, files, &result,
            ).err(),
            _ => Some("transaction_precheck_missing"),
        }
    } else {
        None
    };
    let validation_tool = mutating.then(|| {
        let root = service.engine.tools().workspace_root();
        project_validation_tool(root)
    });
    let verification = if checkpoint_skip_reason.is_some() {
        if output == OutputMode::Human {
            eprintln!("[BLOCK] Skipping project checkpoint; candidate precheck: {}",
                checkpoint_skip_reason.unwrap_or("unknown"));
        }
        None
    } else if let Some(tool) = validation_tool {
        if output == OutputMode::Human {
            eprintln!(
                "[INFO] Controller verification: {tool} (project-owned checkpoint takes precedence)"
            );
        }
        Some(execute_named_tool(service, tool, json!({}), "post-agent-check"))
    } else {
        None
    };
'''
    # match either W11 original or its user-rustfmt equivalent using scope markers
    pattern = (r'    let validation_tool = mutating\.then\(\|\| \{\s*let root = service\.engine\.tools\(\)\.workspace_root\(\);\s*'
               r'project_validation_tool\(root\)\s*\}\);\s*'
               r'    let verification = if let Some\(tool\) = validation_tool \{.*?'
               r'    \} else \{\s*None\s*\};\n')
    text = unique_regex(text, pattern, lambda _m: replacement, 'replace validation order')
    text = unique(text,
        '''        verification_tool: validation_tool,
        transaction_status: tx_status.as_ref(),''',
        '''        verification_tool: validation_tool,
        candidate_precheck_status: candidate_precheck_status.as_ref(),
        candidate_precheck_files: candidate_precheck_files.as_ref(),
        checkpoint_skip_reason,
        transaction_status: tx_status.as_ref(),''', 'evidence input')
    text = unique(text,
        '''    let verification_ok = verification
        .as_ref()
        .map(tool_result_success)
        .unwrap_or(true)
        && (!mutating || transaction_active);''',
        '''    let verification_ok = verification
        .as_ref()
        .map(tool_result_success)
        .unwrap_or(!mutating)
        && checkpoint_skip_reason.is_none()
        && (!mutating || transaction_active);''', 'verification success')
    json_old = '''                "verification_tool":validation_tool,
                "transaction_active":transaction_active,'''
    json_new = '''                "verification_tool":validation_tool,
                "candidate_precheck_status":candidate_precheck_status,
                "candidate_precheck_files":candidate_precheck_files,
                "checkpoint_skip_reason":checkpoint_skip_reason,
                "checkpoint_executed":verification.is_some(),
                "transaction_active":transaction_active,'''
    if text.count(json_old) != 2:
        raise ValueError('W12 JSON and JSONL output anchors changed')
    text = text.replace(json_old, json_new)
    text = unique(text,
        '''    if !verification_ok {
        return Err(format!(
            "{action} did not pass project-authoritative verification''',
        '''    if !verification_ok {
        if let Some(reason) = checkpoint_skip_reason {
            return Err(format!(
                "{action} blocked before project checkpoint: {reason}; review the candidate evidence receipt (no project FULL was invoked)"
            ));
        }
        return Err(format!(
            "{action} did not pass project-authoritative verification''', 'specific error')
    text = unique(text,
        '''    project_grounding: Option<&'a Value>,
}''',
        '''    project_grounding: Option<&'a Value>,
    candidate_precheck_status: Option<&'a cortex_protocol::ToolResultInput>,
    candidate_precheck_files: Option<&'a cortex_protocol::ToolResultInput>,
    checkpoint_skip_reason: Option<&'a str>,
}''', 'struct receipt')
    text = unique(text,
        '''        "project_grounding": input.project_grounding,
        "verification_tool": input.verification_tool,''',
        '''        "project_grounding": input.project_grounding,
        "candidate_precheck_status": input.candidate_precheck_status,
        "candidate_precheck_files": input.candidate_precheck_files,
        "checkpoint_skip_reason": input.checkpoint_skip_reason,
        "checkpoint_executed": input.verification.is_some(),
        "verification_tool": input.verification_tool,''', 'receipt fields')
    text = unique(text, '"schema_version": 2,\n        "mode": input.mode,', '"schema_version": 3,\n        "mode": input.mode,', 'receipt v3')
    text = unique(text,
        '''                is_error,
            }],''',
        '''                is_error,
                ..AgentToolEvidence::default()
            }],''', 'cli test fixture')
    return text


def patch_test_core(text: str) -> str:
    return text


def patch_cli_tests(text: str) -> str:
    needle = '''    #[test]
    fn w11_repair_preflight_does_not_treat_standalone_cargo_as_empty() {'''
    addition = '''    #[test]
    fn w12_rejected_candidate_never_enters_expensive_checkpoint() {
        use cortex_protocol::ToolResultInput;
        let agent = result_with("source.write_text", true, false);
        let status = ToolResultInput {
            call_id: "status".into(),
            output: json!({"transaction": null}),
            is_error: false,
        };
        let files = ToolResultInput {
            call_id: "files".into(),
            output: json!({"transaction_id": null, "touched": [], "created": []}),
            is_error: false,
        };
        assert_eq!(repair_candidate_gate_decision(Some("candidate-1"), &status, &files, &agent),
            Err("candidate_not_active_after_model"));
    }

    #[test]
    fn w12_retained_candidate_requires_real_file_ledger_and_no_pseudo_call() {
        use cortex_protocol::ToolResultInput;
        let mut agent = result_with("source.write_text", true, false);
        let status = ToolResultInput {
            call_id: "status".into(),
            output: json!({"transaction": {"id":"candidate-1"}}),
            is_error: false,
        };
        let files = ToolResultInput {
            call_id: "files".into(),
            output: json!({"transaction_id":"candidate-1", "touched":["Cargo.toml"], "created":[]}),
            is_error: false,
        };
        assert_eq!(repair_candidate_gate_decision(Some("candidate-1"), &status, &files, &agent), Ok(()));
        agent.text = "<tool_call>source.read {\\"path\\":\\"Cargo.toml\\"}".into();
        assert_eq!(repair_candidate_gate_decision(Some("candidate-1"), &status, &files, &agent),
            Err("unexecuted_tool_call_in_model_report"));
    }

'''
    return unique(text, needle, addition+needle, 'add rust CLI tests')


def apply_transforms(originals: dict[str, bytes]) -> dict[str, bytes]:
    result = dict(originals)
    core = patch_core(originals['crates/cortex_core/src/lib.rs'].decode('utf-8'))
    cli = patch_cli(originals['crates/cortex_cli/src/lib.rs'].decode('utf-8'))
    cli = patch_cli_tests(cli)
    result['crates/cortex_core/src/lib.rs'] = core.encode('utf-8')
    result['crates/cortex_cli/src/lib.rs'] = cli.encode('utf-8')
    return result


def install(root: Path) -> Path:
    root = root.resolve(strict=True)
    originals: dict[str, bytes] = {}
    for name, expected_sha in GREEN_SOURCE_BLOBS.items():
        path = root / name
        if not path.is_file() or path.is_symlink():
            raise ValueError(f"W12 source target unavailable/nonregular: {name}")
        data = path.read_bytes()
        actual = git_blob_hash(data)
        if actual != expected_sha:
            raise ValueError(f"W12 GREEN preimage mismatch: {name}, blob={actual}; no files changed")
        originals[name] = data
    # Finish all transformations before any write. This also rejects anchor drift.
    transformed = apply_transforms(originals)
    if any(transformed[n] == originals[n] for n in originals):
        raise ValueError('W12 no-op source transformation rejected')
    base = root / 'artifacts' / 'updates'
    base.mkdir(parents=True, exist_ok=True)
    destination = base / ('w12-green-source-' + datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ'))
    destination.mkdir(exist_ok=False)
    metadata = {'schema':'cortex.w12.green.source.v1','base_commit':EXPECTED_GREEN_COMMIT,
        'scope':list(originals), 'files':{}}
    for name,data in originals.items():
        b = destination / name
        b.parent.mkdir(parents=True, exist_ok=True)
        b.write_bytes(data)
        metadata['files'][name]={'before_git_blob':git_blob_hash(data),
             'after_git_blob':git_blob_hash(transformed[name]),'backup_relative_path':name}
    (destination / 'manifest.json').write_text(json.dumps(metadata,indent=2)+'\n',encoding='utf-8')
    replaced=[]
    try:
        for name,data in transformed.items():
            path=root/name
            fd,temp=tempfile.mkstemp(prefix='.cortex-w12-',dir=path.parent)
            with os.fdopen(fd,'wb') as stream:
                stream.write(data)
            os.replace(temp,path)
            replaced.append(name)
    except Exception:
        for name in reversed(replaced):
            (root/name).write_bytes(originals[name])
        raise
    return destination


def main() -> None:
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root',required=True,type=Path)
    parser.add_argument('--apply',action='store_true',help='Explicitly apply after strict GREEN source preflight')
    args=parser.parse_args()
    root=args.root.resolve(strict=True)
    report={}
    for name,expected in GREEN_SOURCE_BLOBS.items():
        p=root/name
        observed=git_blob_hash(p.read_bytes()) if p.is_file() else None
        report[name]={'expected':expected,'observed':observed,'match':observed==expected}
    print(json.dumps(report,indent=2))
    if not all(x['match'] for x in report.values()):
        raise SystemExit('W12 preflight mismatch: no files changed; retrieve correct GREEN baseline')
    # Preview performs the complete anchor transformation in memory, too.
    # A green preimage with a changed layout must not produce a misleading PASS.
    originals = {name: (root/name).read_bytes() for name in GREEN_SOURCE_BLOBS}
    apply_transforms(originals)
    print('W12 SOURCE ANCHORS: verified (dry-run transformation succeeded).')
    if not args.apply:
        print('PREVIEW ONLY: rerun with --apply to install source changes; no files changed.')
        return
    backup=install(root)
    print('W12 source candidate installed; backups:',backup)
    print('Next run: cargo fmt --all; then Cortex PCC FULL. No commit/push was performed.')

if __name__=='__main__':
    main()
