use forge_state::{ForgeStateStore, ServiceRecord, ServiceState};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceDefinition {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub endpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceLaunch {
    pub id: String,
    pub pid: u32,
    pub log_path: PathBuf,
    pub started_unix_ms: u64,
}

pub struct ServiceManager {
    log_root: PathBuf,
}

impl ServiceManager {
    pub fn new(log_root: impl Into<PathBuf>) -> Self { Self { log_root: log_root.into() } }

    pub fn start(
        &self,
        store: &mut ForgeStateStore,
        definition: &ServiceDefinition,
    ) -> Result<ServiceLaunch, String> {
        validate_definition(definition)?;
        fs::create_dir_all(&self.log_root).map_err(|error| error.to_string())?;
        let log_path = self.log_root.join(format!("{}-{}.log", definition.id, unix_ms()));
        let stdout = OpenOptions::new().create(true).append(true).open(&log_path).map_err(|error| error.to_string())?;
        let stderr = stdout.try_clone().map_err(|error| error.to_string())?;
        let mut command = Command::new(&definition.program);
        command.args(&definition.args).current_dir(&definition.cwd).stdin(Stdio::null()).stdout(Stdio::from(stdout)).stderr(Stdio::from(stderr));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let child = command.spawn().map_err(|error| format!("failed to start service {}: {error}", definition.id))?;
        let launch = ServiceLaunch { id: definition.id.clone(), pid: child.id(), log_path: log_path.clone(), started_unix_ms: unix_ms() };
        store.upsert_service(ServiceRecord {
            id: definition.id.clone(), label: definition.label.clone(), kind: definition.kind.clone(),
            state: ServiceState::Running, endpoint: definition.endpoint.clone(), pid: Some(child.id()),
            detail: format!("started by Rust Forge; log={}", log_path.display()), updated_unix_ms: 0,
        })?;
        Ok(launch)
    }

    pub fn stop(&self, store: &mut ForgeStateStore, id: &str) -> Result<(), String> {
        let service = store.snapshot().services.iter().find(|service| service.id == id).cloned()
            .ok_or_else(|| format!("Forge service is not registered: {id}"))?;
        if let Some(pid) = service.pid {
            stop_pid(pid)?;
        }
        store.upsert_service(ServiceRecord {
            state: ServiceState::Stopped, pid: None, detail: "stopped by Rust Forge".to_owned(), updated_unix_ms: 0,
            ..service
        })
    }

    pub fn refresh(&self, store: &mut ForgeStateStore, id: &str) -> Result<ServiceState, String> {
        let service = store.snapshot().services.iter().find(|service| service.id == id).cloned()
            .ok_or_else(|| format!("Forge service is not registered: {id}"))?;
        let running = service.pid.is_some_and(process_alive);
        let state = if running { ServiceState::Running } else { ServiceState::Stopped };
        if state != service.state {
            store.upsert_service(ServiceRecord { state, pid: if running { service.pid } else { None }, updated_unix_ms: 0, ..service })?;
        }
        Ok(state)
    }
}

pub fn process_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let filter = format!("PID eq {pid}");
        return Command::new("tasklist")
            .args(["/FI", &filter, "/FO", "CSV", "/NH"])
            .output()
            .ok()
            .is_some_and(|output| output.status.success() && String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()));
    }
    #[cfg(not(windows))]
    {
        Command::new("kill").args(["-0", &pid.to_string()]).status().is_ok_and(|status| status.success())
    }
}

fn stop_pid(pid: u32) -> Result<(), String> {
    #[cfg(windows)]
    {
        let status = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).status().map_err(|error| error.to_string())?;
        if status.success() { return Ok(()); }
    }
    #[cfg(not(windows))]
    {
        let status = Command::new("kill").args(["-TERM", &pid.to_string()]).status().map_err(|error| error.to_string())?;
        if status.success() { return Ok(()); }
    }
    Err(format!("failed to stop service process {pid}"))
}

fn validate_definition(definition: &ServiceDefinition) -> Result<(), String> {
    if definition.id.len() < 2 || !definition.id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
        return Err("invalid Forge service id".to_owned());
    }
    if definition.program.trim().is_empty() { return Err("Forge service program is required".to_owned()); }
    if !definition.cwd.is_dir() { return Err(format!("Forge service cwd does not exist: {}", definition.cwd.display())); }
    Ok(())
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_service_ids() {
        let definition = ServiceDefinition { id:"cortex-model".to_owned(), label:"Model".to_owned(), kind:"model".to_owned(), program:"x".to_owned(), args:Vec::new(), cwd:std::env::temp_dir(), endpoint:None };
        assert!(validate_definition(&definition).is_ok());
    }
}
