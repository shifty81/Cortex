use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CortexConnectionState {
    Disconnected,
    Connecting,
    Ready,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CortexCapability {
    pub id: String,
    pub label: String,
    pub risk: String,
}

/// FR01 defines the boundary only. Existing Cortex service/runtime integration is
/// intentionally not faked inside the Forge Rust GUI.
pub trait CortexClient: Send + Sync {
    fn state(&self) -> CortexConnectionState;
    fn capabilities(&self) -> Vec<CortexCapability>;
}
