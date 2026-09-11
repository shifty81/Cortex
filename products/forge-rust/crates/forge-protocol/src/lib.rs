use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub const PROTOCOL_SCHEMA: &str = "forge.stdio_protocol.v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StdioProtocolKind {
    Lsp,
    Dap,
    JsonRpc,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProtocolLaunchSpec {
    pub id: String,
    pub kind: StdioProtocolKind,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

impl ProtocolLaunchSpec {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("protocol launch id cannot be empty".to_owned());
        }
        if self.program.as_os_str().is_empty() {
            return Err("protocol launch program cannot be empty".to_owned());
        }
        if !self.cwd.is_dir() {
            return Err(format!(
                "protocol working directory does not exist: {}",
                self.cwd.display()
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolMessage {
    pub headers: Vec<(String, String)>,
    pub value: Value,
}

pub struct StdioProtocolProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    spec: ProtocolLaunchSpec,
}

impl StdioProtocolProcess {
    pub fn spawn(spec: ProtocolLaunchSpec) -> Result<Self, String> {
        spec.validate()?;
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(|error| {
            format!(
                "failed to launch protocol process {}: {error}",
                spec.program.display()
            )
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "protocol child stdin unavailable".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "protocol child stdout unavailable".to_owned())?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            spec,
        })
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.spec.id
    }

    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn send_value(&mut self, value: &Value) -> Result<(), String> {
        write_framed_json(&mut self.stdin, value)
    }

    pub fn read_value(&mut self) -> Result<ProtocolMessage, String> {
        read_framed_json(&mut self.stdout)
    }

    pub fn request(&mut self, id: Value, method: &str, params: Value) -> Result<Value, String> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.send_value(&request)?;
        loop {
            let message = self.read_value()?;
            if message.value.get("id") == request.get("id") {
                if let Some(error) = message.value.get("error") {
                    return Err(format!("protocol request `{method}` failed: {error}"));
                }
                return Ok(message
                    .value
                    .get("result")
                    .cloned()
                    .unwrap_or(Value::Null));
            }
        }
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send_value(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    pub fn stop(&mut self) -> Result<(), String> {
        match self.child.try_wait() {
            Ok(Some(_)) => Ok(()),
            Ok(None) => {
                self.child.kill().map_err(|error| error.to_string())?;
                let _ = self.child.wait();
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

impl Drop for StdioProtocolProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub fn write_framed_json(writer: &mut impl Write, value: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len()).map_err(|error| error.to_string())?;
    writer.write_all(&body).map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())
}

pub fn read_framed_json(reader: &mut impl BufRead) -> Result<ProtocolMessage, String> {
    let mut headers = Vec::new();
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("protocol stream ended before a complete header".to_owned());
        }
        let trimmed = line.trim_end_matches(|character| character == '\r' || character == '\n');
        if trimmed.is_empty() {
            break;
        }
        let (name, value) = trimmed
            .split_once(':')
            .ok_or_else(|| format!("invalid protocol header: {trimmed}"))?;
        let name = name.trim().to_owned();
        let value = value.trim().to_owned();
        if name.eq_ignore_ascii_case("Content-Length") {
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| format!("invalid Content-Length: {value}"))?,
            );
        }
        headers.push((name, value));
    }
    let content_length = content_length.ok_or_else(|| "protocol Content-Length missing".to_owned())?;
    if content_length > 64 * 1024 * 1024 {
        return Err("protocol frame exceeds 64 MiB safety budget".to_owned());
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).map_err(|error| error.to_string())?;
    let value = serde_json::from_slice(&body).map_err(|error| error.to_string())?;
    Ok(ProtocolMessage { headers, value })
}

pub fn round_trip_for_test(value: &Value) -> Result<Value, String> {
    let mut bytes = Vec::new();
    write_framed_json(&mut bytes, value)?;
    let mut reader = BufReader::new(Cursor::new(bytes));
    Ok(read_framed_json(&mut reader)?.value)
}

#[must_use]
pub fn executable_spec(
    id: &str,
    kind: StdioProtocolKind,
    program: impl AsRef<Path>,
    args: Vec<String>,
    cwd: impl AsRef<Path>,
) -> ProtocolLaunchSpec {
    ProtocolLaunchSpec {
        id: id.to_owned(),
        kind,
        program: program.as_ref().to_path_buf(),
        args,
        cwd: cwd.as_ref().to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_length_frame_round_trips_json() {
        let value = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize"});
        assert_eq!(round_trip_for_test(&value).expect("round trip"), value);
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let bytes = b"Content-Length: 999999999\r\n\r\n".to_vec();
        let mut reader = BufReader::new(Cursor::new(bytes));
        assert!(read_framed_json(&mut reader).is_err());
    }
}
