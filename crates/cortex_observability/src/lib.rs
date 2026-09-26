//! Generic Cortex observability, incident-memory, and semantic-vector contracts.
//!
//! Raw project logs remain authoritative evidence. The event ledger is a normalized
//! operational history. Vector memory is a rebuildable semantic index whose records
//! always retain provenance back to exact project/session/build/trace/debug-bundle
//! identities and evidence paths.

use cortex_protocol::{EmbeddingProvider, EmbeddingRequest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static ID_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub const EVENT_SCHEMA: &str = "cortex.project_event.v1";
pub const INCIDENT_SCHEMA: &str = "cortex.incident.v1";
pub const HEALTH_SCHEMA: &str = "cortex.health_snapshot.v1";
pub const VECTOR_CANDIDATE_SCHEMA: &str = "cortex.vector_candidate.v1";
pub const VECTOR_MEMORY_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventSeverity {
    Trace,
    Debug,
    Info,
    Warning,
    Error,
    Critical,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectEventKind {
    ProjectOpened,
    ProjectClosed,
    ChatCreated,
    ChatRequestStarted,
    ChatProviderRequest,
    ChatStreamStarted,
    ChatStreamChunk,
    ChatCompleted,
    ChatFailed,
    ToolStarted,
    ToolCompleted,
    ToolFailed,
    TransactionStarted,
    TransactionCommitted,
    TransactionRolledBack,
    PatchDetected,
    PatchApplied,
    PatchRejected,
    BuildStarted,
    BuildCompleted,
    BuildFailed,
    TestStarted,
    TestCompleted,
    TestFailed,
    RuntimeStarted,
    RuntimeStopped,
    RuntimeFailed,
    DebugBundleCreated,
    MemoryIngested,
    HealthChanged,
    Custom,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TraceContext {
    pub project_id: Option<String>,
    pub session_id: Option<String>,
    pub conversation_id: Option<String>,
    pub message_id: Option<String>,
    pub trace_id: String,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub build_id: Option<String>,
    pub job_id: Option<String>,
    pub transaction_id: Option<String>,
    pub patch_id: Option<String>,
    pub debug_bundle_id: Option<String>,
}

impl TraceContext {
    pub fn new(trace_id: impl Into<String>) -> Self {
        Self {
            trace_id: trace_id.into(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectEvent {
    pub schema: String,
    pub event_id: String,
    pub timestamp_unix_ms: u128,
    pub severity: EventSeverity,
    pub kind: ProjectEventKind,
    pub source: String,
    pub summary: String,
    pub trace: TraceContext,
    #[serde(default)]
    pub attributes: BTreeMap<String, Value>,
    #[serde(default)]
    pub evidence_paths: Vec<String>,
}

impl ProjectEvent {
    pub fn new(
        event_id: impl Into<String>,
        kind: ProjectEventKind,
        source: impl Into<String>,
        summary: impl Into<String>,
        trace: TraceContext,
    ) -> Self {
        Self {
            schema: EVENT_SCHEMA.into(),
            event_id: event_id.into(),
            timestamp_unix_ms: unix_ms(),
            severity: EventSeverity::Info,
            kind,
            source: source.into(),
            summary: summary.into(),
            trace,
            attributes: BTreeMap::new(),
            evidence_paths: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IncidentRecord {
    pub schema: String,
    pub incident_id: String,
    pub project_id: Option<String>,
    pub opened_unix_ms: u128,
    pub closed_unix_ms: Option<u128>,
    pub title: String,
    pub fingerprint: String,
    pub status: String,
    pub severity: EventSeverity,
    pub first_event_id: String,
    pub last_event_id: String,
    #[serde(default)]
    pub related_event_ids: Vec<String>,
    #[serde(default)]
    pub evidence_paths: Vec<String>,
    pub resolution_summary: Option<String>,
    pub successful_patch_id: Option<String>,
    pub successful_build_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VectorCandidate {
    pub schema: String,
    pub candidate_id: String,
    pub project_id: Option<String>,
    pub source_kind: String,
    pub source_id: String,
    pub title: String,
    pub text: String,
    pub importance: f32,
    pub content_hash: String,
    #[serde(default)]
    pub evidence_paths: Vec<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

impl VectorCandidate {
    pub fn is_embedding_worthy(&self) -> bool {
        self.importance >= 0.35 && !self.text.trim().is_empty()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentHealth {
    pub name: String,
    pub status: String,
    pub message: Option<String>,
    pub checked_unix_ms: u128,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthSnapshot {
    pub schema: String,
    pub snapshot_id: String,
    pub timestamp_unix_ms: u128,
    pub project_id: Option<String>,
    #[serde(default)]
    pub components: Vec<ComponentHealth>,
}

impl HealthSnapshot {
    pub fn new(snapshot_id: impl Into<String>, project_id: Option<String>) -> Self {
        Self {
            schema: HEALTH_SCHEMA.into(),
            snapshot_id: snapshot_id.into(),
            timestamp_unix_ms: unix_ms(),
            project_id,
            components: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct JsonlEventStore {
    path: PathBuf,
}
impl JsonlEventStore {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(Self { path })
    }
    pub fn append(&self, event: &ProjectEvent) -> io::Result<()> {
        append_jsonl(&self.path, event)
    }
    pub fn read_all(&self) -> io::Result<Vec<ProjectEvent>> {
        read_jsonl(&self.path)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone, Debug)]
pub struct JsonlIncidentStore {
    path: PathBuf,
}
impl JsonlIncidentStore {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(Self { path })
    }
    pub fn append(&self, incident: &IncidentRecord) -> io::Result<()> {
        append_jsonl(&self.path, incident)
    }
    pub fn read_all(&self) -> io::Result<Vec<IncidentRecord>> {
        read_jsonl(&self.path)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone, Debug)]
pub struct JsonlVectorCandidateStore {
    path: PathBuf,
}
impl JsonlVectorCandidateStore {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(Self { path })
    }
    pub fn append_if_new(&self, candidate: &VectorCandidate) -> io::Result<bool> {
        let existing = self.read_all()?;
        if existing.iter().any(|item| {
            item.content_hash == candidate.content_hash && item.project_id == candidate.project_id
        }) {
            return Ok(false);
        }
        append_jsonl(&self.path, candidate)?;
        Ok(true)
    }
    pub fn read_all(&self) -> io::Result<Vec<VectorCandidate>> {
        read_jsonl(&self.path)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone, Debug)]
pub struct ProjectObservability {
    event_store: JsonlEventStore,
    incident_store: JsonlIncidentStore,
    candidate_store: JsonlVectorCandidateStore,
    state_root: PathBuf,
}

impl ProjectObservability {
    pub fn open(_project_root: &Path, state_root: &Path) -> io::Result<Self> {
        // Operational observability is portable runtime state, never source-tree state.
        // Keeping the ledger under state_root prevents chat/build activity from dirtying
        // a governed project checkout (historically <project>/logs/events).
        let event_dir = state_root.join("observability").join("events");
        Ok(Self {
            event_store: JsonlEventStore::open(event_dir.join("cortex-project-events.jsonl"))?,
            incident_store: JsonlIncidentStore::open(event_dir.join("cortex-incidents.jsonl"))?,
            candidate_store: JsonlVectorCandidateStore::open(
                event_dir.join("cortex-vector-candidates.jsonl"),
            )?,
            state_root: state_root.to_path_buf(),
        })
    }
    pub fn record(&self, event: &ProjectEvent) -> io::Result<bool> {
        self.event_store.append(event)?;
        if let Some(incident) = event_to_incident(event) {
            self.incident_store.append(&incident)?;
        }
        match event_to_vector_candidate(event) {
            Some(candidate) => self.candidate_store.append_if_new(&candidate),
            None => Ok(false),
        }
    }
    pub fn events(&self) -> io::Result<Vec<ProjectEvent>> {
        self.event_store.read_all()
    }
    pub fn incidents(&self) -> io::Result<Vec<IncidentRecord>> {
        self.incident_store.read_all()
    }
    pub fn candidates(&self) -> io::Result<Vec<VectorCandidate>> {
        self.candidate_store.read_all()
    }
    pub fn event_path(&self) -> &Path {
        self.event_store.path()
    }
    pub fn incident_path(&self) -> &Path {
        self.incident_store.path()
    }
    pub fn candidate_path(&self) -> &Path {
        self.candidate_store.path()
    }
    pub fn vector_database_path(&self) -> PathBuf {
        self.state_root
            .join("observability")
            .join("vector_memory.json")
    }
    pub fn health_path(&self) -> PathBuf {
        self.state_root
            .join("observability")
            .join("health-latest.json")
    }
    pub fn write_health(&self, snapshot: &HealthSnapshot) -> io::Result<()> {
        let path = self.health_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            path,
            serde_json::to_vec_pretty(snapshot).map_err(io::Error::other)?,
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VectorMemoryRecord {
    pub candidate_id: String,
    pub project_id: Option<String>,
    pub source_id: String,
    pub title: String,
    pub text: String,
    pub content_hash: String,
    #[serde(default)]
    pub evidence_paths: Vec<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    pub vector: Vec<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VectorMemoryHit {
    pub candidate_id: String,
    pub title: String,
    pub score: f32,
    pub text: String,
    #[serde(default)]
    pub evidence_paths: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VectorMemoryDatabase {
    pub schema_version: u32,
    pub embedding_model: Option<String>,
    pub records: Vec<VectorMemoryRecord>,
    pub updated_unix_ms: u128,
}

impl Default for VectorMemoryDatabase {
    fn default() -> Self {
        Self {
            schema_version: VECTOR_MEMORY_SCHEMA_VERSION,
            embedding_model: None,
            records: Vec::new(),
            updated_unix_ms: unix_ms(),
        }
    }
}

impl VectorMemoryDatabase {
    pub fn load_or_default(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(
            path,
            serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }
    pub fn sync_candidates(
        &mut self,
        provider: &dyn EmbeddingProvider,
        model: &str,
        candidates: &[VectorCandidate],
        max_chars: usize,
    ) -> Result<usize, String> {
        if self
            .embedding_model
            .as_deref()
            .map(|existing| existing != model)
            .unwrap_or(false)
        {
            self.records.clear();
        }
        let mut known = self
            .records
            .iter()
            .map(|r| r.content_hash.clone())
            .collect::<BTreeSet<_>>();
        let pending = candidates
            .iter()
            .filter(|c| c.is_embedding_worthy() && !known.contains(&c.content_hash))
            .collect::<Vec<_>>();
        let mut added = 0usize;
        for chunk in pending.chunks(16) {
            let input = chunk
                .iter()
                .map(|c| {
                    c.text
                        .chars()
                        .take(max_chars.clamp(256, 16_000))
                        .collect::<String>()
                })
                .collect::<Vec<_>>();
            let vectors = provider
                .embed(&EmbeddingRequest {
                    model: Some(model.to_string()),
                    input,
                })
                .map_err(|e| e.to_string())?;
            if vectors.len() != chunk.len() {
                return Err(
                    "embedding provider returned a different vector count than requested".into(),
                );
            }
            for (candidate, vector) in chunk.iter().zip(vectors) {
                known.insert(candidate.content_hash.clone());
                self.records.push(VectorMemoryRecord {
                    candidate_id: candidate.candidate_id.clone(),
                    project_id: candidate.project_id.clone(),
                    source_id: candidate.source_id.clone(),
                    title: candidate.title.clone(),
                    text: candidate.text.clone(),
                    content_hash: candidate.content_hash.clone(),
                    evidence_paths: candidate.evidence_paths.clone(),
                    metadata: candidate.metadata.clone(),
                    vector,
                });
                added += 1;
            }
        }
        self.embedding_model = Some(model.to_string());
        self.updated_unix_ms = unix_ms();
        Ok(added)
    }
    pub fn search(
        &self,
        query: &str,
        provider: &dyn EmbeddingProvider,
        limit: usize,
    ) -> Result<Vec<VectorMemoryHit>, String> {
        let model = self
            .embedding_model
            .as_deref()
            .ok_or_else(|| "observability vector memory has not been embedded yet".to_string())?;
        let vectors = provider
            .embed(&EmbeddingRequest {
                model: Some(model.to_string()),
                input: vec![query.to_string()],
            })
            .map_err(|e| e.to_string())?;
        let query_vector = vectors
            .first()
            .ok_or_else(|| "embedding provider returned no query vector".to_string())?;
        let mut hits = self
            .records
            .iter()
            .map(|record| VectorMemoryHit {
                candidate_id: record.candidate_id.clone(),
                title: record.title.clone(),
                score: cosine(query_vector, &record.vector),
                text: record.text.clone(),
                evidence_paths: record.evidence_paths.clone(),
            })
            .collect::<Vec<_>>();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(limit.clamp(1, 100));
        Ok(hits)
    }
}

pub fn new_trace_id(prefix: &str) -> String {
    next_id(prefix)
}
pub fn new_event_id(prefix: &str) -> String {
    next_id(prefix)
}

fn next_id(prefix: &str) -> String {
    let sequence = ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "{}-{}-{}-{sequence}",
        sanitize_id(prefix),
        std::process::id(),
        unix_ms()
    )
}

pub fn redact_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for token in input.split_whitespace() {
        let lower = token.to_ascii_lowercase();
        let looks_secret = lower.starts_with("sk-")
            || lower.contains("api_key=")
            || lower.contains("apikey=")
            || lower.contains("authorization:")
            || lower.contains("bearer=")
            || lower.contains("token=")
            || lower.contains("password=");
        if looks_secret {
            out.push_str("[REDACTED]");
        } else {
            out.push_str(token);
        }
        out.push(' ');
    }
    out.trim_end().to_string()
}

pub fn should_vectorize_event(event: &ProjectEvent) -> bool {
    matches!(
        event.kind,
        ProjectEventKind::ChatFailed
            | ProjectEventKind::ToolFailed
            | ProjectEventKind::PatchApplied
            | ProjectEventKind::PatchRejected
            | ProjectEventKind::BuildFailed
            | ProjectEventKind::TestFailed
            | ProjectEventKind::RuntimeFailed
            | ProjectEventKind::DebugBundleCreated
    ) || matches!(
        event.severity,
        EventSeverity::Error | EventSeverity::Critical
    )
}

pub fn event_to_incident(event: &ProjectEvent) -> Option<IncidentRecord> {
    let failure = matches!(
        event.kind,
        ProjectEventKind::ChatFailed
            | ProjectEventKind::ToolFailed
            | ProjectEventKind::PatchRejected
            | ProjectEventKind::BuildFailed
            | ProjectEventKind::TestFailed
            | ProjectEventKind::RuntimeFailed
            | ProjectEventKind::TransactionRolledBack
    ) || matches!(
        event.severity,
        EventSeverity::Error | EventSeverity::Critical
    );
    if !failure {
        return None;
    }
    let fingerprint = stable_text_fingerprint(&redact_text(&event.summary));
    Some(IncidentRecord {
        schema: INCIDENT_SCHEMA.into(),
        incident_id: format!("incident:{}", event.event_id),
        project_id: event.trace.project_id.clone(),
        opened_unix_ms: event.timestamp_unix_ms,
        closed_unix_ms: None,
        title: event.summary.clone(),
        fingerprint,
        status: "open".into(),
        severity: event.severity.clone(),
        first_event_id: event.event_id.clone(),
        last_event_id: event.event_id.clone(),
        related_event_ids: vec![event.event_id.clone()],
        evidence_paths: event.evidence_paths.clone(),
        resolution_summary: None,
        successful_patch_id: None,
        successful_build_id: None,
    })
}

pub fn event_to_vector_candidate(event: &ProjectEvent) -> Option<VectorCandidate> {
    if !should_vectorize_event(event) {
        return None;
    }
    let text = redact_text(&event.summary);
    Some(VectorCandidate {
        schema: VECTOR_CANDIDATE_SCHEMA.into(),
        candidate_id: format!("event:{}", event.event_id),
        project_id: event.trace.project_id.clone(),
        source_kind: "project_event".into(),
        source_id: event.event_id.clone(),
        title: format!("{:?}: {}", event.kind, event.source),
        content_hash: stable_text_fingerprint(&text),
        text,
        importance: if matches!(event.severity, EventSeverity::Critical) {
            1.0
        } else {
            0.7
        },
        evidence_paths: event.evidence_paths.clone(),
        metadata: event.attributes.clone(),
    })
}

pub fn stable_text_fingerprint(text: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, value).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.flush()
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Vec<T>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let file = File::open(path)?;
    let mut values = Vec::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<T>(trimmed) {
            values.push(value);
        }
    }
    Ok(values)
}

fn sanitize_id(value: &str) -> String {
    let cleaned = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    let cleaned = cleaned.trim_matches('-');
    if cleaned.is_empty() {
        "event".into()
    } else {
        cleaned.into()
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let (mut dot, mut an, mut bn) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        an += x * x;
        bn += y * y;
    }
    let denom = an.sqrt() * bn.sqrt();
    if denom <= f32::EPSILON {
        0.0
    } else {
        dot / denom
    }
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failure_event_becomes_vector_candidate_with_provenance() {
        let mut trace = TraceContext::new("trace-1");
        trace.project_id = Some("project-a".into());
        trace.build_id = Some("build-9".into());
        let mut event = ProjectEvent::new(
            "event-1",
            ProjectEventKind::BuildFailed,
            "control_center",
            "rustc failed E0425 token=secret-value",
            trace,
        );
        event.severity = EventSeverity::Error;
        event
            .evidence_paths
            .push("logs/sessions/build-9.log".into());
        let candidate = event_to_vector_candidate(&event).expect("candidate");
        assert!(candidate.is_embedding_worthy());
        assert!(!candidate.text.contains("secret-value"));
        assert_eq!(candidate.evidence_paths.len(), 1);
    }
    #[test]
    fn project_observability_uses_runtime_state_not_project_logs() {
        let nonce = format!("cortex-observability-{}-{}", std::process::id(), unix_ms());
        let base = std::env::temp_dir().join(nonce);
        let project_root = base.join("project");
        let state_root = base.join("state");
        fs::create_dir_all(&project_root).expect("project root");

        let obs = ProjectObservability::open(&project_root, &state_root).expect("observability");
        assert!(obs
            .event_path()
            .starts_with(state_root.join("observability").join("events")));
        assert!(obs
            .incident_path()
            .starts_with(state_root.join("observability").join("events")));
        assert!(obs
            .candidate_path()
            .starts_with(state_root.join("observability").join("events")));
        assert!(!project_root.join("logs").exists());

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn routine_stream_chunk_is_not_vectorized() {
        let event = ProjectEvent::new(
            "event-2",
            ProjectEventKind::ChatStreamChunk,
            "desktop_chat",
            "hello",
            TraceContext::new("trace-2"),
        );
        assert!(event_to_vector_candidate(&event).is_none());
    }
}
