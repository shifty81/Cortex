use forge_protocol::{ProtocolLaunchSpec, StdioProtocolKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::env;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const NATIVE_IDE_SCHEMA: &str = "forge.native_ide.v1";
pub const DEFAULT_MAX_DOCUMENT_BYTES: u64 = 8 * 1024 * 1024;
pub const DEFAULT_UNDO_DEPTH: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeIdeCapabilities {
    pub schema: String,
    pub native_rendering: bool,
    pub webview_required: bool,
    pub multi_tab: bool,
    pub undo_redo: bool,
    pub guarded_save: bool,
    pub workspace_search: bool,
    pub lsp_contract: bool,
    pub dap_contract: bool,
    pub terminal_contract: bool,
}

#[must_use]
pub fn native_ide_capabilities() -> NativeIdeCapabilities {
    NativeIdeCapabilities {
        schema: NATIVE_IDE_SCHEMA.to_owned(),
        native_rendering: true,
        webview_required: false,
        multi_tab: true,
        undo_redo: true,
        guarded_save: true,
        workspace_search: true,
        lsp_contract: true,
        dap_contract: true,
        terminal_contract: true,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdeDocument {
    pub relative_path: String,
    pub language: String,
    pub content: String,
    pub bytes: u64,
    pub sha256: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdeSearchHit {
    pub relative_path: String,
    pub line: usize,
    pub column: usize,
    pub preview: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LanguageToolKind {
    Lsp,
    Dap,
    Formatter,
    Linter,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LanguageToolConfig {
    pub id: String,
    pub language: String,
    pub kind: LanguageToolKind,
    pub program: String,
    pub args: Vec<String>,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalProfile {
    pub id: String,
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdePreferences {
    pub tab_width: usize,
    pub insert_spaces: bool,
    pub word_wrap: bool,
    pub show_line_numbers: bool,
    pub trim_trailing_whitespace_on_save: bool,
    pub ensure_final_newline_on_save: bool,
    pub max_document_bytes: u64,
    pub undo_depth: usize,
}

impl Default for IdePreferences {
    fn default() -> Self {
        Self {
            tab_width: 4,
            insert_spaces: true,
            word_wrap: false,
            show_line_numbers: true,
            trim_trailing_whitespace_on_save: false,
            ensure_final_newline_on_save: false,
            max_document_bytes: DEFAULT_MAX_DOCUMENT_BYTES,
            undo_depth: DEFAULT_UNDO_DEPTH,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdeTabSummary {
    pub relative_path: String,
    pub language: String,
    pub dirty: bool,
    pub conflict: bool,
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct EditorBuffer {
    document: IdeDocument,
    saved_sha256: String,
    dirty: bool,
    conflict: bool,
    undo: VecDeque<String>,
    redo: VecDeque<String>,
    undo_depth: usize,
}

impl EditorBuffer {
    fn new(document: IdeDocument, undo_depth: usize) -> Self {
        Self {
            saved_sha256: document.sha256.clone(),
            document,
            dirty: false,
            conflict: false,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            undo_depth: undo_depth.max(1),
        }
    }

    #[must_use]
    pub fn document(&self) -> &IdeDocument {
        &self.document
    }

    #[must_use]
    pub fn content(&self) -> &str {
        &self.document.content
    }

    pub fn content_mut(&mut self) -> &mut String {
        &mut self.document.content
    }

    #[must_use]
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    #[must_use]
    pub fn conflict(&self) -> bool {
        self.conflict
    }

    #[must_use]
    pub fn saved_sha256(&self) -> &str {
        &self.saved_sha256
    }

    pub fn replace_content(&mut self, new_content: String) {
        if self.document.content == new_content {
            return;
        }
        self.push_undo(self.document.content.clone());
        self.document.content = new_content;
        self.document.bytes = self.document.content.len() as u64;
        self.document.sha256 = sha256_bytes(self.document.content.as_bytes());
        self.dirty = self.document.sha256 != self.saved_sha256;
        self.redo.clear();
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop_back() else {
            return false;
        };
        self.redo.push_back(self.document.content.clone());
        self.document.content = previous;
        self.document.bytes = self.document.content.len() as u64;
        self.document.sha256 = sha256_bytes(self.document.content.as_bytes());
        self.dirty = self.document.sha256 != self.saved_sha256;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop_back() else {
            return false;
        };
        self.push_undo(self.document.content.clone());
        self.document.content = next;
        self.document.bytes = self.document.content.len() as u64;
        self.document.sha256 = sha256_bytes(self.document.content.as_bytes());
        self.dirty = self.document.sha256 != self.saved_sha256;
        true
    }

    fn mark_saved(&mut self, document: IdeDocument) {
        self.saved_sha256 = document.sha256.clone();
        self.document = document;
        self.dirty = false;
        self.conflict = false;
        self.redo.clear();
    }

    fn set_conflict(&mut self, value: bool) {
        self.conflict = value;
    }

    fn push_undo(&mut self, content: String) {
        if self.undo.len() >= self.undo_depth {
            self.undo.pop_front();
        }
        self.undo.push_back(content);
    }
}

#[derive(Debug, Clone)]
pub struct EditorSession {
    workspace: IdeWorkspace,
    preferences: IdePreferences,
    tabs: Vec<EditorBuffer>,
    active: Option<usize>,
}

impl EditorSession {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, String> {
        Ok(Self {
            workspace: IdeWorkspace::open(root)?,
            preferences: IdePreferences::default(),
            tabs: Vec::new(),
            active: None,
        })
    }

    #[must_use]
    pub fn workspace(&self) -> &IdeWorkspace {
        &self.workspace
    }

    #[must_use]
    pub fn preferences(&self) -> &IdePreferences {
        &self.preferences
    }

    pub fn set_preferences(&mut self, preferences: IdePreferences) {
        self.preferences = preferences;
        let depth = self.preferences.undo_depth.max(1);
        for buffer in &mut self.tabs {
            buffer.undo_depth = depth;
            while buffer.undo.len() > depth {
                buffer.undo.pop_front();
            }
        }
    }

    pub fn open_tab(&mut self, relative_path: &str) -> Result<usize, String> {
        let normalized = normalize_relative(relative_path)?;
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.document.relative_path == normalized)
        {
            self.active = Some(index);
            return Ok(index);
        }
        let document = self
            .workspace
            .open_document(&normalized, self.preferences.max_document_bytes)?;
        let index = self.tabs.len();
        self.tabs
            .push(EditorBuffer::new(document, self.preferences.undo_depth));
        self.active = Some(index);
        Ok(index)
    }

    pub fn activate(&mut self, index: usize) -> Result<(), String> {
        if index >= self.tabs.len() {
            return Err(format!("IDE tab index out of range: {index}"));
        }
        self.active = Some(index);
        Ok(())
    }

    pub fn close_tab(&mut self, index: usize, force: bool) -> Result<(), String> {
        let Some(tab) = self.tabs.get(index) else {
            return Err(format!("IDE tab index out of range: {index}"));
        };
        if tab.dirty && !force {
            return Err(format!(
                "IDE tab has unsaved changes: {}",
                tab.document.relative_path
            ));
        }
        self.tabs.remove(index);
        self.active = match self.active {
            None => None,
            Some(_) if self.tabs.is_empty() => None,
            Some(active) if active == index => Some(index.min(self.tabs.len() - 1)),
            Some(active) if active > index => Some(active - 1),
            Some(active) => Some(active),
        };
        Ok(())
    }

    #[must_use]
    pub fn active_index(&self) -> Option<usize> {
        self.active
    }

    #[must_use]
    pub fn active_buffer(&self) -> Option<&EditorBuffer> {
        self.active.and_then(|index| self.tabs.get(index))
    }

    pub fn active_buffer_mut(&mut self) -> Option<&mut EditorBuffer> {
        self.active.and_then(|index| self.tabs.get_mut(index))
    }

    #[must_use]
    pub fn tab_summaries(&self) -> Vec<IdeTabSummary> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| IdeTabSummary {
                relative_path: tab.document.relative_path.clone(),
                language: tab.document.language.clone(),
                dirty: tab.dirty,
                conflict: tab.conflict,
                active: self.active == Some(index),
            })
            .collect()
    }

    pub fn replace_active_content(&mut self, content: String) -> Result<(), String> {
        let buffer = self
            .active_buffer_mut()
            .ok_or_else(|| "no active IDE document".to_owned())?;
        buffer.replace_content(content);
        Ok(())
    }

    pub fn undo(&mut self) -> bool {
        self.active_buffer_mut().is_some_and(EditorBuffer::undo)
    }

    pub fn redo(&mut self) -> bool {
        self.active_buffer_mut().is_some_and(EditorBuffer::redo)
    }

    pub fn refresh_conflict(&mut self) -> Result<bool, String> {
        let Some(index) = self.active else {
            return Ok(false);
        };
        let path = self
            .workspace
            .resolve(&self.tabs[index].document.relative_path)?;
        let current = sha256_file(&path)?;
        let conflict = current != self.tabs[index].saved_sha256;
        self.tabs[index].set_conflict(conflict);
        Ok(conflict)
    }

    pub fn save_active(&mut self) -> Result<IdeDocument, String> {
        let index = self
            .active
            .ok_or_else(|| "no active IDE document".to_owned())?;
        let mut document = self.tabs[index].document.clone();
        apply_save_preferences(&mut document.content, &self.preferences);
        document.bytes = document.content.len() as u64;
        document.sha256 = sha256_bytes(document.content.as_bytes());
        let expected = self.tabs[index].saved_sha256.clone();
        let saved = self.workspace.save_document(&document, &expected)?;
        self.tabs[index].mark_saved(saved.clone());
        Ok(saved)
    }

    pub fn find_active(&self, query: &str, max_hits: usize) -> Vec<IdeSearchHit> {
        let Some(buffer) = self.active_buffer() else {
            return Vec::new();
        };
        if query.is_empty() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        for (line_index, line) in buffer.document.content.lines().enumerate() {
            for (column, _) in line.match_indices(query) {
                hits.push(IdeSearchHit {
                    relative_path: buffer.document.relative_path.clone(),
                    line: line_index + 1,
                    column: column + 1,
                    preview: line.chars().take(240).collect(),
                });
                if hits.len() >= max_hits {
                    return hits;
                }
            }
        }
        hits
    }
}

#[derive(Debug, Clone)]
pub struct IdeWorkspace {
    root: PathBuf,
}

impl IdeWorkspace {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = fs::canonicalize(root.as_ref()).map_err(|error| error.to_string())?;
        if !root.is_dir() {
            return Err("IDE workspace root is not a directory".to_owned());
        }
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn open_document(&self, relative: &str, max_bytes: u64) -> Result<IdeDocument, String> {
        let path = self.resolve(relative)?;
        let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err(format!("IDE path is not a file: {relative}"));
        }
        if metadata.len() > max_bytes {
            return Err(format!(
                "IDE file exceeds configured read budget: {relative}"
            ));
        }
        let bytes = fs::read(&path).map_err(|error| error.to_string())?;
        let digest = sha256_bytes(&bytes);
        let content = String::from_utf8(bytes)
            .map_err(|_| format!("IDE only opens UTF-8 text documents: {relative}"))?;
        Ok(IdeDocument {
            relative_path: normalize_relative(relative)?,
            language: language_for(&path),
            content,
            bytes: metadata.len(),
            sha256: digest,
            read_only: false,
        })
    }

    pub fn save_document(
        &self,
        document: &IdeDocument,
        expected_sha256: &str,
    ) -> Result<IdeDocument, String> {
        if document.read_only {
            return Err(format!(
                "IDE document is read-only: {}",
                document.relative_path
            ));
        }
        let path = self.resolve(&document.relative_path)?;
        if !path.is_file() {
            return Err("IDE save requires an existing file; creation uses project tooling".to_owned());
        }
        let current = sha256_file(&path)?;
        if current != expected_sha256 {
            return Err(format!(
                "IDE preimage changed on disk: {}",
                document.relative_path
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| "IDE path has no parent".to_owned())?;
        let temp = parent.join(format!(".forge-ide-{}.tmp", unix_ms()));
        let backup = parent.join(format!(".forge-ide-{}.bak", unix_ms()));
        {
            let mut file = File::create(&temp).map_err(|error| error.to_string())?;
            file.write_all(document.content.as_bytes())
                .map_err(|error| error.to_string())?;
            file.flush().map_err(|error| error.to_string())?;
            let _ = file.sync_all();
        }
        if sha256_file(&temp)? != sha256_bytes(document.content.as_bytes()) {
            let _ = fs::remove_file(&temp);
            return Err("IDE temp-file verification failed before save".to_owned());
        }
        fs::rename(&path, &backup).map_err(|error| {
            let _ = fs::remove_file(&temp);
            format!("failed to stage IDE save backup: {error}")
        })?;
        match fs::rename(&temp, &path) {
            Ok(()) => {
                let _ = fs::remove_file(&backup);
            }
            Err(error) => {
                let _ = fs::rename(&backup, &path);
                let _ = fs::remove_file(&temp);
                return Err(format!("failed to publish IDE save; backup restored: {error}"));
            }
        }
        self.open_document(&document.relative_path, u64::MAX)
    }

    pub fn search(
        &self,
        query: &str,
        max_files: usize,
        max_hits: usize,
    ) -> Result<Vec<IdeSearchHit>, String> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let mut files_seen = 0usize;
        let mut hits = Vec::new();
        self.search_dir(
            &self.root,
            query,
            max_files,
            max_hits,
            &mut files_seen,
            &mut hits,
        )?;
        Ok(hits)
    }

    pub fn list_text_files(&self, max_files: usize) -> Result<Vec<String>, String> {
        let mut files = Vec::new();
        self.list_text_files_in(&self.root, max_files, &mut files)?;
        files.sort();
        Ok(files)
    }

    pub fn language_tools(&self, language: &str) -> Vec<LanguageToolConfig> {
        suggested_language_tools(language)
            .into_iter()
            .map(|mut config| {
                config.available = program_on_path(&config.program);
                config
            })
            .collect()
    }

    pub fn protocol_specs(&self, language: &str) -> Vec<ProtocolLaunchSpec> {
        self.language_tools(language)
            .into_iter()
            .filter_map(|tool| {
                let kind = match tool.kind {
                    LanguageToolKind::Lsp => StdioProtocolKind::Lsp,
                    LanguageToolKind::Dap => StdioProtocolKind::Dap,
                    LanguageToolKind::Formatter | LanguageToolKind::Linter => return None,
                };
                Some(ProtocolLaunchSpec {
                    id: tool.id,
                    kind,
                    program: PathBuf::from(tool.program),
                    args: tool.args,
                    cwd: self.root.clone(),
                })
            })
            .collect()
    }

    pub fn terminal_profiles(&self) -> Vec<TerminalProfile> {
        #[cfg(windows)]
        let candidates = vec![
            ("pwsh", "PowerShell", "pwsh", Vec::<String>::new()),
            ("powershell", "Windows PowerShell", "powershell", Vec::<String>::new()),
            ("cmd", "Command Prompt", "cmd", Vec::<String>::new()),
        ];
        #[cfg(not(windows))]
        let candidates = vec![
            ("bash", "Bash", "bash", Vec::<String>::new()),
            ("sh", "Shell", "sh", Vec::<String>::new()),
        ];
        candidates
            .into_iter()
            .map(|(id, label, program, args)| TerminalProfile {
                id: id.to_owned(),
                label: label.to_owned(),
                program: program.to_owned(),
                args,
                available: program_on_path(program),
            })
            .collect()
    }

    fn list_text_files_in(
        &self,
        directory: &Path,
        max_files: usize,
        files: &mut Vec<String>,
    ) -> Result<(), String> {
        if files.len() >= max_files {
            return Ok(());
        }
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(_) => return Ok(()),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if ignored_directory(&path) {
                    continue;
                }
                self.list_text_files_in(&path, max_files, files)?;
            } else if path.is_file() && is_probably_text_file(&path) {
                if let Ok(relative) = path.strip_prefix(&self.root) {
                    files.push(relative.to_string_lossy().replace('\\', "/"));
                }
                if files.len() >= max_files {
                    break;
                }
            }
        }
        Ok(())
    }

    fn search_dir(
        &self,
        directory: &Path,
        query: &str,
        max_files: usize,
        max_hits: usize,
        files_seen: &mut usize,
        hits: &mut Vec<IdeSearchHit>,
    ) -> Result<(), String> {
        if *files_seen >= max_files || hits.len() >= max_hits {
            return Ok(());
        }
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(_) => return Ok(()),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if ignored_directory(&path) {
                    continue;
                }
                self.search_dir(&path, query, max_files, max_hits, files_seen, hits)?;
            } else if path.is_file() {
                *files_seen += 1;
                if *files_seen > max_files {
                    break;
                }
                let Ok(content) = fs::read_to_string(&path) else {
                    continue;
                };
                for (line_index, line) in content.lines().enumerate() {
                    for (column, _) in line.match_indices(query) {
                        let relative = path
                            .strip_prefix(&self.root)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .replace('\\', "/");
                        hits.push(IdeSearchHit {
                            relative_path: relative,
                            line: line_index + 1,
                            column: column + 1,
                            preview: line.chars().take(240).collect(),
                        });
                        if hits.len() >= max_hits {
                            return Ok(());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn resolve(&self, relative: &str) -> Result<PathBuf, String> {
        let normalized = normalize_relative(relative)?;
        let path = self.root.join(normalized.split('/').collect::<PathBuf>());
        if let Ok(canonical) = fs::canonicalize(&path) {
            if !canonical.starts_with(&self.root) {
                return Err("IDE path escapes workspace root".to_owned());
            }
            Ok(canonical)
        } else {
            Ok(path)
        }
    }
}

fn apply_save_preferences(content: &mut String, preferences: &IdePreferences) {
    if preferences.trim_trailing_whitespace_on_save {
        let mut rebuilt = String::with_capacity(content.len());
        for (index, line) in content.lines().enumerate() {
            if index > 0 {
                rebuilt.push('\n');
            }
            rebuilt.push_str(line.trim_end());
        }
        if content.ends_with('\n') {
            rebuilt.push('\n');
        }
        *content = rebuilt;
    }
    if preferences.ensure_final_newline_on_save && !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
}

fn normalize_relative(value: &str) -> Result<String, String> {
    let value = value.replace('\\', "/");
    let path = Path::new(&value);
    if value.trim().is_empty() || path.is_absolute() {
        return Err("IDE path must be relative".to_owned());
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err("IDE path traversal is not allowed".to_owned());
        }
    }
    Ok(value)
}

fn ignored_directory(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|value| value.to_str()).unwrap_or_default(),
        ".git"
            | "target"
            | "node_modules"
            | "artifacts"
            | ".project_control"
            | ".cortex"
            | ".venv"
            | "venv"
            | "build"
            | "dist"
    )
}

fn is_probably_text_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()).unwrap_or_default(),
        "rs"
            | "py"
            | "json"
            | "toml"
            | "md"
            | "txt"
            | "cpp"
            | "cc"
            | "cxx"
            | "c"
            | "h"
            | "hpp"
            | "ps1"
            | "cmd"
            | "bat"
            | "js"
            | "ts"
            | "tsx"
            | "jsx"
            | "html"
            | "css"
            | "xml"
            | "yaml"
            | "yml"
            | "ron"
    )
}

fn language_for(path: &Path) -> String {
    match path.extension().and_then(|value| value.to_str()).unwrap_or_default() {
        "rs" => "rust",
        "py" => "python",
        "json" => "json",
        "toml" => "toml",
        "md" => "markdown",
        "cpp" | "cc" | "cxx" | "c" => "cpp",
        "h" | "hpp" => "cpp",
        "ps1" => "powershell",
        "js" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        "html" => "html",
        "css" => "css",
        "yaml" | "yml" => "yaml",
        "ron" => "ron",
        _ => "text",
    }
    .to_owned()
}

fn suggested_language_tools(language: &str) -> Vec<LanguageToolConfig> {
    let mut out = Vec::new();
    let mut push = |id: &str, kind: LanguageToolKind, program: &str, args: &[&str]| {
        out.push(LanguageToolConfig {
            id: id.to_owned(),
            language: language.to_owned(),
            kind,
            program: program.to_owned(),
            args: args.iter().map(|value| (*value).to_owned()).collect(),
            available: false,
        });
    };
    match language {
        "rust" => {
            push("rust-analyzer", LanguageToolKind::Lsp, "rust-analyzer", &[]);
            push("rustfmt", LanguageToolKind::Formatter, "rustfmt", &[]);
        }
        "python" => {
            push("pyright", LanguageToolKind::Lsp, "pyright-langserver", &["--stdio"]);
            push("ruff", LanguageToolKind::Linter, "ruff", &["check", "-"]);
        }
        "cpp" => {
            push("clangd", LanguageToolKind::Lsp, "clangd", &[]);
            push("clang-format", LanguageToolKind::Formatter, "clang-format", &[]);
        }
        "typescript" | "javascript" => {
            push(
                "typescript-language-server",
                LanguageToolKind::Lsp,
                "typescript-language-server",
                &["--stdio"],
            );
        }
        _ => {}
    }
    out
}

fn program_on_path(program: &str) -> bool {
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return program_path.is_file();
    }
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    #[cfg(windows)]
    let extensions: Vec<String> = env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
        .split(';')
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .collect();
    for root in env::split_paths(&path) {
        let direct = root.join(program);
        if direct.is_file() {
            return true;
        }
        #[cfg(windows)]
        for extension in &extensions {
            if root.join(format!("{program}{extension}")).is_file() {
                return true;
            }
        }
    }
    false
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok(hex(&digest))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex(&digest)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
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
        let root = env::temp_dir().join(format!("forge-ide-{name}-{}", unix_ms()));
        fs::create_dir_all(&root).expect("test root");
        root
    }

    #[test]
    fn blocks_path_traversal() {
        assert!(normalize_relative("src/lib.rs").is_ok());
        assert!(normalize_relative("../secret").is_err());
    }

    #[test]
    fn native_capabilities_do_not_require_webview() {
        let capabilities = native_ide_capabilities();
        assert!(capabilities.native_rendering);
        assert!(!capabilities.webview_required);
    }

    #[test]
    fn editor_session_tracks_dirty_undo_and_redo() {
        let root = test_root("history");
        fs::write(root.join("demo.rs"), "fn main() {}\n").expect("source");
        let mut session = EditorSession::open(&root).expect("session");
        session.open_tab("demo.rs").expect("open");
        session
            .replace_active_content("fn main() { println!(\"hi\"); }\n".to_owned())
            .expect("edit");
        assert!(session.active_buffer().expect("buffer").dirty());
        assert!(session.undo());
        assert!(!session.active_buffer().expect("buffer").dirty());
        assert!(session.redo());
        assert!(session.active_buffer().expect("buffer").dirty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn guarded_save_detects_external_change() {
        let root = test_root("conflict");
        fs::write(root.join("demo.txt"), "one\n").expect("source");
        let mut session = EditorSession::open(&root).expect("session");
        session.open_tab("demo.txt").expect("open");
        session
            .replace_active_content("two\n".to_owned())
            .expect("edit");
        fs::write(root.join("demo.txt"), "external\n").expect("external");
        assert!(session.refresh_conflict().expect("refresh"));
        assert!(session.save_active().is_err());
        let _ = fs::remove_dir_all(root);
    }
}
