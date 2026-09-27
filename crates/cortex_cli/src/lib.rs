use cortex_adapter_git::GitAdapter;
use cortex_context::ContextStore;
use cortex_conversation::{ConversationRole, ConversationStore};
use cortex_core::{AgentEngine, AgentMode, AgentSettings, AgentToolProfile, AgentTurnResult};
use cortex_desktop_core::{DesktopBootstrap, DesktopController};
use cortex_jobs::{ActivityStore, EventKind, EventStore, JobStatus, JobStore, TaskStore};
use cortex_model_host::{mirror_lmstudio_models, mirror_model_tree};
use cortex_observability::{ProjectObservability, VectorMemoryDatabase};
use cortex_permissions::{parse_permission, PermissionPolicy};
use cortex_plugin::PluginRegistry;
use cortex_process::ProcessService;
use cortex_project::ProjectContract;
use cortex_protocol::{
    CortexStreamEvent, CortexStreamKind, EmbeddingProvider, ImageProvider, MessageRole,
    ModelMessage, ModelRequest, RpcRequest, RpcResponse, TextProvider, ToolCall, ToolChoicePolicy,
    ToolDefinition, ToolExecutor, VisionProvider, CORTEX_DESKTOP_API_REVISION,
};
use cortex_provider_comfyui::ComfyUiProvider;
use cortex_provider_router::{CortexProvider, ModelCapability, ProviderKind};
use cortex_registry::WorkspaceRegistry;
use cortex_review::ReviewSnapshot;
use cortex_rpc::{default_address, RpcHandler, RpcServer};
use cortex_service::{process_is_alive, ServiceRegistry};
use cortex_settings::CortexSettings;
use cortex_tools::{ToolBroker, ToolExtension};
use cortex_vault::Vault;
use cortex_web::WebToolExtension;
use cortex_workspace::Workspace;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type CortexEngine = AgentEngine<CortexProvider, ToolBroker>;

pub fn main_entry() {
    if let Err(error) = run() {
        eprintln!("Cortex failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let raw_args: Vec<String> = env::args().skip(1).collect();
    let (workspace_override, args) = extract_global_workspace(raw_args)?;
    let project_root = find_workspace_root(
        env::current_dir().map_err(|error| error.to_string())?,
        workspace_override.as_deref(),
    )?;
    let workspace = open_registered_workspace(&project_root)?;
    let command = args.first().map(String::as_str).unwrap_or("status");
    if matches!(
        command,
        "conversation"
            | "conversations"
            | "git"
            | "plugin"
            | "plugins"
            | "permissions"
            | "vault"
            | "observability"
            | "workspace"
            | "project"
            | "desktop"
            | "settings"
            | "review"
            | "activity"
            | "tasks"
    ) {
        return run_local_command(&workspace, &args);
    }
    if command == "service" && args.get(1).map(String::as_str) != Some("run") {
        return service_lifecycle_command(&workspace, &args[1..]);
    }
    let provider_kind = configured_provider_kind();
    let provider_url = configured_provider_url(provider_kind);
    let provider_timeout = configured_provider_timeout();
    let provider_probe =
        configured_provider(provider_kind, provider_url.clone(), None, provider_timeout);
    let model_role = model_role_for_command(&args);
    let runtime_model = resolve_runtime_model_for_role(&project_root, model_role, &provider_probe);
    let provider =
        configured_provider(provider_kind, provider_url, runtime_model, provider_timeout);

    let vision_provider: Option<Box<dyn VisionProvider>> = Some(Box::new(provider.clone()));
    let embedding_model =
        resolve_runtime_model_for_role(&project_root, ModelRole::Embedding, &provider_probe);
    let embedding_provider: Option<Box<dyn EmbeddingProvider>> = embedding_model.map(|model| {
        Box::new(configured_provider(
            provider_kind,
            provider_probe.base_url().to_string(),
            Some(model),
            provider_timeout,
        )) as Box<dyn EmbeddingProvider>
    });
    let image_provider: Option<Box<dyn ImageProvider>> =
        match env_compat("CORTEX_COMFYUI_WORKFLOW", "OPEN2D_COMFYUI_WORKFLOW") {
            Some(workflow) if !workflow.trim().is_empty() => {
                let url = env_compat("CORTEX_COMFYUI_URL", "OPEN2D_COMFYUI_URL")
                    .unwrap_or_else(|| "http://127.0.0.1:8188".into());
                Some(Box::new(ComfyUiProvider::new(url, workflow)))
            }
            _ => None,
        };

    // Project/domain extensions are discovered through the plugin/provider boundary.
    // Standalone Cortex must not compile an Open2D adapter into the generic CLI.
    let tool_extensions: Vec<Box<dyn ToolExtension>> = vec![Box::new(WebToolExtension::from_env())];
    let tools = ToolBroker::new_with_extensions(
        &project_root,
        image_provider,
        vision_provider,
        embedding_provider,
        tool_extensions,
    )?;
    let mode = agent_mode_from_env();
    let settings = AgentSettings {
        mode,
        max_tool_iterations: configured_agent_iterations(),
        max_elapsed_seconds: configured_agent_deadline_seconds(),
        max_tool_result_bytes: configured_agent_tool_result_bytes(),
        ..AgentSettings::default()
    };
    let engine = AgentEngine::new(provider, tools, settings);
    let rpc_token = ensure_rpc_token(&project_root)?;
    let mut service = CortexService {
        engine,
        rpc_token,
        workspace_root: workspace.root().to_path_buf(),
    };

    match args.first().map(String::as_str).unwrap_or("status") {
        "status" => {
            let call = ToolCall {
                call_id: "status".into(),
                name: "workspace.status".into(),
                arguments: json!({}),
            };
            let result = service.engine.tools_mut().execute(&call);
            println!(
                "{}",
                serde_json::to_string_pretty(&result.output).map_err(|e| e.to_string())?
            );
            eprintln!("Cortex provider: {}", provider_kind.as_str());
            eprintln!(
                "Configured model: {}",
                configured_model_description(&project_root, model_role)
            );
            match provider_timeout {
                Some(timeout) => eprintln!(
                    "Inference timeout: {} seconds (explicit watchdog)",
                    timeout.as_secs()
                ),
                None => eprintln!(
                    "Inference timeout: disabled (local inference may run until completion)"
                ),
            }
            let agent_deadline = configured_agent_deadline_seconds();
            if agent_deadline == 0 {
                eprintln!("Project-agent deadline: disabled");
            } else {
                eprintln!("Project-agent deadline: {agent_deadline} seconds (explicit watchdog)");
            }
            eprintln!(
                "Project-agent max iterations: {}",
                configured_agent_iterations()
            );
            eprintln!(
                "Tool-result context cap: {} bytes",
                configured_agent_tool_result_bytes()
            );
        }
        "models" => match args.get(1).map(String::as_str) {
            Some("import-lmstudio") | Some("mirror-lmstudio") => {
                let registry = WorkspaceRegistry::open_default()?;
                let layout = registry
                    .library_layout()?
                    .ok_or_else(|| "Cortex Vault is not configured".to_string())?;
                let destination = layout.models_imported.join("LMStudio");
                let report = mirror_lmstudio_models(&destination)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
                );
            }
            Some("import-comfyui") | Some("mirror-comfyui") => {
                let source = args.get(2).ok_or_else(|| {
                    "usage: cortex models import-comfyui <ComfyUI-root-or-models-directory>"
                        .to_string()
                })?;
                let registry = WorkspaceRegistry::open_default()?;
                let layout = registry
                    .library_layout()?
                    .ok_or_else(|| "Cortex Vault is not configured".to_string())?;
                let supplied = PathBuf::from(source);
                let model_root = if supplied.join("models").is_dir() {
                    supplied.join("models")
                } else {
                    supplied
                };
                if !model_root.is_dir() {
                    return Err(format!(
                        "ComfyUI model directory does not exist: {}",
                        model_root.display()
                    ));
                }
                let destination = layout.image_imported.join("ComfyUI");
                let report = mirror_model_tree("comfyui", &model_root, &destination)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
                );
            }
            _ => {
                let selected = configured_lm_model_for_role(&project_root, model_role);
                let capabilities = service
                    .engine
                    .provider()
                    .model_capabilities()
                    .unwrap_or_default();
                for model in service
                    .engine
                    .provider()
                    .list_models()
                    .map_err(|e| e.to_string())?
                {
                    let marker = if selected.as_deref() == Some(model.as_str()) {
                        "*"
                    } else {
                        " "
                    };
                    let capability = capabilities.iter().find(|info| {
                        info.key == model
                            || info
                                .loaded_instance_ids
                                .iter()
                                .any(|loaded| loaded == &model)
                    });
                    let tool = match capability.and_then(|info| info.trained_for_tool_use) {
                        Some(true) => "tool=yes",
                        Some(false) => "tool=no",
                        None => "tool=?",
                    };
                    let vision = match capability.and_then(|info| info.vision) {
                        Some(true) => "vision=yes",
                        Some(false) => "vision=no",
                        None => "vision=?",
                    };
                    println!("{marker} {model}  [{tool}, {vision}]");
                }
                if selected.is_none() {
                    println!();
                    println!("No explicit model selected; Cortex will choose the first non-embedding text model.");
                }
            }
        },
        "model" => {
            model_command(&project_root, service.engine.provider(), &args[1..])?;
        }
        "doctor" => {
            doctor_command(&project_root, &mut service, &args[1..])?;
        }
        "tools" => {
            tools_command(&mut service, &args[1..])?;
        }
        "tx" => {
            transaction_command(&mut service, &args[1..])?;
        }
        "build" => {
            build_command(&mut service, &args[1..])?;
        }
        "session" => {
            session_command(&project_root, &args[1..])?;
        }
        "logs" => {
            logs_command(&project_root, &args[1..])?;
        }
        "certify" => {
            certify_command(&project_root, &mut service, &args[1..])?;
        }
        "help" | "--help" | "-h" => {
            print_cli_help();
        }
        "chat" | "ask" => {
            let prompt = prompt_from_args(&args[1..]);
            if prompt.trim().is_empty() {
                return Err("usage: cortex chat <prompt> [--json|--jsonl]".into());
            }
            if args.first().map(String::as_str) == Some("ask") {
                eprintln!(
                    "Compatibility: `ask` now uses general chat mode. Use `inspect` for grounded project facts."
                );
            }

            let model = service
                .engine
                .provider()
                .resolve_model()
                .map_err(|e| e.to_string())?;
            let output = output_mode(&args);

            if output == OutputMode::Human {
                print_request_header(
                    "CHAT",
                    &model,
                    mode,
                    provider_timeout,
                    None,
                    "NONE (no project tools)",
                );
            }
            emit_event(
                output,
                "request_started",
                json!({
                    "kind":"chat",
                    "provider":provider_kind.as_str(),
                    "model":model,
                    "grounding":"none"
                }),
            );
            let conversation_id = argument_value(&args[1..], "--conversation");
            let conversation_store =
                ConversationStore::open(workspace.cortex_state_dir(), workspace.root())?;
            if let Some(id) = conversation_id.as_deref() {
                conversation_store.append(id, ConversationRole::User, prompt.clone())?;
            }
            let result = run_with_progress("chat", provider_timeout, output, || {
                service.engine.chat(&prompt)
            })?;
            if let Some(id) = conversation_id.as_deref() {
                conversation_store.append(id, ConversationRole::Assistant, result.text.clone())?;
            }
            match output {
                OutputMode::Human => {
                    println!("{}", result.text);
                    eprintln!("Grounding : N/A - chat mode");
                    eprintln!("Iterations: {}", result.iterations);
                }
                OutputMode::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "schema_version":1,
                        "mode":"chat",
                        "grounding":"none",
                        "model":model,
                        "text":result.text,
                        "iterations":result.iterations
                    }))
                    .map_err(|e| e.to_string())?
                ),
                OutputMode::Jsonl => emit_event(
                    output,
                    "agent_finished",
                    json!({
                        "mode":"chat",
                        "grounding":"none",
                        "model":model,
                        "text":result.text,
                        "iterations":result.iterations
                    }),
                ),
            }
        }
        "inspect" | "plan" | "apply" | "repair" => {
            let action = args.first().map(String::as_str).unwrap_or("inspect");
            let prompt = prompt_from_args(&args[1..]);
            if prompt.trim().is_empty() {
                return Err(format!("usage: cortex {action} <prompt> [--json|--jsonl]"));
            }
            project_agent_command(
                &mut service,
                action,
                &prompt,
                provider_timeout,
                output_mode(&args),
                args.iter().any(|arg| arg == "--reuse-transaction"),
            )?;
        }
        "tool" => {
            let name = args
                .get(1)
                .ok_or_else(|| "usage: cortex tool <name> [json-args]".to_string())?;
            let arguments = args
                .get(2)
                .map(|text| serde_json::from_str::<Value>(text).map_err(|e| e.to_string()))
                .transpose()?
                .unwrap_or_else(|| json!({}));
            let call = ToolCall {
                call_id: "cli-tool".into(),
                name: name.clone(),
                arguments,
            };
            let result = service.engine.tools_mut().execute(&call);
            println!(
                "{}",
                serde_json::to_string_pretty(&result.output).map_err(|e| e.to_string())?
            );
            if result.is_error {
                std::process::exit(2);
            }
        }
        "serve" => {
            serve_rpc(&project_root, &mut service, parse_port(&args[1..], 7337)?)?;
        }
        "service" if args.get(1).map(String::as_str) == Some("run") => {
            let port = parse_port(&args[2..], 7337)?;
            let registry = ServiceRegistry::new(workspace.cortex_state_dir(), workspace.root())?;
            let owner_pid = desktop_owner_pid();
            if let Some(pid) = owner_pid {
                if !process_is_alive(pid) {
                    return Err(format!("Cortex Desktop owner PID {pid} is not running"));
                }
            }

            // Bind the RPC listener before publishing service.json. Desktop discovery
            // treats a Running service record as usable, so publishing before bind
            // creates a race where the first follow-up request can receive 10061.
            let server = RpcServer::bind(default_address(port))?;
            let _claim = registry.claim_current_process(port)?;
            serve_bound_rpc(&project_root, &mut service, &server, owner_pid)?;
        }
        other => {
            return Err(format!(
                "unknown Cortex command: {other}; run `cortex help`"
            ))
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputMode {
    Human,
    Json,
    Jsonl,
}

fn output_mode(args: &[String]) -> OutputMode {
    if args.iter().any(|arg| arg == "--jsonl") {
        OutputMode::Jsonl
    } else if args.iter().any(|arg| arg == "--json") {
        OutputMode::Json
    } else {
        OutputMode::Human
    }
}

fn prompt_from_args(args: &[String]) -> String {
    let mut prompt = Vec::new();
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--json" | "--jsonl" | "--reuse-transaction" => {
                index += 1;
            }
            "--conversation" => {
                index += 2;
            }
            _ => {
                prompt.push(args[index].clone());
                index += 1;
            }
        }
    }
    prompt.join(" ")
}

fn argument_value(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|argument| argument == key)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn emit_event(mode: OutputMode, event: &str, data: Value) {
    if mode == OutputMode::Jsonl {
        println!("{}", json!({"event":event,"data":data}));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ModelRole {
    Default,
    Chat,
    Tool,
    Vision,
    Embedding,
}

impl ModelRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Chat => "chat",
            Self::Tool => "tool",
            Self::Vision => "vision",
            Self::Embedding => "embedding",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "default" => Some(Self::Default),
            "chat" => Some(Self::Chat),
            "tool" | "tools" | "coding" => Some(Self::Tool),
            "vision" => Some(Self::Vision),
            "embedding" | "embeddings" => Some(Self::Embedding),
            _ => None,
        }
    }

    fn env_key(self) -> &'static str {
        match self {
            Self::Default => "CORTEX_MODEL",
            Self::Chat => "CORTEX_MODEL_CHAT",
            Self::Tool => "CORTEX_MODEL_TOOL",
            Self::Vision => "CORTEX_MODEL_VISION",
            Self::Embedding => "CORTEX_MODEL_EMBEDDING",
        }
    }

    fn legacy_env_key(self) -> &'static str {
        match self {
            Self::Default => "CORTEX_LMSTUDIO_MODEL",
            Self::Chat => "CORTEX_LMSTUDIO_MODEL_CHAT",
            Self::Tool => "CORTEX_LMSTUDIO_MODEL_TOOL",
            Self::Vision => "CORTEX_LMSTUDIO_MODEL_VISION",
            Self::Embedding => "CORTEX_LMSTUDIO_MODEL_EMBEDDING",
        }
    }

    fn open2d_legacy_env_key(self) -> &'static str {
        match self {
            Self::Default => "OPEN2D_LMSTUDIO_MODEL",
            Self::Chat => "OPEN2D_LMSTUDIO_MODEL_CHAT",
            Self::Tool => "OPEN2D_LMSTUDIO_MODEL_TOOL",
            Self::Vision => "OPEN2D_LMSTUDIO_MODEL_VISION",
            Self::Embedding => "OPEN2D_LMSTUDIO_MODEL_EMBEDDING",
        }
    }

    fn all() -> [Self; 5] {
        [
            Self::Default,
            Self::Chat,
            Self::Tool,
            Self::Vision,
            Self::Embedding,
        ]
    }
}

fn model_role_for_command(args: &[String]) -> ModelRole {
    match args.first().map(String::as_str).unwrap_or("status") {
        "chat" | "ask" => ModelRole::Chat,
        "inspect" | "plan" | "apply" | "repair" | "doctor" | "tools" | "tx" | "build"
        | "certify" | "serve" | "service" => ModelRole::Tool,
        "tool"
            if args
                .get(1)
                .map(|name| name == "vision.inspect")
                .unwrap_or(false) =>
        {
            ModelRole::Vision
        }
        "tool" => ModelRole::Tool,
        _ => ModelRole::Default,
    }
}

fn model_command(
    project_root: &Path,
    provider: &CortexProvider,
    args: &[String],
) -> Result<(), String> {
    match args.first().map(String::as_str).unwrap_or("current") {
        "current" => {
            let role = args
                .get(1)
                .and_then(|value| ModelRole::parse(value))
                .unwrap_or(ModelRole::Default);
            println!("{}", configured_model_description(project_root, role));
        }
        "roles" => {
            for role in ModelRole::all() {
                println!(
                    "{:<10} {}",
                    role.as_str(),
                    configured_model_description(project_root, role)
                );
            }
        }
        "set" => {
            let (role, model_index) = match args.get(1).and_then(|value| ModelRole::parse(value)) {
                Some(role) => (role, 2),
                None => (ModelRole::Default, 1),
            };
            let model = args.get(model_index).ok_or_else(|| {
                "usage: cortex model set [default|chat|tool|vision|embedding] <model-id>"
                    .to_string()
            })?;
            let models = provider.list_models().map_err(|e| e.to_string())?;
            if !models.iter().any(|candidate| candidate == model) {
                return Err(format!(
                    "Configured Cortex provider model is not currently visible: {model}\nAvailable models:\n  {}",
                    models.join("\n  ")
                ));
            }
            if role == ModelRole::Tool {
                let capabilities = provider.model_capabilities().unwrap_or_default();
                if capability_for_visible_model(&capabilities, model)
                    .and_then(|info| info.trained_for_tool_use)
                    == Some(false)
                {
                    let recommended = models.iter().find(|candidate| {
                        capability_for_visible_model(&capabilities, candidate)
                            .and_then(|info| info.trained_for_tool_use)
                            == Some(true)
                    });
                    return Err(format!(
                        "Cortex provider model '{model}' is not marked as trained for native tool use. Cortex Inspect/Plan/Apply/Repair require structured tool calls.{}",
                        recommended
                            .map(|candidate| format!(" Try: cortex model set tool {candidate}"))
                            .unwrap_or_else(|| " Select a provider model that supports structured tool calls.".into())
                    ));
                }
            }
            let path = model_config_path(project_root, role);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&path, model.as_bytes()).map_err(|e| e.to_string())?;
            println!("Selected Cortex {} model: {model}", role.as_str());
            println!("Stored at: {}", path.display());
        }
        "clear" => {
            let role = args
                .get(1)
                .and_then(|value| ModelRole::parse(value))
                .unwrap_or(ModelRole::Default);
            let path = model_config_path(project_root, role);
            if path.exists() {
                fs::remove_file(&path).map_err(|e| e.to_string())?;
            }
            println!("Cleared project Cortex {} model selection.", role.as_str());
        }
        other => {
            return Err(format!(
                "unknown model command: {other}; use current [role], roles, set [role] <model-id>, or clear [role]"
            ));
        }
    }
    Ok(())
}

fn configured_lm_model_for_role(project_root: &Path, role: ModelRole) -> Option<String> {
    configured_role_value(project_root, role).or_else(|| {
        if role == ModelRole::Default {
            None
        } else {
            configured_role_value(project_root, ModelRole::Default)
        }
    })
}

fn resolve_runtime_model_for_role(
    project_root: &Path,
    role: ModelRole,
    provider: &CortexProvider,
) -> Option<String> {
    let explicit = configured_role_value(project_root, role);
    let default_explicit = configured_role_value(project_root, ModelRole::Default);
    let visible = provider.list_models().unwrap_or_default();
    let capabilities = provider.model_capabilities().unwrap_or_default();
    let is_visible = |model: &str| visible.iter().any(|candidate| candidate == model);
    let is_text = |model: &&String| {
        let lower = model.to_ascii_lowercase();
        !lower.contains("embed") && !lower.contains("embedding") && !lower.contains("rerank")
    };

    // Provider changes must not strand an old provider-specific model ID in settings.
    // Honor explicit role/default selections only while they remain visible to the
    // currently selected provider; otherwise route to a compatible discovered model.
    if role == ModelRole::Chat {
        return explicit
            .filter(|model| is_visible(model))
            .or_else(|| default_explicit.filter(|model| is_visible(model)))
            .or_else(|| visible.iter().find(is_text).cloned());
    }

    if role == ModelRole::Tool {
        if let Some(model) = explicit.as_ref().filter(|model| is_visible(model)) {
            let trained = capability_for_visible_model(&capabilities, model)
                .and_then(|info| info.trained_for_tool_use);
            if trained != Some(false) {
                return Some(model.to_string());
            }
        }

        if let Some(model) = visible.iter().find(|model| {
            capability_for_visible_model(&capabilities, model)
                .map(|info| {
                    info.trained_for_tool_use == Some(true) && !info.loaded_instance_ids.is_empty()
                })
                .unwrap_or(false)
        }) {
            return Some(model.to_string());
        }

        if let Some(model) = visible.iter().find(|model| {
            capability_for_visible_model(&capabilities, model)
                .and_then(|info| info.trained_for_tool_use)
                == Some(true)
        }) {
            return Some(model.to_string());
        }

        // Native llama.cpp templates may support structured tool calls even when
        // model metadata does not explicitly advertise the capability. Unknown is
        // permitted; only an explicit tool=no classification is rejected later.
        if let Some(model) = visible.iter().filter(is_text).find(|model| {
            capability_for_visible_model(&capabilities, model)
                .and_then(|info| info.trained_for_tool_use)
                != Some(false)
        }) {
            return Some(model.to_string());
        }

        return default_explicit
            .filter(|model| is_visible(model))
            .or_else(|| visible.iter().find(is_text).cloned());
    }

    if let Some(model) = explicit.filter(|model| is_visible(model)) {
        return Some(model);
    }

    match role {
        ModelRole::Vision => visible
            .iter()
            .find(|model| {
                capability_for_visible_model(&capabilities, model).and_then(|info| info.vision)
                    == Some(true)
            })
            .cloned()
            .or_else(|| default_explicit.filter(|model| is_visible(model)))
            .or_else(|| visible.iter().find(is_text).cloned()),
        ModelRole::Embedding => visible
            .iter()
            .find(|model| {
                let lower = model.to_ascii_lowercase();
                lower.contains("embed") || lower.contains("embedding")
            })
            .cloned(),
        ModelRole::Default => default_explicit
            .filter(|model| is_visible(model))
            .or_else(|| visible.iter().find(is_text).cloned()),
        ModelRole::Chat | ModelRole::Tool => unreachable!(),
    }
}

fn configured_role_value(project_root: &Path, role: ModelRole) -> Option<String> {
    env_compat(role.env_key(), role.legacy_env_key())
        .or_else(|| env::var(role.open2d_legacy_env_key()).ok())
        .or_else(|| {
            fs::read_to_string(model_config_path(project_root, role))
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
}

fn configured_model_description(project_root: &Path, role: ModelRole) -> String {
    if let Some(value) = env_compat(role.env_key(), role.legacy_env_key())
        .or_else(|| env::var(role.open2d_legacy_env_key()).ok())
    {
        return format!(
            "{} ({} / {} role)",
            value.trim(),
            role.env_key(),
            role.as_str()
        );
    }

    let path = model_config_path(project_root, role);
    if let Ok(value) = fs::read_to_string(&path) {
        if !value.trim().is_empty() {
            return format!("{} (project {} role)", value.trim(), role.as_str());
        }
    }

    if role != ModelRole::Default {
        if let Some(fallback) = configured_role_value(project_root, ModelRole::Default) {
            return format!("{fallback} (fallback from default role)");
        }
    }

    match role {
        ModelRole::Tool => "<automatic: provider tool-capable model>".into(),
        ModelRole::Vision => "<automatic: provider vision-capable model>".into(),
        ModelRole::Embedding => "<automatic: embedding model>".into(),
        ModelRole::Chat | ModelRole::Default => {
            "<automatic: first non-embedding text model>".into()
        }
    }
}

fn model_config_path(project_root: &Path, role: ModelRole) -> PathBuf {
    let file_name = match role {
        ModelRole::Default => "lmstudio-model.txt".to_string(),
        _ => format!("lmstudio-model-{}.txt", role.as_str()),
    };
    cortex_state_dir(project_root).join(file_name)
}

fn doctor_command(
    project_root: &Path,
    service: &mut CortexService,
    args: &[String],
) -> Result<(), String> {
    let full = args
        .iter()
        .any(|arg| arg.eq_ignore_ascii_case("full") || arg == "--full");
    let json_mode = args.iter().any(|arg| arg == "--json");
    let mut checks = Vec::<Value>::new();

    let workspace_status = Workspace::open(project_root);
    let workspace_ok = workspace_status.is_ok();
    doctor_check(
        &mut checks,
        "workspace",
        workspace_ok,
        match workspace_status {
            Ok(workspace) => json!({"profile": workspace.profile()}),
            Err(error) => json!({"root": project_root, "error": error.to_string()}),
        },
    );

    let models = service.engine.provider().list_models();
    let provider_ok = models.is_ok();
    doctor_check(
        &mut checks,
        "provider",
        provider_ok,
        match &models {
            Ok(values) => json!({"models_visible": values.len()}),
            Err(error) => json!({"error": error.to_string()}),
        },
    );

    let visible_models = models.clone().unwrap_or_default();
    let tool_model =
        resolve_runtime_model_for_role(project_root, ModelRole::Tool, service.engine.provider());
    let chat_model =
        resolve_runtime_model_for_role(project_root, ModelRole::Chat, service.engine.provider());

    doctor_check(
        &mut checks,
        "chat_model_resolution",
        chat_model.is_some(),
        json!({
            "role": "chat",
            "model": chat_model,
            "configured": configured_model_description(project_root, ModelRole::Chat)
        }),
    );

    doctor_check(
        &mut checks,
        "tool_model_resolution",
        tool_model.is_some(),
        json!({
            "role": "tool",
            "model": tool_model,
            "configured": configured_model_description(project_root, ModelRole::Tool)
        }),
    );

    let capabilities = service
        .engine
        .provider()
        .model_capabilities()
        .unwrap_or_default();

    let tool_capability = tool_model
        .as_deref()
        .and_then(|model| capability_for_visible_model(&capabilities, model));

    let recommended_tool_model = visible_models.iter().find(|model| {
        capability_for_visible_model(&capabilities, model)
            .and_then(|info| info.trained_for_tool_use)
            == Some(true)
    });

    let tool_capability_ok = tool_model.is_some()
        && !matches!(
            tool_capability.and_then(|info| info.trained_for_tool_use),
            Some(false)
        );

    doctor_check(
        &mut checks,
        "tool_model_capability",
        tool_capability_ok,
        json!({
            "role": "tool",
            "model": tool_model,
            "configured": configured_model_description(project_root, ModelRole::Tool),
            "trained_for_tool_use": tool_capability.and_then(|info| info.trained_for_tool_use),
            "vision": tool_capability.and_then(|info| info.vision),
            "recommended_tool_model": recommended_tool_model,
            "repair_command": recommended_tool_model.map(|model| format!(
                "cortex model set tool {model}"
            ))
        }),
    );

    let definitions = service.engine.tools().definitions();
    let certification = certify_tool_definitions(&definitions);
    let tool_registry_ok = certification
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    doctor_check(
        &mut checks,
        "tool_registry",
        tool_registry_ok,
        certification,
    );

    let project_status = service.engine.tools_mut().execute(&ToolCall {
        call_id: "doctor-project-status".into(),
        name: "project.status".into(),
        arguments: json!({}),
    });
    let project_status_ok = !project_status.is_error;
    doctor_check(
        &mut checks,
        "project_status_tool",
        project_status_ok,
        project_status.output,
    );

    let token_path = cortex_state_dir(project_root).join("rpc.token");
    let token_ok = fs::read_to_string(&token_path)
        .ok()
        .map(|token| token.trim().len() >= 24)
        .unwrap_or(false);
    doctor_check(
        &mut checks,
        "rpc_token",
        token_ok,
        json!({"path": token_path}),
    );

    let core_ready =
        workspace_ok && provider_ok && tool_registry_ok && project_status_ok && token_ok;
    let chat_ready = core_ready && chat_model.is_some();
    let mut project_agent_ready = core_ready && tool_capability_ok;
    let mut structured_smoke_ok = None;

    if full {
        let tool_smoke = match tool_model.as_deref() {
            Some(model) if tool_capability_ok => {
                run_provider_tool_smoke(service.engine.provider(), model, &definitions)
            }
            Some(model) => Err(format!(
                "selected tool model `{model}` is not certified for structured tool use"
            )),
            None => Err("no tool-role model could be resolved".into()),
        };
        let smoke_ok = tool_smoke.is_ok();
        structured_smoke_ok = Some(smoke_ok);
        project_agent_ready &= smoke_ok;

        doctor_check(
            &mut checks,
            "structured_tool_call_smoke",
            smoke_ok,
            match tool_smoke {
                Ok(value) => value,
                Err(error) => json!({"error": error}),
            },
        );
    }

    let full_ready = core_ready && chat_ready && project_agent_ready;
    let report = json!({
        "schema_version": 2,
        "command": if full { "doctor_full" } else { "doctor_quick" },
        "readiness": {
            "core": core_ready,
            "chat": chat_ready,
            "project_agent": project_agent_ready,
            "structured_tool_smoke": structured_smoke_ok,
            "full": full_ready
        },
        "checks": checks
    });

    if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
    } else {
        println!("OPEN2D CORTEX DOCTOR");
        println!("Mode: {}", if full { "FULL" } else { "QUICK" });
        for check in report
            .get("checks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let ok = check.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let name = check.get("name").and_then(Value::as_str).unwrap_or("check");
            println!("[{}] {name}", if ok { "PASS" } else { "FAIL" });

            if !ok && name == "tool_model_capability" {
                if let Some(details) = check.get("details") {
                    if let Some(model) = details.get("model").and_then(Value::as_str) {
                        println!("       Tool model : {model}");
                    }
                    if let Some(configured) = details.get("configured").and_then(Value::as_str) {
                        println!("       Selection  : {configured}");
                    }
                    if let Some(recommended) = details
                        .get("recommended_tool_model")
                        .and_then(Value::as_str)
                    {
                        println!("       Suggested  : {recommended}");
                        println!("       Fix        : cortex model set tool {recommended}");
                    } else {
                        println!(
                            "       Fix        : Cortex Settings -> Models -> select a tool-capable model and choose one reported tool=yes"
                        );
                    }
                }
            }
        }

        println!();
        println!(
            "Core          : {}",
            if core_ready { "READY" } else { "NOT READY" }
        );
        println!(
            "Chat          : {}",
            if chat_ready { "READY" } else { "NOT READY" }
        );
        println!(
            "Project agent : {}",
            if project_agent_ready {
                "READY"
            } else {
                "NOT READY"
            }
        );
        if full {
            println!(
                "Full certify  : {}",
                if full_ready { "READY" } else { "NOT READY" }
            );
        }
        println!();

        if core_ready && chat_ready && !project_agent_ready {
            println!(
                "Cortex: OPERATIONAL (chat/core ready; grounded project agent needs a tool-capable model)"
            );
        } else {
            println!(
                "Cortex: {}",
                if full_ready || (!full && core_ready && chat_ready) {
                    "READY"
                } else {
                    "NOT READY"
                }
            );
        }
    }

    if full {
        if full_ready {
            Ok(())
        } else {
            Err("Cortex full doctor found one or more certification failures".into())
        }
    } else if core_ready && chat_ready {
        Ok(())
    } else {
        Err("Cortex quick doctor found one or more core operational failures".into())
    }
}

fn capability_for_visible_model<'a>(
    capabilities: &'a [ModelCapability],
    model: &str,
) -> Option<&'a ModelCapability> {
    capabilities.iter().find(|info| {
        info.key == model
            || info
                .loaded_instance_ids
                .iter()
                .any(|loaded| loaded == model)
    })
}

fn doctor_check(checks: &mut Vec<Value>, name: &str, ok: bool, details: Value) {
    checks.push(json!({
        "name": name,
        "ok": ok,
        "details": details
    }));
}

fn run_provider_tool_smoke(
    provider: &CortexProvider,
    model: &str,
    definitions: &[ToolDefinition],
) -> Result<Value, String> {
    let project_status = definitions
        .iter()
        .find(|tool| tool.name == "workspace.status")
        .cloned()
        .ok_or_else(|| "workspace.status tool definition is missing".to_string())?;

    let prompt =
        "For this certification check, call the workspace status function once. Do not answer from memory.";
    let mut first_request = ModelRequest::user(prompt);
    first_request.model = Some(model.to_string());
    first_request.instructions =
        Some("This is a Cortex structured-tool certification. Emit the required tool call.".into());
    first_request.tools = vec![project_status.clone()];
    first_request.tool_choice = ToolChoicePolicy::Required;

    let first = provider
        .respond(&first_request)
        .map_err(|e| e.to_string())?;
    let first_call = first
        .tool_calls
        .iter()
        .find(|call| call.name == "workspace.status")
        .cloned()
        .ok_or_else(|| {
            format!(
                "model returned no workspace.status structured tool call; response text preview: {}",
                first.output_text.chars().take(240).collect::<String>()
            )
        })?;

    // Certification must prove a real second tool-bearing turn. A single first-turn
    // function call does not certify stateless replay/template compatibility.
    let replay_messages = vec![
        ModelMessage {
            role: MessageRole::User,
            content: prompt.into(),
        },
        ModelMessage {
            role: MessageRole::Tool,
            content: json!({
                "cortex_replay_type":"function_call",
                "call_id":first_call.call_id,
                "name":first_call.name,
                "arguments":first_call.arguments,
            })
            .to_string(),
        },
        ModelMessage {
            role: MessageRole::Tool,
            content: json!({
                "cortex_replay_type":"function_call_output",
                "call_id":first_call.call_id,
                "output":{
                    "ok":true,
                    "certification":"synthetic workspace status result",
                    "workspace_root":"CORTEX_CERTIFICATION_FIXTURE"
                },
                "is_error":false
            })
            .to_string(),
        },
    ];
    let second_request = ModelRequest {
        model: Some(model.to_string()),
        instructions: Some(
            "Cortex multi-turn tool continuity certification. Use the prior authoritative tool result and call workspace.status once more. Emit a real structured tool call."
                .into(),
        ),
        messages: replay_messages,
        tools: vec![project_status],
        tool_choice: ToolChoicePolicy::Required,
        previous_response_id: None,
        tool_results: Vec::new(),
        images: Vec::new(),
    };
    let second = provider.respond(&second_request).map_err(|error| {
        format!(
            "first structured tool call succeeded, but the required second-turn stateless tool continuation failed: {error}"
        )
    })?;
    let second_called = second
        .tool_calls
        .iter()
        .any(|call| call.name == "workspace.status");
    if !second_called {
        return Err(format!(
            "first structured tool call succeeded, but the second-turn continuity request returned no workspace.status tool call; response text preview: {}",
            second.output_text.chars().take(240).collect::<String>()
        ));
    }

    Ok(json!({
        "model": model,
        "first_response_id": first.id,
        "first_tool_calls": first.tool_calls,
        "second_response_id": second.id,
        "second_tool_calls": second.tool_calls,
        "multi_turn_tool_continuity": true
    }))
}

fn tools_command(service: &mut CortexService, args: &[String]) -> Result<(), String> {
    let command = args.first().map(String::as_str).unwrap_or("list");
    let json_mode = args.iter().any(|arg| arg == "--json");
    let definitions = service.engine.tools().definitions();

    match command {
        "list" => {
            if json_mode {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&definitions).map_err(|e| e.to_string())?
                );
            } else {
                for definition in &definitions {
                    println!(
                        "{:<34} {:<7} {}",
                        definition.name,
                        if definition.mutating { "WRITE" } else { "READ" },
                        definition.description
                    );
                }
                println!("{} tool(s)", definitions.len());
            }
        }
        "describe" => {
            let name = args
                .get(1)
                .ok_or_else(|| "usage: cortex tools describe <name> [--json]".to_string())?;
            let definition = definitions
                .iter()
                .find(|definition| &definition.name == name)
                .ok_or_else(|| format!("unknown Cortex tool: {name}"))?;
            if json_mode {
                println!(
                    "{}",
                    serde_json::to_string_pretty(definition).map_err(|e| e.to_string())?
                );
            } else {
                println!("Name      : {}", definition.name);
                println!(
                    "Authority : {}",
                    if definition.mutating {
                        "MUTATING"
                    } else {
                        "READ-ONLY"
                    }
                );
                println!("Wire name : {}", provider_safe_tool_name(&definition.name));
                println!("Purpose   : {}", definition.description);
                println!(
                    "Schema    : {}",
                    serde_json::to_string_pretty(&definition.parameters)
                        .map_err(|e| e.to_string())?
                );
            }
        }
        "certify" => {
            let report = certify_tool_definitions(&definitions);
            let ok = report.get("ok").and_then(Value::as_bool).unwrap_or(false);
            if json_mode {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
                );
            } else {
                println!("OPEN2D CORTEX TOOL CERTIFICATION");
                println!("Tools : {}", definitions.len());
                println!("Result: {}", if ok { "PASS" } else { "FAIL" });
                if let Some(issues) = report.get("issues").and_then(Value::as_array) {
                    for issue in issues {
                        if let Some(issue) = issue.as_str() {
                            println!(" - {issue}");
                        }
                    }
                }
            }
            if !ok {
                return Err("tool certification failed".into());
            }
        }
        "test" => {
            let name = args.get(1).ok_or_else(|| {
                "usage: cortex tools test <name> [json-args] [--allow-mutation]".to_string()
            })?;
            let definition = definitions
                .iter()
                .find(|definition| &definition.name == name)
                .ok_or_else(|| format!("unknown Cortex tool: {name}"))?;
            let allow_mutation = args.iter().any(|arg| arg == "--allow-mutation");
            if definition.mutating && !allow_mutation {
                return Err(format!(
                    "tool `{name}` is mutating; rerun with --allow-mutation only when intentional"
                ));
            }
            let arguments = args
                .iter()
                .skip(2)
                .find(|arg| !arg.starts_with("--"))
                .map(|text| serde_json::from_str::<Value>(text).map_err(|e| e.to_string()))
                .transpose()?
                .unwrap_or_else(|| json!({}));
            let result = service.engine.tools_mut().execute(&ToolCall {
                call_id: format!("cli-tool-test-{}", unix_ms()),
                name: name.clone(),
                arguments,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "tool": name,
                    "is_error": result.is_error,
                    "output": result.output
                }))
                .map_err(|e| e.to_string())?
            );
            if result.is_error {
                return Err(format!("tool test failed: {name}"));
            }
        }
        "test-all" => {
            let safe_smoke = [
                "workspace.status",
                "project.status",
                "source.transaction_status",
                "runtime.status",
            ];
            let mut results = Vec::<Value>::new();
            let mut failed = false;
            for name in safe_smoke {
                let result = service.engine.tools_mut().execute(&ToolCall {
                    call_id: format!("cli-tool-smoke-{name}"),
                    name: name.into(),
                    arguments: json!({}),
                });
                failed |= result.is_error;
                results.push(json!({
                    "tool": name,
                    "ok": !result.is_error,
                    "output": result.output
                }));
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"results": results}))
                    .map_err(|e| e.to_string())?
            );
            if failed {
                return Err("one or more safe tool smoke tests failed".into());
            }
        }
        other => {
            return Err(format!(
                "unknown tools command: {other}; use list, describe, certify, test, or test-all"
            ));
        }
    }

    Ok(())
}

fn certify_tool_definitions(definitions: &[ToolDefinition]) -> Value {
    let mut issues = Vec::<String>::new();
    let mut canonical = BTreeSet::<String>::new();
    let mut wire = BTreeMap::<String, String>::new();

    for definition in definitions {
        if definition.name.trim().is_empty() {
            issues.push("tool with empty canonical name".into());
        }
        if !canonical.insert(definition.name.clone()) {
            issues.push(format!(
                "duplicate canonical tool name: {}",
                definition.name
            ));
        }
        if definition.description.trim().is_empty() {
            issues.push(format!("tool has empty description: {}", definition.name));
        }
        if !definition.parameters.is_object() {
            issues.push(format!(
                "tool parameters are not a JSON object schema: {}",
                definition.name
            ));
        }

        let wire_name = provider_safe_tool_name(&definition.name);
        if wire_name.is_empty() || wire_name.len() > 64 {
            issues.push(format!(
                "provider-safe tool name has invalid length: {} -> {}",
                definition.name, wire_name
            ));
        }
        if let Some(existing) = wire.insert(wire_name.clone(), definition.name.clone()) {
            if existing != definition.name {
                issues.push(format!(
                    "provider-safe tool-name collision: {existing} and {} -> {wire_name}",
                    definition.name
                ));
            }
        }
    }

    json!({
        "ok": issues.is_empty(),
        "tool_count": definitions.len(),
        "issues": issues
    })
}

fn provider_safe_tool_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

fn transaction_command(service: &mut CortexService, args: &[String]) -> Result<(), String> {
    let command = args.first().map(String::as_str).unwrap_or("status");
    let (tool_name, arguments) = match command {
        "status" => ("source.transaction_status", json!({})),
        "files" => ("source.transaction_files", json!({})),
        "begin" => {
            let label = if args.len() > 1 {
                args[1..].join(" ")
            } else {
                "cli-edit".into()
            };
            ("source.begin_transaction", json!({"label": label}))
        }
        "checkpoint" => {
            let path = args
                .get(1)
                .ok_or_else(|| "usage: cortex tx checkpoint <project-relative-path>".to_string())?;
            ("source.checkpoint", json!({"path": path}))
        }
        "commit" => {
            if !args.iter().any(|arg| arg == "--force") {
                let root = service.engine.tools().workspace_root().to_path_buf();
                let verification = execute_named_tool(
                    service,
                    project_validation_tool(&root),
                    json!({}),
                    "tx-commit-check",
                );
                if !tool_result_success(&verification) {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&verification.output)
                            .map_err(|e| e.to_string())?
                    );
                    return Err(
                        "transaction commit blocked because project-authoritative verification failed; repair or rollback the candidate (the --force override is explicit and unsafe)"
                            .into(),
                    );
                }
            }
            ("source.commit", json!({}))
        }
        "rollback" => ("source.rollback", json!({})),
        other => {
            return Err(format!(
                "unknown transaction command: {other}; use status, files, begin, checkpoint, commit, or rollback"
            ));
        }
    };

    let result = service.engine.tools_mut().execute(&ToolCall {
        call_id: format!("cli-tx-{}", unix_ms()),
        name: tool_name.into(),
        arguments,
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&result.output).map_err(|e| e.to_string())?
    );
    if result.is_error {
        Err(format!("transaction command failed: {command}"))
    } else {
        Ok(())
    }
}

/// W11: ground the physical active root before starting a mutating repair.
/// The optional SOURCE_MANIFEST and [workspace].members fields are not filesystem counts.
fn repair_project_preflight(root: &Path) -> Result<Value, String> {
    let items = fs::read_dir(root).map_err(|error| {
        format!(
            "repair preflight cannot enumerate {}: {error}",
            root.display()
        )
    })?;
    let mut observed = Vec::new();
    let mut truncated = false;
    for item in items {
        let item = item.map_err(|error| format!("repair root enumeration failed: {error}"))?;
        if observed.len() >= 48 {
            truncated = true;
            break;
        }
        observed.push(item.file_name().to_string_lossy().to_string());
    }
    observed.sort();
    let markers = [
        "Cargo.toml",
        "PCC.cmd",
        "PCC.sh",
        "project.control.json",
        "CMakeLists.txt",
        "pyproject.toml",
        "package.json",
        "build.gradle",
        "build.gradle.kts",
    ];
    let present = markers
        .iter()
        .filter(|marker| root.join(marker).is_file())
        .copied()
        .collect::<Vec<_>>();
    if present.is_empty() {
        return Err(format!(
            "repair blocked before any transaction: no authoritative project/build marker was observed in {}. Found root entries {:?}. Do not initialize, delete, or infer an empty project; select the correct registered project or inspect its build contract first.",
            root.display(), observed
        ));
    }
    Ok(json!({
        "active_root":root,
        "root_enumeration":"verified",
        "root_entries_sample":observed,
        "root_sample_truncated":truncated,
        "project_markers":present,
        "src_directory_exists":root.join("src").is_dir(),
        "Cargo_lock_exists":root.join("Cargo.lock").is_file(),
        "note":"SOURCE_MANIFEST file_count and explicit Cargo workspace members are NOT actual filesystem counts; never conclude empty project from those values."
    }))
}

fn project_validation_tool(root: &Path) -> &'static str {
    if cortex_development::detect_project_checkpoint_authority(root).project_native() {
        "build.project_checkpoint"
    } else {
        "build.project_validate"
    }
}

fn project_agent_command(
    service: &mut CortexService,
    action: &str,
    prompt: &str,
    provider_timeout: Option<Duration>,
    output: OutputMode,
    reuse_transaction: bool,
) -> Result<(), String> {
    let provider_kind = service.engine.provider().kind();
    let model = service
        .engine
        .provider()
        .resolve_model()
        .map_err(|e| e.to_string())?;
    let capability = service
        .engine
        .provider()
        .capability_for_model(&model)
        .ok()
        .flatten();

    if matches!(
        capability
            .as_ref()
            .and_then(|info| info.trained_for_tool_use),
        Some(false)
    ) {
        return Err(format!(
            "Selected Cortex provider model `{model}` reports tool=no. `{action}` requires structured tool use. Select a tool-capable model for the Cortex tool role."
        ));
    }

    let mutating = matches!(action, "apply" | "repair");
    let mode = if mutating {
        AgentMode::WorkspaceAutonomy
    } else {
        AgentMode::Observe
    };
    service.engine.set_mode(mode);

    let repair_grounding = if action == "repair" {
        let root = service.engine.tools().workspace_root().to_path_buf();
        Some(repair_project_preflight(&root)?)
    } else {
        None
    };
    if let (OutputMode::Human, Some(evidence)) = (output, &repair_grounding) {
        eprintln!("[PASS] Physical project-root preflight: {evidence}");
    }

    let tx_id = if mutating {
        Some(ensure_active_transaction(
            service,
            action,
            reuse_transaction,
        )?)
    } else {
        None
    };

    let effective_prompt = match action {
        "plan" => format!(
            "Read-only planning mode. Inspect the current project using Cortex tools. Do not mutate files. Produce a concrete implementation plan with files, validation steps and risks.\n\nUser request: {prompt}"
        ),
        "apply" => format!(
            "An active durable Cortex transaction is already open. Do not begin another transaction and do not commit or roll it back. You have authoritative project source inspection and project-file mutation tools in this mode. Never tell the user to edit files manually or claim that you cannot edit/save project files. Apply the requested bounded changes using Cortex source tools, then run build.cargo_check before finishing. Leave the transaction active for human review.\n\nUser request: {prompt}"
        ),
        "repair" => format!(
            "An active durable Cortex transaction is open. Controller-verified project-root facts (not a model inference): {}. These override any optional SOURCE_MANIFEST file_count=0 or Cargo member_count=0. If the native PCC failure excerpt is missing, say that diagnostic evidence is missing; do not invent an empty project or initialize Cargo. First inspect the actual root manifest and failing diagnostics using structured tools. Use actual source.read/source.search and source.replace_text/source.write_text calls: fenced pseudo-commands are NOT executed. Make one bounded candidate change grounded in real source, and never modify registry cache or unrelated projects. Do not begin/commit/rollback transactions or run build/orchestration tools; the controller alone verifies changes and reports real transaction state. A suggestion or prose diagnosis is not a repair.\n\nRepair request: {prompt}",
            repair_grounding.as_ref().unwrap_or(&Value::Null)
        ),
        _ => prompt.to_string(),
    };

    let tool_status = match capability
        .as_ref()
        .and_then(|info| info.trained_for_tool_use)
    {
        Some(true) => "trained",
        Some(false) => "not trained",
        None => "unknown/parser-dependent",
    };

    if output == OutputMode::Human {
        print_request_header(
            &format!("PROJECT {}", action.to_ascii_uppercase()),
            &model,
            mode,
            provider_timeout,
            (service.engine.settings().max_elapsed_seconds > 0)
                .then_some(service.engine.settings().max_elapsed_seconds),
            "REQUIRED",
        );
        eprintln!("Tool use  : {tool_status}");
        if let Some(id) = &tx_id {
            eprintln!("Transaction: {id} (left active for review)");
        }
    }
    emit_event(
        output,
        "request_started",
        json!({
            "kind":action,
            "provider":provider_kind.as_str(),
            "model":model,
            "grounding":"required",
            "transaction_id":tx_id
        }),
    );

    service.engine.set_tool_profile(if action == "repair" {
        AgentToolProfile::Repair
    } else {
        AgentToolProfile::General
    });

    let agent_limit = (service.engine.settings().max_elapsed_seconds > 0).then_some(
        Duration::from_secs(service.engine.settings().max_elapsed_seconds),
    );
    let result = run_with_progress("project agent", agent_limit, output, || {
        service.engine.run(&effective_prompt)
    });
    service.engine.set_tool_profile(AgentToolProfile::General);
    let result = result?;

    if mutating && !has_successful_project_file_mutation(&result) {
        if !reuse_transaction {
            let _ = execute_named_tool(
                service,
                "source.rollback",
                json!({}),
                "empty-mutation-rollback",
            );
        }
        return Err(format!(
            "Cortex {action} completed without a successful project-file mutation tool call. Prose, code snippets, or suggested edits are not accepted as implementation. The request remains incomplete. Structured evidence: {}",
            agent_evidence_summary(&result)
        ));
    }

    let validation_tool = mutating.then(|| {
        let root = service.engine.tools().workspace_root();
        project_validation_tool(root)
    });
    let verification = if let Some(tool) = validation_tool {
        if output == OutputMode::Human {
            eprintln!(
                "[INFO] Controller verification: {tool} (project-owned checkpoint takes precedence)"
            );
        }
        Some(execute_named_tool(
            service,
            tool,
            json!({}),
            "post-agent-check",
        ))
    } else {
        None
    };

    let tx_status = if mutating {
        Some(execute_named_tool(
            service,
            "source.transaction_status",
            json!({}),
            "post-agent-tx-status",
        ))
    } else {
        None
    };

    let tx_files = if mutating {
        Some(execute_named_tool(
            service,
            "source.transaction_files",
            json!({}),
            "post-agent-tx-files",
        ))
    } else {
        None
    };
    let evidence_input = AgentEvidenceWrite {
        session_dir: service.engine.tools().session().directory.as_path(),
        mode: action,
        prompt,
        model: &model,
        result: &result,
        transaction_id: tx_id.as_deref(),
        verification: verification.as_ref(),
        verification_tool: validation_tool,
        transaction_status: tx_status.as_ref(),
        transaction_files: tx_files.as_ref(),
        project_grounding: repair_grounding.as_ref(),
    };
    let evidence_path = write_agent_evidence(&evidence_input)?;

    let transaction_active = tx_status
        .as_ref()
        .and_then(|status| status.output.pointer("/transaction/id"))
        .and_then(Value::as_str)
        .is_some_and(|id| tx_id.as_deref() == Some(id));
    let verification_ok = verification
        .as_ref()
        .map(tool_result_success)
        .unwrap_or(true)
        && (!mutating || transaction_active);

    match output {
        OutputMode::Human => {
            println!("Model report (unverified narrative):\n{}", result.text);
            eprintln!(
                "Grounding : physical-root preflight + structured tool evidence; model narrative is not verification"
            );
            eprintln!(
                "Evidence  : {} structured tool call(s)",
                result.evidence.len()
            );
            for evidence in &result.evidence {
                eprintln!(
                    "  [{}] {}{}",
                    if evidence.is_error { "FAIL" } else { "PASS" },
                    evidence.tool,
                    if evidence.mutating { " [mutating]" } else { "" }
                );
            }
            if mutating {
                eprintln!(
                    "Post-check : {}",
                    if verification_ok {
                        "PASS - candidate remains reviewable"
                    } else if transaction_active {
                        "FAIL - transaction remains active for review"
                    } else {
                        "FAIL - no active candidate (controller rolled back or transaction ended)"
                    }
                );
                eprintln!(
                    "Verification authority: {}",
                    validation_tool.unwrap_or("not selected")
                );
            }
            eprintln!("Evidence file: {}", evidence_path.display());
            eprintln!("Iterations: {}", result.iterations);
        }
        OutputMode::Json => println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schema_version":1,
                "mode":action,
                "grounding":"controller_root_preflight_and_tool_evidence",
                "model":model,
                "text":result.text,
                "iterations":result.iterations,
                "evidence":result.evidence,
                "transaction_id":tx_id,
                "verification":verification,
                "transaction_status":tx_status,
                "transaction_files":tx_files,
                "project_grounding":repair_grounding,
                "verification_tool":validation_tool,
                "transaction_active":transaction_active,
                "evidence_file":evidence_path,
                "success":verification_ok
            }))
            .map_err(|e| e.to_string())?
        ),
        OutputMode::Jsonl => emit_event(
            output,
            "agent_finished",
            json!({
                "mode":action,
                "grounding":"controller_root_preflight_and_tool_evidence",
                "model":model,
                "text":result.text,
                "iterations":result.iterations,
                "evidence":result.evidence,
                "transaction_id":tx_id,
                "verification":verification,
                "transaction_status":tx_status,
                "transaction_files":tx_files,
                "project_grounding":repair_grounding,
                "verification_tool":validation_tool,
                "transaction_active":transaction_active,
                "evidence_file":evidence_path,
                "success":verification_ok
            }),
        ),
    }

    if !verification_ok {
        return Err(format!(
            "{action} did not pass project-authoritative verification ({}) ; transaction {}. Review the evidence receipt before another mutation",
            validation_tool.unwrap_or("not selected"),
            if transaction_active {
                "remains active for review"
            } else {
                "is not active (possible controller rollback)"
            }
        ));
    }
    Ok(())
}

fn agent_evidence_summary(result: &AgentTurnResult) -> String {
    if result.evidence.is_empty() {
        return "none".into();
    }
    result
        .evidence
        .iter()
        .map(|evidence| {
            format!(
                "{}{}{}",
                evidence.tool,
                if evidence.mutating {
                    "[write]"
                } else {
                    "[read]"
                },
                if evidence.is_error { "[error]" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn has_successful_project_file_mutation(result: &AgentTurnResult) -> bool {
    result.evidence.iter().any(|evidence| {
        !evidence.is_error
            && matches!(
                evidence.tool.as_str(),
                "source.write_text"
                    | "source.replace_text"
                    | "vscode.apply_workspace_edit"
                    | "image.promote"
            )
    })
}

fn rollback_empty_transaction_after_failed_agent(service: &mut CortexService) {
    let files = execute_named_tool(
        service,
        "source.transaction_files",
        json!({}),
        "failed-agent-tx-files",
    );
    if files.is_error {
        return;
    }
    let touched = files
        .output
        .get("touched")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let created = files
        .output
        .get("created")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    if touched + created == 0 {
        let _ = execute_named_tool(
            service,
            "source.rollback",
            json!({}),
            "failed-agent-empty-tx-rollback",
        );
    }
}

fn ensure_active_transaction(
    service: &mut CortexService,
    label: &str,
    reuse_transaction: bool,
) -> Result<String, String> {
    let status = execute_named_tool(
        service,
        "source.transaction_status",
        json!({}),
        "ensure-tx-status",
    );
    if status.is_error {
        return Err(status
            .output
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("could not read transaction status")
            .to_string());
    }
    if let Some(id) = status
        .output
        .pointer("/transaction/id")
        .and_then(Value::as_str)
    {
        if reuse_transaction {
            return Ok(id.to_string());
        }
        return Err(format!(
            "an active Cortex transaction already exists: {id}; commit/rollback it first or rerun with --reuse-transaction intentionally"
        ));
    }

    let begin = execute_named_tool(
        service,
        "source.begin_transaction",
        json!({"label": format!("{label}-{}", unix_ms())}),
        "ensure-tx-begin",
    );
    if begin.is_error {
        return Err(begin
            .output
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("could not begin transaction")
            .to_string());
    }
    begin
        .output
        .get("transaction_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "transaction begin returned no transaction id".to_string())
}

fn execute_named_tool(
    service: &mut CortexService,
    name: &str,
    arguments: Value,
    call_id: &str,
) -> cortex_protocol::ToolResultInput {
    service.engine.tools_mut().execute(&ToolCall {
        call_id: call_id.into(),
        name: name.into(),
        arguments,
    })
}

fn tool_result_success(result: &cortex_protocol::ToolResultInput) -> bool {
    !result.is_error
        && result
            .output
            .get("success")
            .and_then(Value::as_bool)
            .is_none_or(|success| success)
}

struct AgentEvidenceWrite<'a> {
    session_dir: &'a Path,
    mode: &'a str,
    prompt: &'a str,
    model: &'a str,
    result: &'a AgentTurnResult,
    transaction_id: Option<&'a str>,
    verification: Option<&'a cortex_protocol::ToolResultInput>,
    verification_tool: Option<&'a str>,
    transaction_status: Option<&'a cortex_protocol::ToolResultInput>,
    transaction_files: Option<&'a cortex_protocol::ToolResultInput>,
    project_grounding: Option<&'a Value>,
}

fn write_agent_evidence(input: &AgentEvidenceWrite<'_>) -> Result<PathBuf, String> {
    let artifacts = input.session_dir.join("artifacts");
    fs::create_dir_all(&artifacts).map_err(|e| e.to_string())?;
    let path = artifacts.join(format!("cortex-{}-evidence-{}.json", input.mode, unix_ms()));
    let report = json!({
        "schema_version": 2,
        "mode": input.mode,
        "grounding": "controller_root_preflight_and_tool_evidence",
        "project_grounding": input.project_grounding,
        "verification_tool": input.verification_tool,
        "transaction_files": input.transaction_files,
        "prompt": input.prompt,
        "model": input.model,
        "iterations": input.result.iterations,
        "evidence": input.result.evidence,
        "tool_result_count": input.result.tool_results.len(),
        "response_id": input.result.last_response_id,
        "transaction_id": input.transaction_id,
        "verification": input.verification,
        "transaction_status": input.transaction_status
    });
    fs::write(
        &path,
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(path)
}

fn build_command(service: &mut CortexService, args: &[String]) -> Result<(), String> {
    let command = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .map(String::as_str)
        .unwrap_or("check");
    let output = output_mode(args);
    let tool_name = match command {
        "fmt" | "fmt-check" => "build.cargo_fmt_check",
        "check" => "build.cargo_check",
        "test-compile" | "test-no-run" => "build.cargo_test",
        "test" => "build.cargo_test_run",
        "clippy" | "lint" => "build.clippy",
        "certify" => "build.certify",
        other => return Err(format!(
            "unknown build command: {other}; use fmt-check, check, test-compile, test, clippy, or certify"
        )),
    };
    emit_event(
        output,
        "build_started",
        json!({"command":command,"tool":tool_name}),
    );
    let result = execute_named_tool(service, tool_name, json!({}), "cli-build");
    let ok = tool_result_success(&result);
    match output {
        OutputMode::Human => println!(
            "{}",
            serde_json::to_string_pretty(&result.output).map_err(|e| e.to_string())?
        ),
        OutputMode::Json => println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schema_version":1,
                "command":command,
                "success":ok,
                "result":result.output
            }))
            .map_err(|e| e.to_string())?
        ),
        OutputMode::Jsonl => emit_event(
            output,
            "build_finished",
            json!({
                "command":command,
                "success":ok,
                "result":result.output
            }),
        ),
    }
    if ok {
        Ok(())
    } else {
        Err(format!("build command `{command}` failed"))
    }
}

fn run_local_command(workspace: &Workspace, args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str).unwrap_or("status") {
        "conversation" | "conversations" => conversation_command(workspace, &args[1..]),
        "git" => git_command(workspace, &args[1..]),
        "plugin" | "plugins" => plugin_command(workspace, &args[1..]),
        "permissions" => permissions_command(workspace, &args[1..]),
        "vault" => vault_command(workspace, &args[1..]),
        "observability" => observability_command(workspace, &args[1..]),
        "workspace" => workspace_command(workspace, &args[1..]),
        "project" => project_command(workspace, &args[1..]),
        "desktop" => desktop_command(workspace, &args[1..]),
        "settings" => settings_command(workspace, &args[1..]),
        "review" => review_command(workspace),
        "activity" => activity_command(workspace, &args[1..]),
        "tasks" => tasks_command(workspace, &args[1..]),
        other => Err(format!("unsupported local Cortex command: {other}")),
    }
}

fn service_lifecycle_command(workspace: &Workspace, args: &[String]) -> Result<(), String> {
    let r = ServiceRegistry::new(workspace.cortex_state_dir(), workspace.root())?;
    match args.first().map(String::as_str).unwrap_or("status") {
        "status" => println!(
            "{}",
            serde_json::to_string_pretty(&r.read()?).map_err(|e| e.to_string())?
        ),
        "start" => {
            return Err("Cortex service is Desktop-owned and may not be started as a persistent background service. Launch Cortex Desktop instead.".into());
        }
        "stop" => println!(
            "{}",
            if r.stop()? {
                "Cortex service stopped."
            } else {
                "Cortex service was not running."
            }
        ),
        "restart" => {
            return Err("Cortex service is Desktop-owned. Close/reopen Cortex Desktop to restart its service.".into());
        }
        "clear-stale" => println!("Cleared stale state: {}", r.clear_stale()?),
        other => return Err(format!("unknown service command: {other}")),
    }
    Ok(())
}
fn parse_port(args: &[String], default: u16) -> Result<u16, String> {
    if let Some(i) = args.iter().position(|a| a == "--port") {
        return args
            .get(i + 1)
            .ok_or_else(|| "--port requires a number".to_string())?
            .parse::<u16>()
            .map_err(|e| e.to_string());
    }
    args.first()
        .filter(|v| v.chars().all(|c| c.is_ascii_digit()))
        .map(|v| v.parse::<u16>().map_err(|e| e.to_string()))
        .transpose()
        .map(|v| v.unwrap_or(default))
}
fn desktop_owner_pid() -> Option<u32> {
    env::var("CORTEX_DESKTOP_OWNER_PID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid > 0)
}

fn serve_bound_rpc(
    root: &Path,
    service: &mut CortexService,
    server: &RpcServer,
    owner_pid: Option<u32>,
) -> Result<(), String> {
    println!("Cortex RPC listening at {}", server.local_addr()?);
    println!("Workspace: {}", root.display());
    match owner_pid {
        Some(pid) => {
            if !process_is_alive(pid) {
                return Err(format!("Cortex Desktop owner PID {pid} is not running"));
            }
            println!("Desktop owner PID: {pid}");
            server.serve_while(service, || process_is_alive(pid))
        }
        None => server.serve(service),
    }
}

fn serve_rpc(root: &Path, service: &mut CortexService, port: u16) -> Result<(), String> {
    let owner_pid = desktop_owner_pid();
    if let Some(pid) = owner_pid {
        if !process_is_alive(pid) {
            return Err(format!("Cortex Desktop owner PID {pid} is not running"));
        }
    }
    let server = RpcServer::bind(default_address(port))?;
    serve_bound_rpc(root, service, &server, owner_pid)
}
fn git_command(workspace: &Workspace, args: &[String]) -> Result<(), String> {
    let g = GitAdapter::detect(workspace.root())
        .ok_or_else(|| "Git adapter is not active for this workspace".to_string())?;
    match args.first().map(String::as_str).unwrap_or("status") {
        "status" => println!(
            "{}",
            serde_json::to_string_pretty(&g.status()?).map_err(|e| e.to_string())?
        ),
        "diff" => {
            let staged = args.iter().any(|a| a == "--staged");
            let paths = args
                .iter()
                .skip(1)
                .filter(|a| a.as_str() != "--staged")
                .map(PathBuf::from)
                .collect::<Vec<_>>();
            print!("{}", g.diff(&paths, staged)?);
        }
        "log" => println!(
            "{}",
            serde_json::to_string_pretty(
                &g.log(args.get(1).and_then(|v| v.parse().ok()).unwrap_or(20))?
            )
            .map_err(|e| e.to_string())?
        ),
        "show" => print!(
            "{}",
            g.show(
                args.get(1)
                    .ok_or_else(|| "git show requires a revision".to_string())?
            )?
        ),
        other => return Err(format!("unknown Git command: {other}")),
    }
    Ok(())
}
fn plugin_command(workspace: &Workspace, args: &[String]) -> Result<(), String> {
    let plugins = PluginRegistry::discover(workspace.root(), &workspace.cortex_state_dir())?;
    match args.first().map(String::as_str).unwrap_or("list") {
        "list" => {
            for p in plugins {
                println!(
                    "{:<24} {:<8} {} [{}]",
                    p.manifest.id, p.manifest.version, p.manifest.name, p.source
                )
            }
        }
        "inspect" => {
            let id = args
                .get(1)
                .ok_or_else(|| "plugin inspect requires id".to_string())?;
            let p = plugins
                .into_iter()
                .find(|p| &p.manifest.id == id)
                .ok_or_else(|| format!("plugin not found: {id}"))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?
            )
        }
        other => return Err(format!("unknown plugin command: {other}")),
    }
    Ok(())
}
fn permissions_command(workspace: &Workspace, args: &[String]) -> Result<(), String> {
    let path = workspace.cortex_state_dir().join("permissions.json");
    let mut p = PermissionPolicy::load_or_default(&path)?;
    match args.first().map(String::as_str).unwrap_or("show") {
        "show" => println!(
            "{}",
            serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?
        ),
        "grant" => {
            let v = args
                .get(1)
                .ok_or_else(|| "grant requires permission".to_string())?;
            p.grant(parse_permission(v).ok_or_else(|| format!("unknown permission: {v}"))?);
            p.save(&path)?;
        }
        "revoke" => {
            let v = args
                .get(1)
                .ok_or_else(|| "revoke requires permission".to_string())?;
            p.revoke(parse_permission(v).ok_or_else(|| format!("unknown permission: {v}"))?);
            p.save(&path)?;
        }
        "reset" => {
            p = PermissionPolicy::default();
            p.save(&path)?;
        }
        other => return Err(format!("unknown permissions command: {other}")),
    }
    Ok(())
}
fn workspace_command(workspace: &Workspace, args: &[String]) -> Result<(), String> {
    let registry = WorkspaceRegistry::open_default()?;
    let context = ContextStore::new(workspace.clone());

    match args.first().map(String::as_str).unwrap_or("status") {
        "status" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "profile": workspace.profile(),
                    "context": context.load_inventory()?,
                    "context_path": context.inventory_path(),
                    "index": context.load_index()?,
                    "index_path": context.index_path(),
                    "registry_home": registry.home(),
                    "registered_workspaces": registry.list()?.len()
                }))
                .map_err(|error| error.to_string())?
            );
        }
        "scan" | "rebuild" | "context-rebuild" => {
            let max_files = args
                .get(1)
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(50_000)
                .clamp(1, 250_000);
            let inventory = context.rebuild_inventory(max_files)?;
            let registered = registry.attach(workspace)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "inventory": inventory,
                    "path": context.inventory_path(),
                    "registered_workspace": registered
                }))
                .map_err(|error| error.to_string())?
            );
        }
        "context" | "context-status" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "inventory": context.load_inventory()?,
                    "path": context.inventory_path()
                }))
                .map_err(|error| error.to_string())?
            );
        }
        "index" | "index-rebuild" => {
            let max_files = args
                .get(1)
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(50_000)
                .clamp(1, 250_000);
            let max_hash_bytes = args
                .get(2)
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(512 * 1024)
                .clamp(4 * 1024, 8 * 1024 * 1024);

            let jobs = JobStore::open(workspace.cortex_state_dir())?;
            let events = EventStore::open(workspace.cortex_state_dir())?;
            let mut job = jobs.create("context_index", "Rebuild workspace context index")?;
            jobs.progress(
                &mut job,
                0,
                None,
                "scanning workspace metadata and content fingerprints",
            )?;
            events.append(
                EventKind::Context,
                "Context index",
                "rebuild started",
                None,
                json!({
                    "job_id": job.id.clone(),
                    "max_files": max_files,
                    "max_hash_bytes": max_hash_bytes
                }),
            )?;

            match context.rebuild_index(max_files, max_hash_bytes) {
                Ok((index, delta)) => {
                    let result = json!({
                        "index": index,
                        "delta": delta,
                        "path": context.index_path()
                    });
                    jobs.update(
                        &mut job,
                        JobStatus::Succeeded,
                        "context index rebuilt",
                        result.clone(),
                    )?;
                    events.append(
                        EventKind::Context,
                        "Context index",
                        "rebuild completed",
                        Some(true),
                        json!({
                            "job_id": job.id.clone(),
                            "new_files": delta.new_files.len(),
                            "modified_files": delta.modified_files.len(),
                            "deleted_files": delta.deleted_files.len(),
                            "unchanged_files": delta.unchanged_files,
                            "hashed_files": delta.hashed_files,
                            "reused_hashes": delta.reused_hashes
                        }),
                    )?;
                    let registered = registry.attach(workspace)?;
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "job": job,
                            "result": result,
                            "registered_workspace": registered
                        }))
                        .map_err(|error| error.to_string())?
                    );
                }
                Err(error) => {
                    jobs.update(
                        &mut job,
                        JobStatus::Failed,
                        &error,
                        json!({"path": context.index_path()}),
                    )?;
                    events.append(
                        EventKind::Error,
                        "Context index failed",
                        &error,
                        Some(false),
                        json!({"job_id": job.id.clone()}),
                    )?;
                    return Err(error);
                }
            }
        }
        "index-status" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "index": context.load_index()?,
                    "path": context.index_path()
                }))
                .map_err(|error| error.to_string())?
            );
        }
        "home" => println!("{}", registry.home().display()),
        "list" | "registry" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "active": registry.active()?,
                    "workspaces": registry.list()?
                }))
                .map_err(|error| error.to_string())?
            );
        }
        "attach" => {
            let target = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| workspace.root().to_path_buf());
            let attached_workspace =
                Workspace::open(&target).map_err(|error| error.to_string())?;
            let record = registry.attach(&attached_workspace)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&record).map_err(|error| error.to_string())?
            );
        }
        "activate" => {
            let id = args
                .get(1)
                .ok_or_else(|| "workspace activate requires a registered workspace id".to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&registry.set_active(id)?)
                    .map_err(|error| error.to_string())?
            );
        }
        "detach" => {
            let id = args
                .get(1)
                .ok_or_else(|| "workspace detach requires a registered workspace id".to_string())?;
            println!("detached={}", registry.detach(id)?);
        }
        other => {
            return Err(format!(
                "unknown workspace command: {other}; use status, scan, context-status, index, index-status, home, list, attach, activate, or detach"
            ))
        }
    }
    Ok(())
}

fn project_command(workspace: &Workspace, args: &[String]) -> Result<(), String> {
    let registry = WorkspaceRegistry::open_default()?;
    let _ = registry.ensure_cortex_self_registered()?;

    match args.first().map(String::as_str).unwrap_or("status") {
        "status" => {
            let record = registry.resolve_for_root(workspace.root())?;
            let contract = ProjectContract::load_optional(workspace.root())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "schema_version": 1,
                    "project_root": workspace.root(),
                    "active": registry.active()?,
                    "project": record,
                    "contract": contract,
                    "registered_projects": registry.list()?.len(),
                    "cortex_is_registered": registry.list()?.iter().any(|entry| {
                        entry.project_id.as_ref().is_some_and(|id| id.is_cortex())
                    })
                }))
                .map_err(|error| error.to_string())?
            );
        }
        "register" | "register-self" => {
            let record = registry.attach(workspace)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&record).map_err(|error| error.to_string())?
            );
        }
        "list" | "registry" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "active": registry.active()?,
                    "projects": registry.list()?
                }))
                .map_err(|error| error.to_string())?
            );
        }
        "activate" => {
            let id = args
                .get(1)
                .ok_or_else(|| "project activate requires a registered workspace id".to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&registry.set_active(id)?)
                    .map_err(|error| error.to_string())?
            );
        }
        "contract" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&ProjectContract::load_required(workspace.root())?)
                    .map_err(|error| error.to_string())?
            );
        }
        "gate" => {
            let gate = args.get(1).map(String::as_str).unwrap_or("fast");
            run_native_project_gate(workspace, gate)?;
        }
        other => {
            return Err(format!(
                "unknown project command: {other}; use status, register-self, list, activate, contract, or gate <fast|full>"
            ));
        }
    }
    Ok(())
}

fn run_native_project_gate(workspace: &Workspace, gate_key: &str) -> Result<(), String> {
    let contract = ProjectContract::load_required(workspace.root())?;
    let gate = contract
        .quality_gate(gate_key)
        .ok_or_else(|| format!("unknown project quality gate: {gate_key}"))?;
    let processes = ProcessService::default();
    eprintln!("Cortex project gate: {} ({})", gate.label, gate.key);

    for stage in &gate.stages {
        let command = contract.command(stage).ok_or_else(|| {
            format!(
                "quality gate `{}` references missing command `{stage}`",
                gate.key
            )
        })?;
        eprintln!("[RUN ] {} — {}", command.key, command.label);
        let command_args = command.args.iter().map(String::as_str).collect::<Vec<_>>();
        let parse_cargo_json = command.program.eq_ignore_ascii_case("cargo")
            && command
                .args
                .first()
                .is_some_and(|arg| matches!(arg.as_str(), "check" | "build" | "clippy"));
        let result = match processes.run_capture_exact(
            workspace.root(),
            &command.program,
            &command_args,
            parse_cargo_json,
        ) {
            Ok(result) => result,
            Err(_error) if command.program.eq_ignore_ascii_case("pwsh") => {
                eprintln!("[INFO] pwsh unavailable; retrying stage with Windows PowerShell");
                processes.run_capture_exact(
                    workspace.root(),
                    "powershell",
                    &command_args,
                    parse_cargo_json,
                )?
            }
            Err(error) => return Err(error),
        };
        if !result.stdout.trim().is_empty() {
            print!("{}", result.stdout);
        }
        if !result.stderr.trim().is_empty() {
            eprint!("{}", result.stderr);
        }
        if !result.success {
            return Err(format!(
                "project quality gate `{}` failed at stage `{}` with exit code {:?}",
                gate.key, command.key, result.exit_code
            ));
        }
        eprintln!("[PASS] {} ({} ms)", command.key, result.duration_ms);
    }
    eprintln!("[PASS] Cortex project gate `{}` completed", gate.key);
    Ok(())
}

fn vault_command(workspace: &Workspace, args: &[String]) -> Result<(), String> {
    let path = workspace
        .cortex_state_dir()
        .join("vault")
        .join("vault.json");
    let files = workspace.list_files("", 20000).map_err(|e| e.to_string())?;
    let mut vault = if path.is_file() {
        Vault::load(&path)?
    } else {
        Vault::build_lexical(workspace.root(), &files, 512 * 1024)
    };
    match args.first().map(String::as_str).unwrap_or("status") {
        "status" => println!(
            "{}",
            serde_json::to_string_pretty(&vault.status()).map_err(|e| e.to_string())?
        ),
        "rebuild" => {
            vault = Vault::build_lexical(workspace.root(), &files, 512 * 1024);
            vault.save(&path)?;
            println!(
                "Vault lexical index rebuilt: {} documents",
                vault.status().documents
            );
        }
        "embed" => {
            let kind = configured_provider_kind();
            let url = configured_provider_url(kind);
            let probe = configured_provider(kind, url, None, configured_provider_timeout());
            let model =
                resolve_runtime_model_for_role(workspace.root(), ModelRole::Embedding, &probe)
                    .ok_or_else(|| "no embedding model is available".to_string())?;
            let provider = configured_provider(
                kind,
                probe.base_url().to_string(),
                Some(model.clone()),
                configured_provider_timeout(),
            );
            let count = vault.rebuild_embeddings(
                &provider,
                &model,
                args.get(1).and_then(|v| v.parse().ok()).unwrap_or(2000),
                4000,
            )?;
            vault.save(&path)?;
            println!("Vault embedded {count} documents with {model}");
        }
        "search" => {
            let q = args.iter().skip(1).cloned().collect::<Vec<_>>().join(" ");
            if q.is_empty() {
                return Err("vault search requires query".into());
            }
            let provider = if vault.embedding_model.is_some() {
                let kind = configured_provider_kind();
                Some(configured_provider(
                    kind,
                    configured_provider_url(kind),
                    vault.embedding_model.clone(),
                    configured_provider_timeout(),
                ))
            } else {
                None
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&vault.search(
                    &q,
                    8,
                    provider.as_ref().map(|p| p as &dyn EmbeddingProvider)
                )?)
                .map_err(|e| e.to_string())?
            )
        }
        other => return Err(format!("unknown vault command: {other}")),
    }
    Ok(())
}
fn observability_command(workspace: &Workspace, args: &[String]) -> Result<(), String> {
    let observability = ProjectObservability::open(workspace.root(), &workspace.cortex_state_dir())
        .map_err(|error| error.to_string())?;
    let vector_path = observability.vector_database_path();
    let mut database = VectorMemoryDatabase::load_or_default(&vector_path)?;

    match args.first().map(String::as_str).unwrap_or("status") {
        "status" => {
            let events = observability.events().map_err(|error| error.to_string())?;
            let incidents = observability
                .incidents()
                .map_err(|error| error.to_string())?;
            let candidates = observability
                .candidates()
                .map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "schema_version": 1,
                    "event_ledger": observability.event_path(),
                    "events": events.len(),
                    "incident_ledger": observability.incident_path(),
                    "incidents": incidents.len(),
                    "vector_candidates": candidates.len(),
                    "vector_database": vector_path,
                    "embedded_records": database.records.len(),
                    "embedding_model": database.embedding_model,
                }))
                .map_err(|error| error.to_string())?
            );
        }
        "sync" => {
            let candidates = observability
                .candidates()
                .map_err(|error| error.to_string())?;
            let kind = configured_provider_kind();
            let url = configured_provider_url(kind);
            let probe = configured_provider(kind, url, None, configured_provider_timeout());
            let model =
                resolve_runtime_model_for_role(workspace.root(), ModelRole::Embedding, &probe)
                    .ok_or_else(|| {
                        "no embedding model is available for observability sync".to_string()
                    })?;
            let provider = configured_provider(
                kind,
                probe.base_url().to_string(),
                Some(model.clone()),
                configured_provider_timeout(),
            );
            let added = database.sync_candidates(&provider, &model, &candidates, 6000)?;
            database.save(&vector_path)?;
            println!("Observability vector memory synchronized: {added} new records with {model}");
        }
        "search" => {
            let query = args.iter().skip(1).cloned().collect::<Vec<_>>().join(" ");
            if query.trim().is_empty() {
                return Err("observability search requires a query".into());
            }
            let model = database.embedding_model.clone().ok_or_else(|| {
                "observability vector memory has not been synchronized yet".to_string()
            })?;
            let kind = configured_provider_kind();
            let provider = configured_provider(
                kind,
                configured_provider_url(kind),
                Some(model),
                configured_provider_timeout(),
            );
            let hits = database.search(&query, &provider, 8)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&hits).map_err(|error| error.to_string())?
            );
        }
        other => {
            return Err(format!(
                "unknown observability command: {other}; use status, sync, or search"
            ))
        }
    }
    Ok(())
}

fn desktop_command(workspace: &Workspace, args: &[String]) -> Result<(), String> {
    if args.first().map(String::as_str) == Some("certify-chat") {
        println!("[certify-chat] opening Cortex Desktop controller");
        let mut controller = DesktopController::open(workspace.root())?;
        println!("[certify-chat] desktop controller ready");
        let mut progress = |stage: &str| println!("[certify-chat] {stage}");
        let report = controller.certify_chat_roundtrip_with_progress(&mut progress);
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
        if report.ready {
            println!("Cortex Desktop chat runtime certification: READY");
            return Ok(());
        }
        return Err(report
            .error
            .unwrap_or_else(|| "Cortex Desktop chat runtime certification failed".into()));
    }

    if args.first().map(String::as_str) == Some("certify") {
        let controller = DesktopController::open(workspace.root())?;
        let report = controller.certify();
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
        if report.ready {
            println!("Cortex Desktop certification: READY");
            return Ok(());
        }
        return Err("Cortex Desktop certification failed".into());
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&DesktopBootstrap::load(workspace.root())?)
            .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn settings_command(workspace: &Workspace, args: &[String]) -> Result<(), String> {
    let state_root = workspace.cortex_state_dir();
    let mut settings = CortexSettings::load_or_default(&state_root)?;
    match args.first().map(String::as_str).unwrap_or("show") {
        "show" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&settings).map_err(|error| error.to_string())?
            );
        }
        "set" => {
            let key = args
                .get(1)
                .ok_or_else(|| "settings set requires a key".to_string())?;
            let value = args
                .get(2)
                .ok_or_else(|| "settings set requires a value".to_string())?;
            settings.set(key, value)?;
            settings.save(&state_root)?;
            println!("Cortex setting updated: {key}");
        }
        "reset" => {
            settings = CortexSettings::default();
            settings.save(&state_root)?;
            println!("Cortex settings reset.");
        }
        other => return Err(format!("unknown settings command: {other}")),
    }
    Ok(())
}

fn review_command(workspace: &Workspace) -> Result<(), String> {
    let review = ReviewSnapshot::collect(workspace.root())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&review).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn activity_command(workspace: &Workspace, args: &[String]) -> Result<(), String> {
    let limit = args
        .get(1)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(40);
    let store = ActivityStore::open(workspace.cortex_state_dir())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&store.recent(limit)?).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn tasks_command(workspace: &Workspace, args: &[String]) -> Result<(), String> {
    let limit = args
        .get(1)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(40);
    let store = TaskStore::open(workspace.cortex_state_dir())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&store.list(limit)?).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn conversation_command(workspace: &Workspace, args: &[String]) -> Result<(), String> {
    let store = ConversationStore::open(workspace.cortex_state_dir(), workspace.root())?;
    match args.first().map(String::as_str).unwrap_or("list") {
        "new" => {
            let title = args.iter().skip(1).cloned().collect::<Vec<_>>().join(" ");
            let conversation = store.create(&title)?;
            println!("{}", conversation.id);
        }
        "list" => {
            let include_archived = args.iter().any(|argument| argument == "--all");
            for conversation in store.list(include_archived)? {
                println!(
                    "{}  {}  messages={}{}",
                    conversation.id,
                    conversation.title,
                    conversation.messages.len(),
                    if conversation.archived {
                        " [archived]"
                    } else {
                        ""
                    }
                );
            }
        }
        "show" => {
            let id = args
                .get(1)
                .ok_or_else(|| "conversation show requires an id".to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&store.load(id)?).map_err(|error| error.to_string())?
            );
        }
        "rename" => {
            let id = args
                .get(1)
                .ok_or_else(|| "conversation rename requires an id".to_string())?;
            let title = args.iter().skip(2).cloned().collect::<Vec<_>>().join(" ");
            let conversation = store.rename(id, &title)?;
            println!("Renamed {} -> {}", conversation.id, conversation.title);
        }
        "archive" => {
            let id = args
                .get(1)
                .ok_or_else(|| "conversation archive requires an id".to_string())?;
            let conversation = store.archive(id)?;
            println!("Archived {}", conversation.id);
        }
        other => {
            return Err(format!(
                "unknown conversation command: {other}; use new, list, show, rename, archive"
            ));
        }
    }
    Ok(())
}

fn session_command(project_root: &Path, args: &[String]) -> Result<(), String> {
    let root = Workspace::open(project_root)
        .map_err(|error| error.to_string())?
        .sessions_dir();
    let command = args.first().map(String::as_str).unwrap_or("latest");
    match command {
        "list" => {
            for id in list_session_ids(&root)? {
                println!("{id}");
            }
        }
        "latest" => {
            let id = latest_session_id(&root)?
                .ok_or_else(|| "no Cortex development sessions found".to_string())?;
            println!("{}", root.join(&id).display());
        }
        "show" => {
            let id = resolve_session_id(&root, args.get(1).map(String::as_str))?;
            let path = root.join(id).join("session.json");
            let text = fs::read_to_string(&path)
                .map_err(|e| format!("could not read {}: {e}", path.display()))?;
            println!("{text}");
        }
        "artifacts" => {
            let id = resolve_session_id(&root, args.get(1).map(String::as_str))?;
            let artifacts = root.join(id).join("artifacts");
            if artifacts.is_dir() {
                let mut files = fs::read_dir(&artifacts)
                    .map_err(|e| e.to_string())?
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().is_file())
                    .map(|entry| entry.path())
                    .collect::<Vec<_>>();
                files.sort();
                for path in files {
                    println!("{}", path.display());
                }
            }
        }
        other => return Err(format!(
            "unknown session command: {other}; use list, latest, show [id|latest], or artifacts [id|latest]"
        )),
    }
    Ok(())
}

fn list_session_ids(root: &Path) -> Result<Vec<String>, String> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut ids = fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();
    ids.sort();
    ids.reverse();
    Ok(ids)
}

fn latest_session_id(root: &Path) -> Result<Option<String>, String> {
    Ok(list_session_ids(root)?.into_iter().next())
}

fn resolve_session_id(root: &Path, value: Option<&str>) -> Result<String, String> {
    match value.unwrap_or("latest") {
        "latest" => latest_session_id(root)?
            .ok_or_else(|| "no Cortex development sessions found".to_string()),
        id if !id.contains('/') && !id.contains('\\') && id != "." && id != ".." => {
            let path = root.join(id);
            if path.is_dir() {
                Ok(id.to_string())
            } else {
                Err(format!("session not found: {id}"))
            }
        }
        _ => Err("invalid session id".into()),
    }
}

fn logs_command(project_root: &Path, args: &[String]) -> Result<(), String> {
    let root = Workspace::open(project_root)
        .map_err(|error| error.to_string())?
        .logs_dir();
    let command = args.first().map(String::as_str).unwrap_or("latest");
    match command {
        "list" => {
            if !root.is_dir() {
                return Ok(());
            }
            let mut files = fs::read_dir(&root)
                .map_err(|e| e.to_string())?
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_file())
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .collect::<Vec<_>>();
            files.sort();
            files.reverse();
            for file in files {
                println!("{file}");
            }
        }
        "latest" => {
            let lines = args
                .get(1)
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(80);
            let path = root.join("LATEST_CORTEX_CHECKPOINT.log");
            print_tail(&path, lines)?;
        }
        "tail" => {
            let name = args
                .get(1)
                .map(String::as_str)
                .unwrap_or("LATEST_CORTEX_CHECKPOINT.log");
            if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
                return Err("log name must be a file under logs/sessions".into());
            }
            let lines = args
                .get(2)
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(80);
            print_tail(&root.join(name), lines)?;
        }
        other => {
            return Err(format!(
                "unknown logs command: {other}; use list, latest [lines], or tail [file] [lines]"
            ))
        }
    }
    Ok(())
}

fn print_tail(path: &Path, lines: usize) -> Result<(), String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let all = text.lines().collect::<Vec<_>>();
    let start = all.len().saturating_sub(lines.clamp(1, 10_000));
    for line in &all[start..] {
        println!("{line}");
    }
    Ok(())
}

fn certify_command(
    project_root: &Path,
    service: &mut CortexService,
    args: &[String],
) -> Result<(), String> {
    let output = output_mode(args);
    let definitions = service.engine.tools().definitions();
    let tool_registry = certify_tool_definitions(&definitions);
    let tool_registry_ok = tool_registry
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let model = service
        .engine
        .provider()
        .resolve_model()
        .map_err(|e| e.to_string())?;
    let provider_smoke = run_provider_tool_smoke(service.engine.provider(), &model, &definitions);
    let provider_smoke_ok = provider_smoke.is_ok();

    let transaction_smoke = transaction_smoke_test(project_root, service);
    let transaction_smoke_ok = transaction_smoke.is_ok();

    emit_event(
        output,
        "certification_stage",
        json!({"stage":"build","status":"running"}),
    );
    let build = execute_named_tool(service, "build.certify", json!({}), "coding-cert-build");
    let build_ok = tool_result_success(&build);

    let success = tool_registry_ok && provider_smoke_ok && transaction_smoke_ok && build_ok;
    let report = json!({
        "schema_version":1,
        "certification":"cortex_coding",
        "success":success,
        "model":model,
        "tool_registry":tool_registry,
        "structured_tool_call":match provider_smoke { Ok(value) => value, Err(error) => json!({"error":error}) },
        "transaction_smoke":match transaction_smoke { Ok(value) => value, Err(error) => json!({"error":error}) },
        "build":build.output
    });
    match output {
        OutputMode::Human => {
            println!("CORTEX CODING CERTIFICATION");
            println!(
                "Tool registry      : {}",
                if tool_registry_ok { "PASS" } else { "FAIL" }
            );
            println!(
                "Structured tool use: {}",
                if provider_smoke_ok { "PASS" } else { "FAIL" }
            );
            println!(
                "Transaction safety : {}",
                if transaction_smoke_ok { "PASS" } else { "FAIL" }
            );
            println!(
                "Build gate         : {}",
                if build_ok { "PASS" } else { "FAIL" }
            );
            println!(
                "Overall            : {}",
                if success { "READY" } else { "FAILED" }
            );
        }
        OutputMode::Json => println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        ),
        OutputMode::Jsonl => emit_event(output, "certification_finished", report),
    }
    if success {
        Ok(())
    } else {
        Err("Cortex coding certification failed".into())
    }
}

fn transaction_smoke_test(
    project_root: &Path,
    service: &mut CortexService,
) -> Result<Value, String> {
    let status = execute_named_tool(
        service,
        "source.transaction_status",
        json!({}),
        "cert-tx-status",
    );
    if status
        .output
        .get("transaction")
        .is_some_and(|value| !value.is_null())
    {
        return Err("an active source transaction already exists; finish or roll it back before coding certification".into());
    }
    Workspace::open(project_root).map_err(|error| error.to_string())?;
    let relative = ".cortex-transaction-certification-smoke.txt".to_string();
    let path = project_root.join(&relative);
    if path.exists() {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    let begin = execute_named_tool(
        service,
        "source.begin_transaction",
        json!({"label":"certification-smoke"}),
        "cert-tx-begin",
    );
    if begin.is_error {
        return Err("could not begin certification transaction".into());
    }
    let write = execute_named_tool(
        service,
        "source.write_text",
        json!({"path":&relative,"content":"cortex transaction certification\n"}),
        "cert-tx-write",
    );
    if write.is_error {
        let _ = execute_named_tool(
            service,
            "source.rollback",
            json!({}),
            "cert-tx-rollback-write-error",
        );
        return Err("could not write certification transaction file".into());
    }
    let rollback = execute_named_tool(service, "source.rollback", json!({}), "cert-tx-rollback");
    if rollback.is_error || path.exists() {
        return Err("transaction rollback certification failed".into());
    }
    Ok(json!({"success":true,"path":relative,"rolled_back":true}))
}

fn print_cli_help() {
    println!("Cortex CLI");
    println!();
    println!("Core:");
    println!("  status");
    println!("  doctor [full] [--json]");
    println!("  chat <prompt> [--json|--jsonl]");
    println!("  inspect <prompt> [--json|--jsonl]");
    println!("  plan <request> [--json|--jsonl]");
    println!("  apply <request> [--reuse-transaction] [--json|--jsonl]");
    println!("  repair <request> [--reuse-transaction] [--json|--jsonl]");
    println!("  certify [--json|--jsonl]");
    println!("  serve [port]");
    println!();
    println!("Models:");
    println!("  models");
    println!("  model current [role]");
    println!("  model roles");
    println!("  model set [role] <model-id>");
    println!("  model clear [role]");
    println!("  roles: default, chat, tool, vision, embedding");
    println!();
    println!("Tools:");
    println!("  tools list [--json]");
    println!("  tools describe <name> [--json]");
    println!("  tools certify [--json]");
    println!("  tools test <name> [json-args] [--allow-mutation]");
    println!("  tools test-all");
    println!("  tool <name> [json-args]        (low-level compatibility)");
    println!();
    println!("Build:");
    println!("  build fmt-check|check|test-compile|test|clippy|certify [--json|--jsonl]");
    println!();
    println!("Sessions / logs:");
    println!("  service status|start|stop|restart [--port 7337]");
    println!("  conversation new|list|show|rename|archive");
    println!("  desktop [certify]");
    println!("  workspace status|scan [max_files]|context-status");
    println!("  workspace index [max_files] [max_hash_bytes]|index-status");
    println!("  workspace home|list|attach [path]|activate <id>|detach <id>");
    println!("  review");
    println!("  activity [limit]");
    println!("  tasks [limit]");
    println!("  settings show|set <key> <value>|reset");
    println!("  git status|diff|log|show");
    println!("  vault status|rebuild|embed|search");
    println!("  plugin list|inspect <id>");
    println!("  permissions show|grant|revoke|reset");
    println!("  desktop");
    println!("  chat <prompt> [--conversation <id>] [--json|--jsonl]");
    println!("  session list|latest|show [id|latest]|artifacts [id|latest]");
    println!("  logs list|latest [lines]|tail [file] [lines]");
    println!();
    println!("Transactions:");
    println!("  tx status");
    println!("  tx files");
    println!("  tx begin [label]");
    println!("  tx checkpoint <path>");
    println!(
        "  tx commit [--force]    (project-authoritative verification required unless forced)"
    );
    println!("  tx rollback");
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn configured_provider_kind() -> ProviderKind {
    ProviderKind::parse(&env::var("CORTEX_PROVIDER").unwrap_or_else(|_| "native".into()))
}

fn configured_provider_url(kind: ProviderKind) -> String {
    match kind {
        ProviderKind::Native => env::var("CORTEX_NATIVE_MODEL_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:12400/v1".into()),
        ProviderKind::LmStudio => env_compat("CORTEX_LMSTUDIO_URL", "OPEN2D_LMSTUDIO_URL")
            .unwrap_or_else(|| "http://127.0.0.1:1234/v1".into()),
    }
}

fn configured_provider_timeout() -> Option<Duration> {
    let raw = env::var("CORTEX_PROVIDER_TIMEOUT_SECONDS")
        .ok()
        .or_else(|| {
            env_compat(
                "CORTEX_LMSTUDIO_TIMEOUT_SECONDS",
                "OPEN2D_LMSTUDIO_TIMEOUT_SECONDS",
            )
        });

    let Some(raw) = raw else {
        // Desktop-owned local inference must not be silently unbounded. Explicit
        // off/none/disabled/0 still opts into an unlimited provider response.
        return Some(Duration::from_secs(120));
    };

    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() || matches!(normalized.as_str(), "0" | "off" | "none" | "disabled") {
        return None;
    }
    normalized
        .parse::<u64>()
        .ok()
        .filter(|seconds| *seconds > 0)
        .map(|seconds| Duration::from_secs(seconds.clamp(30, 86_400)))
}

fn configured_provider(
    kind: ProviderKind,
    base_url: impl Into<String>,
    model: Option<String>,
    timeout: Option<Duration>,
) -> CortexProvider {
    CortexProvider::new(kind, base_url, model, timeout)
}

fn configured_agent_tool_result_bytes() -> usize {
    env_compat(
        "CORTEX_AGENT_TOOL_RESULT_BYTES",
        "OPEN2D_CORTEX_AGENT_TOOL_RESULT_BYTES",
    )
    .and_then(|value| value.parse::<usize>().ok())
    .unwrap_or(12 * 1024)
    .clamp(4 * 1024, 256 * 1024)
}

fn configured_agent_deadline_seconds() -> u64 {
    let Some(raw) = env_compat(
        "CORTEX_TOTAL_TIMEOUT_SECONDS",
        "OPEN2D_CORTEX_TOTAL_TIMEOUT_SECONDS",
    ) else {
        return 0;
    };
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() || matches!(normalized.as_str(), "0" | "off" | "none" | "disabled") {
        return 0;
    }
    normalized
        .parse::<u64>()
        .ok()
        .filter(|seconds| *seconds > 0)
        .map(|seconds| seconds.clamp(60, 86_400))
        .unwrap_or(0)
}

fn configured_agent_iterations() -> usize {
    env_compat("CORTEX_MAX_ITERATIONS", "OPEN2D_CORTEX_MAX_ITERATIONS")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(8)
        .clamp(1, 24)
}

fn print_request_header(
    request_kind: &str,
    model: &str,
    mode: AgentMode,
    provider_timeout: Option<Duration>,
    total_deadline_seconds: Option<u64>,
    grounding: &str,
) {
    eprintln!();
    eprintln!("CORTEX {request_kind}");
    eprintln!("Provider  : Cortex provider router");
    eprintln!("Model     : {model}");
    eprintln!("Mode      : {}", mode_name(mode));
    match provider_timeout {
        Some(timeout) => eprintln!(
            "HTTP limit: {} seconds per provider response (explicit watchdog)",
            timeout.as_secs()
        ),
        None => eprintln!("HTTP limit: disabled for local inference"),
    }
    if let Some(seconds) = total_deadline_seconds.filter(|seconds| *seconds > 0) {
        eprintln!("Loop limit: {seconds} seconds total project-agent budget");
    } else {
        eprintln!("Loop limit: disabled");
    }
    eprintln!("Grounding : {grounding}");
    eprintln!("Cancel    : Ctrl+C");
    eprintln!();
}

fn run_with_progress<T, F>(
    label: &'static str,
    advertised_limit: Option<Duration>,
    output: OutputMode,
    work: F,
) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String>,
{
    let done = Arc::new(AtomicBool::new(false));
    let ticker_done = Arc::clone(&done);
    let ticker = thread::spawn(move || {
        let started = Instant::now();
        let mut last_reported = 0u64;
        while !ticker_done.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_secs(1));
            let elapsed = started.elapsed().as_secs();
            if elapsed >= last_reported + 10 && !ticker_done.load(Ordering::Relaxed) {
                last_reported = elapsed;
                match output {
                    OutputMode::Human => match advertised_limit {
                        Some(limit) => eprintln!(
                            "Cortex {label} active... elapsed {elapsed}s / explicit watchdog {}s",
                            limit.as_secs()
                        ),
                        None => eprintln!(
                            "Cortex {label} active... elapsed {elapsed}s (no local inference deadline)"
                        ),
                    },
                    OutputMode::Json => {}
                    OutputMode::Jsonl => emit_event(
                        output,
                        "progress",
                        json!({
                            "label":label,
                            "elapsed_seconds":elapsed,
                            "advertised_limit_seconds":advertised_limit.map(|value| value.as_secs())
                        }),
                    ),
                }
            }
        }
    });

    let result = work();
    done.store(true, Ordering::Relaxed);
    let _ = ticker.join();
    result
}

struct CortexService {
    engine: CortexEngine,
    rpc_token: String,
    workspace_root: PathBuf,
}

impl RpcHandler for CortexService {
    fn handle(&mut self, request: RpcRequest) -> RpcResponse {
        let id = request.id.clone();
        if request.auth_token.as_deref() != Some(self.rpc_token.as_str()) {
            return RpcResponse::error(id, "Cortex RPC authentication failed");
        }
        match request.method.as_str() {
            "health" => RpcResponse::ok(
                id,
                json!({
                    "service": "cortex",
                    "status": "ready",
                    "desktop_api_revision": CORTEX_DESKTOP_API_REVISION,
                    "streaming_rpc": true,
                    "provider_status": true,
                    "workspace_root": self.workspace_root.display().to_string()
                }),
            ),
            "tools.list" => {
                RpcResponse::ok(id, json!({"tools": self.engine.tools().definitions()}))
            }
            "tools.call" => {
                let name = request
                    .params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let arguments = request
                    .params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let result = self.engine.tools_mut().execute(&ToolCall {
                    call_id: request.id.clone(),
                    name: name.into(),
                    arguments,
                });
                if result.is_error {
                    RpcResponse::error(
                        id,
                        result
                            .output
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("tool failed"),
                    )
                } else {
                    RpcResponse::ok(id, result.output)
                }
            }
            "models.list" => match self.engine.provider().list_models() {
                Ok(models) => RpcResponse::ok(id, json!({"models": models})),
                Err(error) => RpcResponse::error(id, error.to_string()),
            },
            "provider.status" => RpcResponse::ok(id, self.engine.provider().status_value()),
            "provider.tool_smoke" => {
                let definitions = self.engine.tools().definitions();
                let model = match self.engine.provider().resolve_model() {
                    Ok(model) => model,
                    Err(error) => return RpcResponse::error(id, error.to_string()),
                };
                match run_provider_tool_smoke(self.engine.provider(), &model, &definitions) {
                    Ok(value) => RpcResponse::ok(id, value),
                    Err(error) => RpcResponse::error(id, error),
                }
            }
            "agent.ask" | "agent.chat" => {
                let Some(prompt) = request.params.get("prompt").and_then(Value::as_str) else {
                    return RpcResponse::error(id, "agent.chat requires prompt");
                };
                match self.engine.chat(prompt) {
                    Ok(result) => {
                        RpcResponse::ok(id, serde_json::to_value(result).unwrap_or(Value::Null))
                    }
                    Err(error) => RpcResponse::error(id, error),
                }
            }
            "agent.inspect" | "agent.plan" => {
                let Some(prompt) = request.params.get("prompt").and_then(Value::as_str) else {
                    return RpcResponse::error(id, "read-only project agent requires prompt");
                };
                self.engine.set_mode(AgentMode::Observe);
                match self.engine.run(prompt) {
                    Ok(result) => {
                        RpcResponse::ok(id, serde_json::to_value(result).unwrap_or(Value::Null))
                    }
                    Err(error) => RpcResponse::error(id, error),
                }
            }
            "agent.apply" | "agent.repair" => {
                let Some(prompt) = request.params.get("prompt").and_then(Value::as_str) else {
                    return RpcResponse::error(id, "mutating project agent requires prompt");
                };
                let action = if request.method == "agent.apply" {
                    "apply"
                } else {
                    "repair"
                };
                let reuse_transaction = request
                    .params
                    .get("reuse_transaction")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let transaction_id =
                    match ensure_active_transaction(self, action, reuse_transaction) {
                        Ok(transaction_id) => transaction_id,
                        Err(error) => return RpcResponse::error(id, error),
                    };

                self.engine.set_mode(AgentMode::WorkspaceAutonomy);
                self.engine.set_tool_profile(if action == "repair" {
                    AgentToolProfile::Repair
                } else {
                    AgentToolProfile::General
                });
                let effective_prompt = if action == "apply" {
                    format!(
                        "An active durable Cortex transaction is already open. Do not begin another transaction and do not commit or roll it back. You have authoritative project source inspection and project-file mutation tools in this mode. Never tell the user to edit files manually or claim that you cannot edit/save project files. Apply the requested bounded changes using source tools, then run build.cargo_check before finishing. Leave the transaction active for human review.\n\nUser request: {prompt}"
                    )
                } else {
                    format!(
                        "An active durable Cortex transaction is already open and the deterministic Repair Coordinator owns transaction lifecycle, dependency grounding, quality-gate execution, rollback, commit, and completion authority. Do not begin/commit/rollback transactions and do not run build/development orchestration tools. Review the supplied repair evidence, inspect only source needed to understand it, and make one coherent bounded candidate repair with the offered source-edit tools. Never tell the user to edit files manually. Leave verification to the controller.\n\nRepair request: {prompt}"
                    )
                };

                let repair_turn = self.engine.run(&effective_prompt);
                self.engine.set_tool_profile(AgentToolProfile::General);
                match repair_turn {
                    Ok(result) if has_successful_project_file_mutation(&result) => RpcResponse::ok(
                        id,
                        json!({
                            "transaction_id": transaction_id,
                            "agent": result
                        }),
                    ),
                    Ok(result) => {
                        if !reuse_transaction {
                            let _ = execute_named_tool(
                                self,
                                "source.rollback",
                                json!({}),
                                "rpc-empty-mutation-rollback",
                            );
                        }
                        RpcResponse::error(
                            id,
                            format!(
                                "Cortex {action} completed without a successful project-file mutation tool call. Prose, code snippets, or suggested edits were rejected; the request is still incomplete. Structured evidence: {}",
                                agent_evidence_summary(&result)
                            ),
                        )
                    }
                    Err(error) => {
                        // H54: provider/tool failures must not strand an empty transaction
                        // that blocks the next natural-language Developer request. Preserve
                        // transactions that already touched project files so the next Repair/
                        // Continue pass can resume them; roll back only empty fresh attempts.
                        if !reuse_transaction {
                            rollback_empty_transaction_after_failed_agent(self);
                        }
                        RpcResponse::error(id, error)
                    }
                }
            }
            "session.info" => RpcResponse::ok(
                id,
                json!({
                    "id": self.engine.tools().session().id,
                    "directory": self.engine.tools().session().directory
                }),
            ),
            _ => RpcResponse::error(id, format!("unknown RPC method: {}", request.method)),
        }
    }

    fn handle_stream(
        &mut self,
        request: RpcRequest,
        emit: &mut dyn FnMut(CortexStreamEvent) -> Result<(), String>,
    ) -> RpcResponse {
        let id = request.id.clone();
        if request.auth_token.as_deref() != Some(self.rpc_token.as_str()) {
            return RpcResponse::error(id, "Cortex RPC authentication failed");
        }
        if matches!(
            request.method.as_str(),
            "agent.inspect.stream"
                | "agent.plan.stream"
                | "agent.apply.stream"
                | "agent.repair.stream"
        ) {
            let started = Instant::now();
            let base_method = request
                .method
                .strip_suffix(".stream")
                .unwrap_or(request.method.as_str())
                .to_string();
            let mut base_request = request;
            base_request.method = base_method.clone();

            let _ = emit(CortexStreamEvent::status(
                id.clone(),
                CortexStreamKind::Started,
                "agent_request",
                format!("Starting long-running Cortex {base_method} request"),
                0,
            ));

            return thread::scope(|scope| {
                let (tx, rx) = std::sync::mpsc::sync_channel::<RpcResponse>(1);
                scope.spawn(move || {
                    let response = self.handle(base_request);
                    let _ = tx.send(response);
                });

                loop {
                    match rx.recv_timeout(Duration::from_secs(5)) {
                        Ok(response) => {
                            let kind = if response.ok {
                                CortexStreamKind::Completed
                            } else {
                                CortexStreamKind::Failed
                            };
                            let phase = if response.ok { "complete" } else { "failed" };
                            let detail = if response.ok {
                                format!("Cortex {base_method} request completed")
                            } else {
                                format!("Cortex {base_method} request failed")
                            };
                            let _ = emit(CortexStreamEvent::status(
                                id.clone(),
                                kind,
                                phase,
                                detail,
                                started.elapsed().as_millis() as u64,
                            ));
                            return response;
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            let _ = emit(CortexStreamEvent::status(
                                id.clone(),
                                CortexStreamKind::Status,
                                "agent_active",
                                "Cortex project agent is still executing inside the service",
                                started.elapsed().as_millis() as u64,
                            ));
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            return RpcResponse::error(
                                id.clone(),
                                "Cortex project-agent worker disconnected before returning a response",
                            );
                        }
                    }
                }
            });
        }

        if request.method != "agent.chat.stream" {
            return self.handle(request);
        }
        let Some(prompt) = request.params.get("prompt").and_then(Value::as_str) else {
            return RpcResponse::error(id, "agent.chat.stream requires prompt");
        };

        let started = Instant::now();
        let _ = emit(CortexStreamEvent::status(
            id.clone(),
            CortexStreamKind::Started,
            "preparing",
            "Preparing Cortex chat context",
            0,
        ));
        let _ = emit(CortexStreamEvent::status(
            id.clone(),
            CortexStreamKind::Status,
            "provider_request",
            "Querying Cortex provider",
            started.elapsed().as_millis() as u64,
        ));

        let result = self.engine.chat_stream(prompt, &mut |delta| {
            let _ = emit(CortexStreamEvent::text_delta(
                id.clone(),
                delta,
                started.elapsed().as_millis() as u64,
            ));
        });

        match result {
            Ok(result) => {
                let _ = emit(CortexStreamEvent::status(
                    id.clone(),
                    CortexStreamKind::Completed,
                    "complete",
                    "Response complete",
                    started.elapsed().as_millis() as u64,
                ));
                RpcResponse::ok(id, serde_json::to_value(result).unwrap_or(Value::Null))
            }
            Err(error) => {
                let _ = emit(CortexStreamEvent::status(
                    id.clone(),
                    CortexStreamKind::Failed,
                    "failed",
                    error.clone(),
                    started.elapsed().as_millis() as u64,
                ));
                RpcResponse::error(id, error)
            }
        }
    }
}

fn agent_mode_from_env() -> AgentMode {
    match env_compat("CORTEX_MODE", "OPEN2D_CORTEX_MODE")
        .unwrap_or_else(|| "workspace_autonomy".into())
        .to_ascii_lowercase()
        .as_str()
    {
        "observe" => AgentMode::Observe,
        "guided" => AgentMode::Guided,
        _ => AgentMode::WorkspaceAutonomy,
    }
}

fn mode_name(mode: AgentMode) -> &'static str {
    match mode {
        AgentMode::Observe => "Observe",
        AgentMode::Guided => "Guided",
        AgentMode::WorkspaceAutonomy => "Workspace Autonomy",
    }
}

fn extract_global_workspace(args: Vec<String>) -> Result<(Option<PathBuf>, Vec<String>), String> {
    let mut workspace = None;
    let mut filtered = Vec::new();
    let mut index = 0usize;
    while index < args.len() {
        if args[index] == "--workspace" {
            let value = args
                .get(index + 1)
                .ok_or_else(|| "--workspace requires a path".to_string())?;
            workspace = Some(PathBuf::from(value));
            index += 2;
        } else {
            filtered.push(args[index].clone());
            index += 1;
        }
    }
    Ok((workspace, filtered))
}

fn find_workspace_root(start: PathBuf, override_root: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(root) = override_root {
        return fs::canonicalize(root).map_err(|error| error.to_string());
    }
    if let Some(value) = env_compat("CORTEX_WORKSPACE", "OPEN2D_CORTEX_WORKSPACE") {
        return fs::canonicalize(value).map_err(|error| error.to_string());
    }

    let original = fs::canonicalize(start).map_err(|error| error.to_string())?;
    let mut cursor = original.clone();
    loop {
        if is_workspace_marker(&cursor) {
            return Ok(cursor);
        }
        if !cursor.pop() {
            return Ok(original);
        }
    }
}

fn is_workspace_marker(path: &Path) -> bool {
    path.join("project.control.json").is_file()
        || path.join(".cortex").exists()
        || path.join(".git").exists()
        || path.join("Cargo.toml").is_file()
        || path.join("package.json").is_file()
        || path.join("pyproject.toml").is_file()
        || path.join("CMakeLists.txt").is_file()
        || path
            .join("config")
            .join("architecture")
            .join("foundry_dependency_policy.json")
            .is_file()
}

fn open_registered_workspace(project_root: &Path) -> Result<Workspace, String> {
    let bootstrap = Workspace::open(project_root).map_err(|error| error.to_string())?;
    let Ok(registry) = WorkspaceRegistry::open_default() else {
        return Ok(bootstrap);
    };
    let _ = registry.ensure_cortex_self_registered();
    match registry.attach(&bootstrap) {
        Ok(record) => registry.open_workspace(&record),
        Err(error) => {
            eprintln!("Cortex workspace registry warning: {error}");
            Ok(bootstrap)
        }
    }
}

fn cortex_state_dir(project_root: &Path) -> PathBuf {
    open_registered_workspace(project_root)
        .map(|workspace| workspace.cortex_state_dir())
        .unwrap_or_else(|_| project_root.join(".cortex"))
}

fn env_compat(primary: &str, legacy: &str) -> Option<String> {
    env::var(primary)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var(legacy)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
}

fn ensure_rpc_token(project_root: &Path) -> Result<String, String> {
    let dir = cortex_state_dir(project_root);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("rpc.token");
    if let Ok(existing) = fs::read_to_string(&path) {
        let token = existing.trim();
        if token.len() >= 24 {
            return Ok(token.to_string());
        }
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let token = format!("{:x}{:x}{:x}", now, std::process::id(), now.rotate_left(37));
    fs::write(&path, token.as_bytes()).map_err(|e| e.to_string())?;
    Ok(token)
}

#[cfg(test)]
mod mutation_guard_tests {
    use super::*;
    use cortex_core::AgentToolEvidence;

    fn result_with(tool: &str, mutating: bool, is_error: bool) -> AgentTurnResult {
        AgentTurnResult {
            text: "done".into(),
            iterations: 1,
            tool_results: Vec::new(),
            evidence: vec![AgentToolEvidence {
                tool: tool.into(),
                call_id: "call-1".into(),
                mutating,
                is_error,
            }],
            last_response_id: None,
        }
    }

    #[test]
    fn prose_or_non_file_tools_do_not_satisfy_mutating_agent_contract() {
        let prose_only = AgentTurnResult {
            text: "paste this snippet into src/main.rs".into(),
            iterations: 1,
            tool_results: Vec::new(),
            evidence: Vec::new(),
            last_response_id: None,
        };
        assert!(!has_successful_project_file_mutation(&prose_only));
        assert!(!has_successful_project_file_mutation(&result_with(
            "build.cargo_check",
            false,
            false,
        )));
        assert!(!has_successful_project_file_mutation(&result_with(
            "source.write_text",
            true,
            true,
        )));
    }

    #[test]
    fn w11_repair_preflight_does_not_treat_standalone_cargo_as_empty() {
        let root =
            std::env::temp_dir().join(format!("cortex-w11-preflight-{}", std::process::id()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname=\"fixture\"\n").unwrap();
        let evidence = repair_project_preflight(&root).unwrap();
        assert_eq!(evidence.get("root_enumeration"), Some(&json!("verified")));
        assert_eq!(evidence.get("src_directory_exists"), Some(&json!(true)));
        assert_eq!(project_validation_tool(&root), "build.project_validate");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn w11_repair_preflight_fails_closed_before_transaction_on_unknown_root() {
        let root = std::env::temp_dir().join(format!("cortex-w11-unknown-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        assert!(repair_project_preflight(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn w11_project_local_pcc_overrides_generic_cargo_validation() {
        let root = std::env::temp_dir().join(format!("cortex-w11-gate-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname=\"fixture\"\n").unwrap();
        fs::write(root.join("PCC.cmd"), "@echo off\n").unwrap();
        fs::write(root.join("ProjectControlCenter.py"), "# fixture\n").unwrap();
        assert_eq!(project_validation_tool(&root), "build.project_checkpoint");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn successful_project_file_tools_satisfy_mutating_agent_contract() {
        for tool in [
            "source.write_text",
            "source.replace_text",
            "vscode.apply_workspace_edit",
            "image.promote",
        ] {
            assert!(
                has_successful_project_file_mutation(&result_with(tool, true, false)),
                "{tool} should count as an authoritative project-file mutation"
            );
        }
    }
}
