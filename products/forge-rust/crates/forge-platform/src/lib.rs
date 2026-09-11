use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlatformSettings {
    pub minimize_to_tray: bool,
    pub close_to_tray: bool,
    pub tray_enabled: bool,
    pub notifications_enabled: bool,
    pub startup_enabled: bool,
}

impl Default for PlatformSettings {
    fn default() -> Self {
        Self {
            minimize_to_tray: true,
            close_to_tray: false,
            tray_enabled: true,
            notifications_enabled: true,
            startup_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevel {
    Info,
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationRecord {
    pub id: String,
    pub level: NotificationLevel,
    pub title: String,
    pub body: String,
    pub project_id: Option<String>,
    pub created_unix_ms: u64,
    pub read: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrayCommand {
    ShowForge,
    HideForge,
    RunFullGate,
    OpenActiveProject,
    OpenCortex,
    OpenSettings,
    Exit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrayMenuItem {
    pub label: String,
    pub command: TrayCommand,
    pub enabled: bool,
    pub separator_after: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformState {
    pub schema: String,
    pub settings: PlatformSettings,
    pub notifications: VecDeque<NotificationRecord>,
}

impl Default for PlatformState {
    fn default() -> Self {
        Self {
            schema: "forge.platform.v1".to_owned(),
            settings: PlatformSettings::default(),
            notifications: VecDeque::new(),
        }
    }
}

pub struct PlatformStore {
    path: PathBuf,
    state: PlatformState,
}

impl PlatformStore {
    pub fn open(root: &Path) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(|error| error.to_string())?;
        let path = root.join("platform.json");
        let state = if path.is_file() {
            serde_json::from_slice(
                &fs::read(&path).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?
        } else {
            PlatformState::default()
        };
        Ok(Self { path, state })
    }

    #[must_use]
    pub fn state(&self) -> &PlatformState {
        &self.state
    }

    pub fn update_settings(&mut self, settings: PlatformSettings) -> Result<(), String> {
        self.state.settings = settings;
        self.save()
    }

    pub fn push_notification(
        &mut self,
        level: NotificationLevel,
        title: impl Into<String>,
        body: impl Into<String>,
        project_id: Option<String>,
    ) -> Result<String, String> {
        let id = format!("notice-{}", unix_ms());
        self.state.notifications.push_front(NotificationRecord {
            id: id.clone(),
            level,
            title: title.into(),
            body: body.into(),
            project_id,
            created_unix_ms: unix_ms(),
            read: false,
        });
        while self.state.notifications.len() > 500 {
            self.state.notifications.pop_back();
        }
        self.save()?;
        Ok(id)
    }

    pub fn mark_read(&mut self, id: &str) -> Result<(), String> {
        if let Some(record) = self
            .state
            .notifications
            .iter_mut()
            .find(|record| record.id == id)
        {
            record.read = true;
        }
        self.save()
    }

    pub fn mark_all_read(&mut self) -> Result<(), String> {
        for record in &mut self.state.notifications {
            record.read = true;
        }
        self.save()
    }

    #[must_use]
    pub fn unread_count(&self) -> usize {
        self.state
            .notifications
            .iter()
            .filter(|record| !record.read)
            .count()
    }

    #[must_use]
    pub fn tray_menu(&self, project_active: bool) -> Vec<TrayMenuItem> {
        vec![
            TrayMenuItem {
                label: "Show Forge".to_owned(),
                command: TrayCommand::ShowForge,
                enabled: true,
                separator_after: false,
            },
            TrayMenuItem {
                label: "Open Cortex".to_owned(),
                command: TrayCommand::OpenCortex,
                enabled: true,
                separator_after: false,
            },
            TrayMenuItem {
                label: "Full Gate Active Project".to_owned(),
                command: TrayCommand::RunFullGate,
                enabled: project_active,
                separator_after: true,
            },
            TrayMenuItem {
                label: "Settings".to_owned(),
                command: TrayCommand::OpenSettings,
                enabled: true,
                separator_after: true,
            },
            TrayMenuItem {
                label: "Exit Forge".to_owned(),
                command: TrayCommand::Exit,
                enabled: true,
                separator_after: false,
            },
        ]
    }

    pub fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&self.state).map_err(|error| error.to_string())?;
        let mut file = File::create(&self.path).map_err(|error| error.to_string())?;
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        file.write_all(b"\n").map_err(|error| error.to_string())
    }
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

    #[test]
    fn defaults_keep_tray_enabled() {
        assert!(PlatformSettings::default().tray_enabled);
    }

    #[test]
    fn full_gate_tray_command_disables_without_project() {
        let state = PlatformState::default();
        let root = std::env::temp_dir().join(format!("forge-platform-{}", unix_ms()));
        fs::create_dir_all(&root).expect("root");
        let store = PlatformStore {
            path: root.join("platform.json"),
            state,
        };
        let menu = store.tray_menu(false);
        assert!(menu
            .iter()
            .find(|item| item.command == TrayCommand::RunFullGate)
            .is_some_and(|item| !item.enabled));
        let _ = fs::remove_dir_all(root);
    }
}
