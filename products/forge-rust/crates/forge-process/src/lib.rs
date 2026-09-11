use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::Sender,
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const OPERATION_RECEIPT_SCHEMA: &str = "forge.operation.receipt.v1";
pub const OPERATION_HOST_SCHEMA: &str = "forge.operation_host.v1";

static OPERATION_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    StartFailed,
    Interrupted,
}

impl OperationState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::StartFailed => "start_failed",
            Self::Interrupted => "interrupted",
        }
    }

    #[must_use]
    pub const fn terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::StartFailed | Self::Interrupted
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperationHostCapabilities {
    pub schema: String,
    pub schema_version: u32,
    pub durable_operation_ids: bool,
    pub persistent_receipts: bool,
    pub persistent_logs: bool,
    pub cancellation: bool,
    pub interrupted_recovery: bool,
    pub durable_history: bool,
}

#[must_use]
pub fn operation_host_capabilities() -> OperationHostCapabilities {
    OperationHostCapabilities {
        schema: OPERATION_HOST_SCHEMA.to_owned(),
        schema_version: 1,
        durable_operation_ids: true,
        persistent_receipts: true,
        persistent_logs: true,
        cancellation: true,
        interrupted_recovery: true,
        durable_history: true,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperationReceipt {
    pub schema: String,
    pub operation_id: String,
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub state: OperationState,
    pub pid: Option<u32>,
    pub started_unix_ms: u64,
    pub completed_unix_ms: Option<u64>,
    pub elapsed_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub cancel_requested: bool,
    pub log_path: String,
    pub receipt_path: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum OperationEvent {
    Queued {
        operation_id: String,
        label: String,
        log_path: String,
        receipt_path: String,
    },
    Started {
        operation_id: String,
        label: String,
        pid: u32,
    },
    Output {
        operation_id: String,
        stderr: bool,
        line: String,
    },
    CancelRequested {
        operation_id: String,
        label: String,
    },
    Cancelled {
        operation_id: String,
        label: String,
        elapsed_ms: u64,
        log_path: String,
        receipt_path: String,
    },
    Finished {
        operation_id: String,
        label: String,
        success: bool,
        code: Option<i32>,
        elapsed_ms: u64,
        log_path: String,
        receipt_path: String,
    },
    FailedToStart {
        operation_id: String,
        label: String,
        error: String,
        log_path: String,
        receipt_path: String,
    },
    HostError {
        operation_id: String,
        label: String,
        error: String,
        log_path: String,
        receipt_path: String,
    },
}

#[derive(Debug, Clone)]
pub struct OperationHandle {
    id: String,
    cancel: Arc<AtomicBool>,
}

impl OperationHandle {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn cancellation_requested(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

#[must_use]
pub fn operation_root(project_root: &Path) -> PathBuf {
    project_root
        .join("artifacts")
        .join("forge-rust")
        .join("operations")
}

pub fn spawn(spec: CommandSpec, tx: Sender<OperationEvent>) -> Result<OperationHandle, String> {
    let (operation_id, operation_dir) = reserve_operation_dir(&spec.cwd)?;
    let log_path = operation_dir.join("operation.log");
    let receipt_path = operation_dir.join("receipt.json");
    File::create(&log_path)
        .map_err(|error| format!("failed to create operation log {}: {error}", log_path.display()))?;

    let started_unix_ms = unix_ms();
    let receipt = OperationReceipt {
        schema: OPERATION_RECEIPT_SCHEMA.to_owned(),
        operation_id: operation_id.clone(),
        label: spec.label.clone(),
        program: spec.program.clone(),
        args: spec.args.clone(),
        cwd: spec.cwd.to_string_lossy().into_owned(),
        state: OperationState::Queued,
        pid: None,
        started_unix_ms,
        completed_unix_ms: None,
        elapsed_ms: None,
        exit_code: None,
        cancel_requested: false,
        log_path: log_path.to_string_lossy().into_owned(),
        receipt_path: receipt_path.to_string_lossy().into_owned(),
        error: None,
    };
    persist_receipt(&receipt_path, &receipt)?;
    append_log(&log_path, &format!("[INFO] QUEUED {}", spec.label))?;

    let cancel = Arc::new(AtomicBool::new(false));
    let handle = OperationHandle {
        id: operation_id.clone(),
        cancel: Arc::clone(&cancel),
    };
    let _ = tx.send(OperationEvent::Queued {
        operation_id: operation_id.clone(),
        label: spec.label.clone(),
        log_path: receipt.log_path.clone(),
        receipt_path: receipt.receipt_path.clone(),
    });

    thread::spawn(move || {
        run_operation(
            spec,
            tx,
            cancel,
            receipt,
            log_path,
            receipt_path,
            operation_id,
        );
    });
    Ok(handle)
}

fn run_operation(
    spec: CommandSpec,
    tx: Sender<OperationEvent>,
    cancel: Arc<AtomicBool>,
    mut receipt: OperationReceipt,
    log_path: PathBuf,
    receipt_path: PathBuf,
    operation_id: String,
) {
    let started = Instant::now();

    if cancel.load(Ordering::SeqCst) {
        receipt.state = OperationState::Cancelled;
        receipt.cancel_requested = true;
        receipt.completed_unix_ms = Some(unix_ms());
        receipt.elapsed_ms = Some(0);
        let _ = persist_receipt(&receipt_path, &receipt);
        let _ = append_log(&log_path, "[WARN] CANCELLED before process start");
        let _ = tx.send(OperationEvent::Cancelled {
            operation_id,
            label: spec.label,
            elapsed_ms: 0,
            log_path: receipt.log_path,
            receipt_path: receipt.receipt_path,
        });
        return;
    }

    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            receipt.state = OperationState::StartFailed;
            receipt.error = Some(error.to_string());
            receipt.completed_unix_ms = Some(unix_ms());
            receipt.elapsed_ms = Some(elapsed_ms(started));
            let _ = persist_receipt(&receipt_path, &receipt);
            let _ = append_log(&log_path, &format!("[FAIL] START {}: {error}", spec.label));
            let _ = tx.send(OperationEvent::FailedToStart {
                operation_id,
                label: spec.label,
                error: error.to_string(),
                log_path: receipt.log_path,
                receipt_path: receipt.receipt_path,
            });
            return;
        }
    };

    let pid = child.id();
    receipt.state = OperationState::Running;
    receipt.pid = Some(pid);
    if let Err(error) = persist_receipt(&receipt_path, &receipt) {
        let _ = request_child_stop(&mut child);
        receipt.state = OperationState::Failed;
        receipt.error = Some(error.clone());
        receipt.completed_unix_ms = Some(unix_ms());
        receipt.elapsed_ms = Some(elapsed_ms(started));
        let _ = append_log(&log_path, &format!("[FAIL] durable receipt update failed: {error}"));
        let _ = tx.send(OperationEvent::HostError {
            operation_id,
            label: spec.label,
            error,
            log_path: receipt.log_path,
            receipt_path: receipt.receipt_path,
        });
        return;
    }

    let _ = append_log(&log_path, &format!("[INFO] START {} (pid {pid})", spec.label));
    let _ = tx.send(OperationEvent::Started {
        operation_id: operation_id.clone(),
        label: spec.label.clone(),
        pid,
    });

    let shared_log = match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(file) => Arc::new(Mutex::new(file)),
        Err(error) => {
            let error = format!("failed to open persistent operation log: {error}");
            let _ = request_child_stop(&mut child);
            receipt.state = OperationState::Failed;
            receipt.error = Some(error.clone());
            receipt.completed_unix_ms = Some(unix_ms());
            receipt.elapsed_ms = Some(elapsed_ms(started));
            let _ = persist_receipt(&receipt_path, &receipt);
            let _ = tx.send(OperationEvent::HostError {
                operation_id,
                label: spec.label,
                error,
                log_path: receipt.log_path,
                receipt_path: receipt.receipt_path,
            });
            return;
        }
    };

    let stdout_thread = child.stdout.take().map(|stdout| {
        spawn_reader(
            stdout,
            false,
            operation_id.clone(),
            tx.clone(),
            Arc::clone(&shared_log),
        )
    });
    let stderr_thread = child.stderr.take().map(|stderr| {
        spawn_reader(
            stderr,
            true,
            operation_id.clone(),
            tx.clone(),
            Arc::clone(&shared_log),
        )
    });

    let mut cancel_announced = false;
    let status = loop {
        if cancel.load(Ordering::SeqCst) && !cancel_announced {
            cancel_announced = true;
            receipt.cancel_requested = true;
            let _ = persist_receipt(&receipt_path, &receipt);
            let _ = write_shared_log(&shared_log, "[WARN] CANCEL REQUESTED");
            let _ = tx.send(OperationEvent::CancelRequested {
                operation_id: operation_id.clone(),
                label: spec.label.clone(),
            });
            if let Err(error) = request_child_stop(&mut child) {
                let _ = write_shared_log(
                    &shared_log,
                    &format!("[WARN] process-tree stop fallback reported: {error}"),
                );
            }
        }

        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(error) => break Err(error.to_string()),
        }
    };

    if let Some(handle) = stdout_thread {
        let _ = handle.join();
    }
    if let Some(handle) = stderr_thread {
        let _ = handle.join();
    }

    let elapsed_ms = elapsed_ms(started);
    receipt.completed_unix_ms = Some(unix_ms());
    receipt.elapsed_ms = Some(elapsed_ms);

    match status {
        Ok(status) if cancel_announced || cancel.load(Ordering::SeqCst) => {
            receipt.state = OperationState::Cancelled;
            receipt.exit_code = status.code();
            receipt.cancel_requested = true;
            let _ = persist_receipt(&receipt_path, &receipt);
            let _ = write_shared_log(
                &shared_log,
                &format!("[WARN] CANCELLED {} ({elapsed_ms} ms)", spec.label),
            );
            let _ = tx.send(OperationEvent::Cancelled {
                operation_id,
                label: spec.label,
                elapsed_ms,
                log_path: receipt.log_path,
                receipt_path: receipt.receipt_path,
            });
        }
        Ok(status) => {
            let success = status.success();
            receipt.state = if success {
                OperationState::Succeeded
            } else {
                OperationState::Failed
            };
            receipt.exit_code = status.code();
            let _ = persist_receipt(&receipt_path, &receipt);
            let token = if success { "PASS" } else { "FAIL" };
            let _ = write_shared_log(
                &shared_log,
                &format!(
                    "[{token}] END {} ({elapsed_ms} ms, exit {})",
                    spec.label,
                    status
                        .code()
                        .map_or_else(|| "?".to_owned(), |value| value.to_string())
                ),
            );
            let _ = tx.send(OperationEvent::Finished {
                operation_id,
                label: spec.label,
                success,
                code: status.code(),
                elapsed_ms,
                log_path: receipt.log_path,
                receipt_path: receipt.receipt_path,
            });
        }
        Err(error) => {
            receipt.state = OperationState::Failed;
            receipt.error = Some(error.clone());
            let _ = persist_receipt(&receipt_path, &receipt);
            let _ = write_shared_log(&shared_log, &format!("[FAIL] PROCESS HOST: {error}"));
            let _ = tx.send(OperationEvent::HostError {
                operation_id,
                label: spec.label,
                error,
                log_path: receipt.log_path,
                receipt_path: receipt.receipt_path,
            });
        }
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    reader: R,
    stderr: bool,
    operation_id: String,
    tx: Sender<OperationEvent>,
    log: Arc<Mutex<File>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let stream = if stderr { "STDERR" } else { "STDOUT" };
            let _ = write_shared_log(&log, &format!("[{stream}] {line}"));
            let _ = tx.send(OperationEvent::Output {
                operation_id: operation_id.clone(),
                stderr,
                line,
            });
        }
    })
}

fn request_child_stop(child: &mut Child) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let pid = child.id().to_string();
        let mut taskkill = Command::new("taskkill");
        taskkill
            .args(["/PID", &pid, "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        if taskkill.status().is_ok_and(|status| status.success()) {
            return Ok(());
        }
    }

    child
        .kill()
        .map_err(|error| format!("failed to terminate child process {}: {error}", child.id()))
}

pub fn recover_interrupted_operations(project_root: &Path) -> Result<usize, String> {
    let root = operation_root(project_root);
    if !root.is_dir() {
        return Ok(0);
    }

    let mut recovered = 0usize;
    for entry in fs::read_dir(&root)
        .map_err(|error| format!("failed to read operation history {}: {error}", root.display()))?
        .flatten()
    {
        let receipt_path = entry.path().join("receipt.json");
        if !receipt_path.is_file() {
            continue;
        }
        let Ok(text) = fs::read_to_string(&receipt_path) else {
            continue;
        };
        let Ok(mut receipt) = serde_json::from_str::<OperationReceipt>(&text) else {
            continue;
        };
        if receipt.state.terminal() {
            continue;
        }

        let completed = unix_ms();
        receipt.state = OperationState::Interrupted;
        receipt.completed_unix_ms = Some(completed);
        receipt.elapsed_ms = Some(completed.saturating_sub(receipt.started_unix_ms));
        receipt.error = Some(
            "operation host restarted before a terminal receipt; external process status is unknown"
                .to_owned(),
        );
        persist_receipt(&receipt_path, &receipt)?;
        append_log(
            Path::new(&receipt.log_path),
            "[WARN] INTERRUPTED: operation host restarted before terminal state",
        )?;
        recovered += 1;
    }
    Ok(recovered)
}

pub fn recent_operation_receipts(
    project_root: &Path,
    limit: usize,
) -> Result<Vec<OperationReceipt>, String> {
    let root = operation_root(project_root);
    if !root.is_dir() {
        return Ok(Vec::new());
    }

    let mut receipts = Vec::new();
    for entry in fs::read_dir(&root)
        .map_err(|error| format!("failed to read operation history {}: {error}", root.display()))?
        .flatten()
    {
        let receipt_path = entry.path().join("receipt.json");
        if !receipt_path.is_file() {
            continue;
        }
        let Ok(text) = fs::read_to_string(&receipt_path) else {
            continue;
        };
        if let Ok(receipt) = serde_json::from_str::<OperationReceipt>(&text) {
            receipts.push(receipt);
        }
    }
    receipts.sort_by(|left, right| right.started_unix_ms.cmp(&left.started_unix_ms));
    receipts.truncate(limit);
    Ok(receipts)
}

fn reserve_operation_dir(project_root: &Path) -> Result<(String, PathBuf), String> {
    let root = operation_root(project_root);
    fs::create_dir_all(&root)
        .map_err(|error| format!("failed to create operation root {}: {error}", root.display()))?;

    for _ in 0..32 {
        let operation_id = new_operation_id();
        let dir = root.join(&operation_id);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok((operation_id, dir)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "failed to reserve operation directory {}: {error}",
                    dir.display()
                ));
            }
        }
    }
    Err("failed to reserve a unique durable operation identity".to_owned())
}

fn new_operation_id() -> String {
    let counter = OPERATION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("forgeop-{}-{}-{counter:06}", unix_ms(), std::process::id())
}

fn persist_receipt(path: &Path, receipt: &OperationReceipt) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(receipt)
        .map_err(|error| format!("failed to serialize operation receipt: {error}"))?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("operation receipt has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create operation receipt directory {}: {error}",
            parent.display()
        )
    })?;

    let temp = parent.join(format!(
        "receipt.tmp-{}-{}",
        std::process::id(),
        OPERATION_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    {
        let mut file = File::create(&temp)
            .map_err(|error| format!("failed to create receipt temp {}: {error}", temp.display()))?;
        file.write_all(&bytes)
            .map_err(|error| format!("failed to write receipt temp {}: {error}", temp.display()))?;
        file.write_all(b"\n")
            .map_err(|error| format!("failed to finalize receipt temp {}: {error}", temp.display()))?;
        file.flush()
            .map_err(|error| format!("failed to flush receipt temp {}: {error}", temp.display()))?;
        let _ = file.sync_all();
    }

    if path.exists() {
        fs::remove_file(path).map_err(|error| {
            format!(
                "failed to replace existing operation receipt {}: {error}",
                path.display()
            )
        })?;
    }
    fs::rename(&temp, path).map_err(|error| {
        format!(
            "failed to publish operation receipt {} -> {}: {error}",
            temp.display(),
            path.display()
        )
    })
}

fn append_log(path: &Path, line: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("failed to open operation log {}: {error}", path.display()))?;
    writeln!(file, "{line}")
        .map_err(|error| format!("failed to write operation log {}: {error}", path.display()))?;
    file.flush()
        .map_err(|error| format!("failed to flush operation log {}: {error}", path.display()))
}

fn write_shared_log(log: &Arc<Mutex<File>>, line: &str) -> Result<(), String> {
    let mut file = log
        .lock()
        .map_err(|_| "operation log lock poisoned".to_owned())?;
    writeln!(file, "{line}").map_err(|error| format!("failed to write operation log: {error}"))?;
    file.flush()
        .map_err(|error| format!("failed to flush operation log: {error}"))
}

fn elapsed_ms(started: Instant) -> u64 {
    started
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "forge-process-{name}-{}-{}",
            std::process::id(),
            OPERATION_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("test root");
        root
    }

    fn test_receipt(root: &Path, id: &str, state: OperationState, started_unix_ms: u64) -> OperationReceipt {
        let dir = operation_root(root).join(id);
        fs::create_dir_all(&dir).expect("operation dir");
        OperationReceipt {
            schema: OPERATION_RECEIPT_SCHEMA.to_owned(),
            operation_id: id.to_owned(),
            label: "test".to_owned(),
            program: "test".to_owned(),
            args: Vec::new(),
            cwd: root.to_string_lossy().into_owned(),
            state,
            pid: None,
            started_unix_ms,
            completed_unix_ms: None,
            elapsed_ms: None,
            exit_code: None,
            cancel_requested: false,
            log_path: dir.join("operation.log").to_string_lossy().into_owned(),
            receipt_path: dir.join("receipt.json").to_string_lossy().into_owned(),
            error: None,
        }
    }

    #[test]
    fn operation_host_declares_durable_authority() {
        let capabilities = operation_host_capabilities();
        assert_eq!(capabilities.schema, OPERATION_HOST_SCHEMA);
        assert!(capabilities.durable_operation_ids);
        assert!(capabilities.persistent_receipts);
        assert!(capabilities.persistent_logs);
        assert!(capabilities.cancellation);
        assert!(capabilities.interrupted_recovery);
        assert!(capabilities.durable_history);
    }

    #[test]
    fn interrupted_receipt_is_recovered_truthfully() {
        let root = test_root("recovery");
        let receipt = test_receipt(&root, "op-running", OperationState::Running, unix_ms());
        let receipt_path = PathBuf::from(&receipt.receipt_path);
        File::create(PathBuf::from(&receipt.log_path)).expect("log");
        persist_receipt(&receipt_path, &receipt).expect("persist");

        assert_eq!(recover_interrupted_operations(&root).expect("recover"), 1);
        let history = recent_operation_receipts(&root, 8).expect("history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].state, OperationState::Interrupted);
        assert!(history[0].error.as_deref().is_some_and(|value| value.contains("status is unknown")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recent_history_is_newest_first() {
        let root = test_root("history");
        let first = test_receipt(&root, "op-1", OperationState::Succeeded, 10);
        let second = test_receipt(&root, "op-2", OperationState::Failed, 20);
        File::create(PathBuf::from(&first.log_path)).expect("first log");
        File::create(PathBuf::from(&second.log_path)).expect("second log");
        persist_receipt(Path::new(&first.receipt_path), &first).expect("first");
        persist_receipt(Path::new(&second.receipt_path), &second).expect("second");

        let history = recent_operation_receipts(&root, 8).expect("history");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].operation_id, "op-2");
        assert_eq!(history[1].operation_id, "op-1");

        let _ = fs::remove_dir_all(root);
    }
}
