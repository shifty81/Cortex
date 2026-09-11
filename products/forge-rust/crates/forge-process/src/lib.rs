use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone)]
pub enum OperationEvent {
    Started { label: String, pid: u32 },
    Output { stderr: bool, line: String },
    Finished {
        label: String,
        success: bool,
        code: Option<i32>,
        elapsed_ms: u128,
    },
    FailedToStart { label: String, error: String },
}

pub fn spawn(spec: CommandSpec, tx: Sender<OperationEvent>) {
    thread::spawn(move || {
        let started = Instant::now();
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
                let _ = tx.send(OperationEvent::FailedToStart {
                    label: spec.label,
                    error: error.to_string(),
                });
                return;
            }
        };

        let pid = child.id();
        let _ = tx.send(OperationEvent::Started {
            label: spec.label.clone(),
            pid,
        });

        let stdout_thread = child.stdout.take().map(|stdout| {
            let tx = tx.clone();
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let _ = tx.send(OperationEvent::Output {
                        stderr: false,
                        line,
                    });
                }
            })
        });

        let stderr_thread = child.stderr.take().map(|stderr| {
            let tx = tx.clone();
            thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let _ = tx.send(OperationEvent::Output {
                        stderr: true,
                        line,
                    });
                }
            })
        });

        let status = child.wait();
        if let Some(handle) = stdout_thread {
            let _ = handle.join();
        }
        if let Some(handle) = stderr_thread {
            let _ = handle.join();
        }

        match status {
            Ok(status) => {
                let _ = tx.send(OperationEvent::Finished {
                    label: spec.label,
                    success: status.success(),
                    code: status.code(),
                    elapsed_ms: started.elapsed().as_millis(),
                });
            }
            Err(error) => {
                let _ = tx.send(OperationEvent::FailedToStart {
                    label: spec.label,
                    error: error.to_string(),
                });
            }
        }
    });
}
