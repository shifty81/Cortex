//! Cortex agent orchestration independent of Foundry, VS Code and model host.

use cortex_protocol::{
    MessageRole, ModelMessage, ModelRequest, ModelResponse, TextProvider, ToolCall,
    ToolChoicePolicy, ToolDefinition, ToolExecutor, ToolResultInput,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const AUTHORITATIVE_PREFLIGHT_MAX_BYTES: usize = 6 * 1024;
const STATELESS_REPLAY_MAX_CHARS: usize = 12 * 1024;
const MUTATION_ONLY_AFTER_TOOL_ROUNDS: usize = 3;
const REPAIR_MUTATION_ONLY_AFTER_TOOL_ROUNDS: usize = 1;
const REPAIR_PRE_MUTATION_OBSERVATION_CALL_LIMIT: usize = 2;
const REQUIRED_TOOL_PROTOCOL_RECOVERY_LIMIT: usize = 2;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    Observe,
    Guided,
    WorkspaceAutonomy,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolProfile {
    #[default]
    General,
    Repair,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSettings {
    pub mode: AgentMode,
    #[serde(default)]
    pub tool_profile: AgentToolProfile,
    pub max_tool_iterations: usize,
    pub max_elapsed_seconds: u64,
    pub max_tool_result_bytes: usize,
    pub system_instructions: String,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            mode: AgentMode::Guided,
            tool_profile: AgentToolProfile::General,
            max_tool_iterations: 8,
            max_elapsed_seconds: 0,
            max_tool_result_bytes: 12 * 1024,
            system_instructions: default_system_instructions(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
            .or_else(|| {
                result
                    .output
                    .pointer("/candidate/decision")
                    .and_then(Value::as_str)
            })
            .filter(|decision| {
                matches!(
                    *decision,
                    "keep_verified"
                        | "keep_improved"
                        | "rollback_unchanged"
                        | "rollback_regressed"
                        | "observe_without_baseline"
                        | "no_active_candidate"
                )
            })
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
    if candidate.is_empty()
        || candidate.len() > 240
        || candidate.contains(':')
        || candidate.starts_with('/')
        || candidate.starts_with('\\')
        || candidate.chars().any(|ch| ch.is_control())
    {
        return None;
    }
    let normalized = candidate.replace('\\', "/");
    if normalized
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }
    Some(normalized)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentTurnResult {
    pub text: String,
    pub iterations: usize,
    pub tool_results: Vec<ToolResultInput>,
    pub evidence: Vec<AgentToolEvidence>,
    pub last_response_id: Option<String>,
}

pub struct AgentEngine<P, T> {
    provider: P,
    tools: T,
    settings: AgentSettings,
}

impl<P, T> AgentEngine<P, T>
where
    P: TextProvider,
    T: ToolExecutor,
{
    pub fn new(provider: P, tools: T, settings: AgentSettings) -> Self {
        Self {
            provider,
            tools,
            settings,
        }
    }

    pub fn provider(&self) -> &P {
        &self.provider
    }
    pub fn tools(&self) -> &T {
        &self.tools
    }
    pub fn tools_mut(&mut self) -> &mut T {
        &mut self.tools
    }

    pub fn set_mode(&mut self, mode: AgentMode) {
        self.settings.mode = mode;
    }

    pub fn set_tool_profile(&mut self, profile: AgentToolProfile) {
        self.settings.tool_profile = profile;
    }

    pub fn settings(&self) -> &AgentSettings {
        &self.settings
    }

    pub fn chat(&self, prompt: &str) -> Result<AgentTurnResult, String> {
        let mut request = ModelRequest::user(prompt);
        request.instructions = Some(default_chat_instructions());

        let response = self
            .provider
            .respond_stream(&request, &mut |_| {})
            .map_err(|e| e.to_string())?;
        if !response.tool_calls.is_empty() {
            return Err(
                "Cortex chat received an unexpected tool call even though no tools were offered"
                    .into(),
            );
        }
        if response.output_text.trim().is_empty() {
            return Err(format!(
                "Cortex provider completed chat without any user-visible text (response id: {:?}). The request was not accepted as a successful blank reply.",
                response.id
            ));
        }

        Ok(AgentTurnResult {
            text: response.output_text,
            iterations: 1,
            tool_results: Vec::new(),
            evidence: Vec::new(),
            last_response_id: response.id,
        })
    }

    pub fn chat_stream(
        &self,
        prompt: &str,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<AgentTurnResult, String> {
        let mut request = ModelRequest::user(prompt);
        request.instructions = Some(default_chat_instructions());

        let response = self
            .provider
            .respond_stream(&request, on_delta)
            .map_err(|e| e.to_string())?;
        if !response.tool_calls.is_empty() {
            return Err(
                "Cortex chat received an unexpected tool call even though no tools were offered"
                    .into(),
            );
        }
        if response.output_text.trim().is_empty() {
            return Err(format!(
                "Cortex provider completed streaming chat without any user-visible text (response id: {:?}). The request was not accepted as a successful blank reply.",
                response.id
            ));
        }

        Ok(AgentTurnResult {
            text: response.output_text,
            iterations: 1,
            tool_results: Vec::new(),
            evidence: Vec::new(),
            last_response_id: response.id,
        })
    }

    pub fn run(&mut self, prompt: &str) -> Result<AgentTurnResult, String> {
        let sanitized_prompt = sanitize_internal_transport_transcript(prompt);
        let definitions = self
            .tools
            .definitions()
            .into_iter()
            .filter(|tool| !is_controller_only_tool(&tool.name))
            .filter(|tool| match self.settings.tool_profile {
                AgentToolProfile::General => true,
                AgentToolProfile::Repair => is_repair_model_tool(&tool.name),
            })
            .filter(|tool| match self.settings.mode {
                AgentMode::Observe | AgentMode::Guided => !tool.mutating,
                AgentMode::WorkspaceAutonomy => true,
            })
            .collect::<Vec<_>>();
        let mut collected_results = Vec::new();
        let mut collected_evidence = Vec::new();
        let (grounded_prompt, authoritative_preflight) = self.seed_authoritative_preflight(
            &sanitized_prompt,
            &definitions,
            &mut collected_results,
            &mut collected_evidence,
        );
        let requires_project_file_mutation = self.settings.mode == AgentMode::WorkspaceAutonomy
            && definitions
                .iter()
                .any(|tool| is_project_file_mutation_tool(&tool.name));
        let mutation_definitions = definitions
            .iter()
            .filter(|tool| is_project_file_mutation_tool(&tool.name))
            .cloned()
            .collect::<Vec<_>>();
        let mut successful_project_file_mutation = false;
        let mut completed_tool_rounds = 0usize;
        let mut required_tool_protocol_misses = 0usize;
        let mut dependency_grounding_recovery_active = false;

        let supports_response_chaining = self.provider.supports_previous_response_id();
        let mut stateless_messages = vec![ModelMessage {
            role: MessageRole::User,
            content: grounded_prompt.clone(),
        }];
        let mut request = ModelRequest::user(grounded_prompt);
        request.instructions = Some(self.settings.system_instructions.clone());
        request.tools = definitions.clone();
        request.tool_choice = if requires_project_file_mutation {
            ToolChoicePolicy::Required
        } else {
            ToolChoicePolicy::Auto
        };

        let mut last_response: Option<ModelResponse> = None;
        let started = Instant::now();
        let max_elapsed = (self.settings.max_elapsed_seconds > 0)
            .then(|| Duration::from_secs(self.settings.max_elapsed_seconds));

        for iteration in 1..=self.settings.max_tool_iterations {
            if max_elapsed.is_some_and(|limit| started.elapsed() >= limit) {
                return Err(format!(
                    "Cortex project-agent deadline exceeded after {} seconds before iteration {}",
                    self.settings.max_elapsed_seconds, iteration
                ));
            }

            let response = self
                .provider
                .respond_stream(&request, &mut |_| {})
                .map_err(|e| e.to_string())?;

            if max_elapsed.is_some_and(|limit| started.elapsed() >= limit) {
                return Err(format!(
                    "Cortex project-agent deadline exceeded after {} seconds during iteration {}",
                    self.settings.max_elapsed_seconds, iteration
                ));
            }
            let response_id = response.id.clone();
            if response.tool_calls.is_empty() {
                if requires_project_file_mutation && !successful_project_file_mutation {
                    required_tool_protocol_misses = required_tool_protocol_misses.saturating_add(1);
                    if required_tool_protocol_misses <= REQUIRED_TOOL_PROTOCOL_RECOVERY_LIMIT
                        && iteration < self.settings.max_tool_iterations
                    {
                        let next_iteration = iteration.saturating_add(1);
                        let recovery_tools = if dependency_grounding_recovery_active {
                            dependency_grounding_recovery_tools(&definitions)
                        } else {
                            mutation_definitions.clone()
                        };
                        let recovery_instructions = if dependency_grounding_recovery_active {
                            dependency_grounding_recovery_instructions(
                                &self.settings.system_instructions,
                                next_iteration,
                                self.settings.max_tool_iterations,
                            )
                        } else {
                            required_tool_protocol_recovery_instructions(
                                &self.settings.system_instructions,
                                next_iteration,
                                self.settings.max_tool_iterations,
                                &response.output_text,
                            )
                        };
                        if supports_response_chaining {
                            request = ModelRequest {
                                model: request.model.clone(),
                                instructions: Some(recovery_instructions),
                                messages: Vec::new(),
                                tools: recovery_tools.clone(),
                                tool_choice: ToolChoicePolicy::Required,
                                previous_response_id: response_id,
                                tool_results: Vec::new(),
                                images: Vec::new(),
                            };
                        } else {
                            stateless_messages.push(ModelMessage {
                                role: MessageRole::User,
                                content: if dependency_grounding_recovery_active {
                                    dependency_grounding_recovery_message()
                                } else {
                                    required_tool_protocol_recovery_message(&response.output_text)
                                },
                            });
                            compact_stateless_messages(
                                &mut stateless_messages,
                                STATELESS_REPLAY_MAX_CHARS,
                            );
                            request = ModelRequest {
                                model: request.model.clone(),
                                instructions: Some(recovery_instructions),
                                messages: stateless_messages.clone(),
                                tools: recovery_tools,
                                tool_choice: ToolChoicePolicy::Required,
                                previous_response_id: None,
                                tool_results: Vec::new(),
                                images: Vec::new(),
                            };
                        }
                        last_response = Some(response);
                        continue;
                    }
                    return Err(format!(
                        "Cortex provider failed the required structured-tool contract after {required_tool_protocol_misses} bounded recovery attempt(s) at iteration {iteration}. The response contained text instead of an executable project-file mutation call; no prose-only implementation was accepted. {}",
                        required_tool_protocol_failure_summary(&response.output_text)
                    ));
                }
                if iteration == 1 && !definitions.is_empty() && !authoritative_preflight {
                    let preview = response.output_text.chars().take(320).collect::<String>();
                    return Err(format!(
                        "Cortex rejected an ungrounded model response: no authoritative workspace preflight was available and the provider/model did not emit a structured tool call on the first turn. No claimed project inspection was accepted. Model text preview: {preview}"
                    ));
                }

                if response.output_text.trim().is_empty() {
                    let previous_response_id = if supports_response_chaining {
                        Some(response_id.clone().ok_or_else(|| {
                            "Cortex project agent completed without user-visible text and supplied no response id for a recovery synthesis turn. The blank result was rejected instead of being shown as success.".to_string()
                        })?)
                    } else {
                        None
                    };
                    let recovery_request = ModelRequest {
                        model: request.model.clone(),
                        instructions: Some(format!(
                            "{}

Your previous project-agent response completed without any user-visible final text. Do not call tools. Produce the concise grounded final answer for the user now, using the evidence and context already available in this request. Never return an empty final answer.",
                            self.settings.system_instructions
                        )),
                        messages: if supports_response_chaining {
                            Vec::new()
                        } else {
                            request.messages.clone()
                        },
                        tools: Vec::new(),
                        tool_choice: ToolChoicePolicy::None,
                        previous_response_id,
                        tool_results: Vec::new(),
                        images: Vec::new(),
                    };
                    let recovered = self.provider.respond(&recovery_request).map_err(|error| {
                        format!("Cortex empty-response recovery synthesis failed: {error}")
                    })?;
                    if !recovered.tool_calls.is_empty() {
                        return Err(
                            "Cortex empty-response recovery unexpectedly returned tool calls after tools were removed."
                                .into(),
                        );
                    }
                    if recovered.output_text.trim().is_empty() {
                        return Err(format!(
                            "Cortex provider completed both the project-agent turn and its recovery synthesis without user-visible text (response id: {:?}). The blank reply was rejected.",
                            recovered.id
                        ));
                    }
                    return Ok(AgentTurnResult {
                        text: recovered.output_text,
                        iterations: iteration.saturating_add(1),
                        tool_results: collected_results,
                        evidence: collected_evidence,
                        last_response_id: recovered.id,
                    });
                }

                return Ok(AgentTurnResult {
                    text: response.output_text,
                    iterations: iteration,
                    tool_results: collected_results,
                    evidence: collected_evidence,
                    last_response_id: response_id,
                });
            }

            let mut results = Vec::new();
            let mut repair_observation_calls_this_round = 0usize;
            for call in &response.tool_calls {
                let definition = request.tools.iter().find(|tool| tool.name == call.name);
                let mutating = definition.map(|tool| tool.mutating).unwrap_or(true);
                let repair_observation_over_budget = self.settings.tool_profile
                    == AgentToolProfile::Repair
                    && requires_project_file_mutation
                    && !successful_project_file_mutation
                    && !dependency_grounding_recovery_active
                    && !mutating
                    && repair_observation_calls_this_round
                        >= REPAIR_PRE_MUTATION_OBSERVATION_CALL_LIMIT;
                let result = if repair_observation_over_budget {
                    repair_action_required(call)
                } else {
                    if self.settings.tool_profile == AgentToolProfile::Repair
                        && requires_project_file_mutation
                        && !successful_project_file_mutation
                        && !dependency_grounding_recovery_active
                        && !mutating
                    {
                        repair_observation_calls_this_round =
                            repair_observation_calls_this_round.saturating_add(1);
                    }
                    if self.may_execute(call, &request.tools) {
                        self.tools.execute(call)
                    } else {
                        denied(call, self.settings.mode)
                    }
                };
                let result = bound_tool_result(result, self.settings.max_tool_result_bytes);
                collected_evidence.push(AgentToolEvidence::from_result(call, mutating, &result));
                if is_project_file_mutation_tool(&call.name) && !result.is_error {
                    successful_project_file_mutation = true;
                }
                collected_results.push(result.clone());
                results.push(result);
            }
            let blocked_dependency_packages = dependency_grounding_packages(&results);
            if !blocked_dependency_packages.is_empty() {
                // Ground Before Write is controller-owned recovery. Ground the entire
                // required package set in one deterministic tool round instead of burning
                // one model iteration per dependency.
                let mut batch_grounded = true;
                for (index, package) in blocked_dependency_packages.iter().enumerate() {
                    let grounding_call = ToolCall {
                        call_id: format!("cortex-ground-{iteration}-{index}"),
                        name: "dependency.ground".into(),
                        arguments: json!({"package": package, "ecosystem": "cargo"}),
                    };
                    let grounding_result = bound_tool_result(
                        self.tools.execute(&grounding_call),
                        self.settings.max_tool_result_bytes,
                    );
                    batch_grounded &= !grounding_result.is_error
                        && grounding_result
                            .output
                            .get("grounding_recorded")
                            .and_then(Value::as_bool)
                            == Some(true);
                    collected_evidence.push(AgentToolEvidence::from_result(
                        &grounding_call,
                        false,
                        &grounding_result,
                    ));
                    collected_results.push(grounding_result.clone());
                    results.push(grounding_result);
                }
                dependency_grounding_recovery_active = !batch_grounded;
            } else if tool_results_require_dependency_grounding(&results) {
                dependency_grounding_recovery_active = true;
            } else if dependency_grounding_recovery_active
                && tool_results_complete_dependency_grounding(&results)
            {
                dependency_grounding_recovery_active = false;
            }
            completed_tool_rounds = completed_tool_rounds.saturating_add(1);

            let next_iteration = iteration.saturating_add(1);
            let mutation_only_after_rounds =
                if self.settings.tool_profile == AgentToolProfile::Repair {
                    REPAIR_MUTATION_ONLY_AFTER_TOOL_ROUNDS
                } else {
                    MUTATION_ONLY_AFTER_TOOL_ROUNDS
                };
            let force_mutation_only = requires_project_file_mutation
                && !successful_project_file_mutation
                && completed_tool_rounds >= mutation_only_after_rounds
                && !dependency_grounding_recovery_active;
            let next_tools = if dependency_grounding_recovery_active {
                dependency_grounding_recovery_tools(&definitions)
            } else if force_mutation_only {
                mutation_definitions.clone()
            } else {
                definitions.clone()
            };
            let next_tool_choice = if dependency_grounding_recovery_active
                || (requires_project_file_mutation && !successful_project_file_mutation)
            {
                ToolChoicePolicy::Required
            } else {
                ToolChoicePolicy::Auto
            };
            let next_instructions = if dependency_grounding_recovery_active {
                dependency_grounding_recovery_instructions(
                    &self.settings.system_instructions,
                    next_iteration,
                    self.settings.max_tool_iterations,
                )
            } else if force_mutation_only {
                mutation_required_instructions(
                    &self.settings.system_instructions,
                    next_iteration,
                    self.settings.max_tool_iterations,
                )
            } else {
                project_iteration_instructions(
                    &self.settings.system_instructions,
                    next_iteration,
                    self.settings.max_tool_iterations,
                )
            };
            if supports_response_chaining {
                request = ModelRequest {
                    model: request.model.clone(),
                    instructions: Some(next_instructions.clone()),
                    messages: Vec::new(),
                    tools: next_tools.clone(),
                    tool_choice: next_tool_choice,
                    previous_response_id: response_id,
                    tool_results: results,
                    images: Vec::new(),
                };
            } else {
                append_stateless_tool_exchange(&mut stateless_messages, &response, &results);
                compact_stateless_messages(&mut stateless_messages, STATELESS_REPLAY_MAX_CHARS);
                request = ModelRequest {
                    model: request.model.clone(),
                    instructions: Some(next_instructions),
                    messages: stateless_messages.clone(),
                    tools: next_tools,
                    tool_choice: next_tool_choice,
                    previous_response_id: None,
                    tool_results: Vec::new(),
                    images: Vec::new(),
                };
            }
            last_response = Some(response);
        }

        if max_elapsed.is_some_and(|limit| started.elapsed() >= limit) {
            return Err(format!(
                "Cortex project-agent deadline exceeded after {} seconds before final synthesis",
                self.settings.max_elapsed_seconds
            ));
        }

        if requires_project_file_mutation && !successful_project_file_mutation {
            return Err(format!(
                "Cortex exhausted {} structured-tool iteration(s) without a successful project-file mutation. Required tool choice and the mutation-only phase were enforced; no prose-only implementation was accepted. Evidence: {}",
                self.settings.max_tool_iterations,
                mutation_evidence_summary(&collected_evidence)
            ));
        }

        // Reaching the evidence/tool budget is not itself a project-agent failure.
        // The final tool results are already attached to `request`. Force one
        // no-tools synthesis turn so the provider must summarize the grounded
        // evidence it has collected instead of asking for a ninth tool call.
        let synthesis_request = ModelRequest {
            model: request.model.clone(),
            instructions: Some(final_synthesis_instructions(
                &self.settings.system_instructions,
                self.settings.max_tool_iterations,
            )),
            messages: if supports_response_chaining {
                Vec::new()
            } else {
                request.messages.clone()
            },
            tools: Vec::new(),
            tool_choice: ToolChoicePolicy::None,
            previous_response_id: if supports_response_chaining {
                request.previous_response_id.clone()
            } else {
                None
            },
            tool_results: if supports_response_chaining {
                request.tool_results.clone()
            } else {
                Vec::new()
            },
            images: Vec::new(),
        };
        let response = self
            .provider
            .respond_stream(&synthesis_request, &mut |_| {})
            .map_err(|error| {
                format!(
                    "Cortex exhausted {} tool iterations and final evidence synthesis failed: {}. Last response id: {:?}",
                    self.settings.max_tool_iterations,
                    error,
                    last_response.as_ref().and_then(|response| response.id.clone())
                )
            })?;

        if !response.tool_calls.is_empty() {
            return Err(format!(
                "Cortex final synthesis unexpectedly returned tool calls after the tool catalog was removed; last response id: {:?}",
                response.id
            ));
        }
        if response.output_text.trim().is_empty() {
            return Err(format!(
                "Cortex final evidence synthesis completed without user-visible text (response id: {:?}). The blank reply was rejected.",
                response.id
            ));
        }

        Ok(AgentTurnResult {
            text: response.output_text,
            iterations: self.settings.max_tool_iterations.saturating_add(1),
            tool_results: collected_results,
            evidence: collected_evidence,
            last_response_id: response.id,
        })
    }

    fn seed_authoritative_preflight(
        &mut self,
        prompt: &str,
        definitions: &[ToolDefinition],
        collected_results: &mut Vec<ToolResultInput>,
        collected_evidence: &mut Vec<AgentToolEvidence>,
    ) -> (String, bool) {
        let Some(definition) = definitions.iter().find(|tool| {
            !tool.mutating && (tool.name == "workspace.status" || tool.name == "project.status")
        }) else {
            return (prompt.to_string(), false);
        };

        let call = ToolCall {
            call_id: "cortex-authoritative-workspace-preflight".into(),
            name: definition.name.clone(),
            arguments: json!({}),
        };
        let raw_result = self.tools.execute(&call);
        let compact_output = if raw_result.output.is_object() {
            json!({
                "inspection_snapshot": raw_result
                    .output
                    .get("inspection_snapshot")
                    .cloned()
                    .unwrap_or(Value::Null),
                "dependency_grounding": raw_result
                    .output
                    .get("dependency_grounding")
                    .cloned()
                    .unwrap_or(Value::Null),
                "dependency_grounding_state": raw_result
                    .output
                    .get("dependency_grounding_state")
                    .cloned()
                    .unwrap_or(Value::Null),
                "verification_authority": raw_result
                    .output
                    .get("verification_authority")
                    .cloned()
                    .unwrap_or(Value::Null),
                "active_transaction": raw_result
                    .output
                    .get("active_transaction")
                    .cloned()
                    .unwrap_or(Value::Null)
            })
        } else {
            raw_result.output.clone()
        };
        let result = bound_tool_result(
            ToolResultInput {
                call_id: raw_result.call_id,
                output: compact_output,
                is_error: raw_result.is_error,
            },
            self.settings
                .max_tool_result_bytes
                .min(AUTHORITATIVE_PREFLIGHT_MAX_BYTES),
        );

        collected_evidence.push(AgentToolEvidence::from_result(&call, false, &result));
        collected_results.push(result.clone());

        if result.is_error {
            return (prompt.to_string(), false);
        }

        let serialized = serde_json::to_string(&result.output)
            .unwrap_or_else(|_| "{\"preflight\":\"unserializable\"}".into());

        (
            format!(
                "{prompt}\n\n[CORTEX AUTHORITATIVE WORKSPACE PREFLIGHT]\nThis evidence was collected directly by Cortex before the model turn. Treat it as authoritative for workspace membership, project inventory, exact dependency resolution, and verification state. Do not replace these facts with counts inferred from directory listings or with remembered dependency APIs. If verification_authority is not HEALTHY, do not claim the project is healthy. When project.profile reports a project-native checkpoint, that checkpoint is the highest project-health authority and generated per-stage checks are subordinate. Treat checkpoint_authority.protected_paths as infrastructure: do not weaken or rewrite them unless the user's task explicitly targets checkpoint/build infrastructure. If an API-sensitive mutation references an external dependency, Ground Before Write requires exact local dependency-source evidence through source.search with dependency_package before mutation.\n{serialized}\n[END CORTEX AUTHORITATIVE WORKSPACE PREFLIGHT]"
            ),
            true,
        )
    }

    fn may_execute(&self, call: &ToolCall, definitions: &[ToolDefinition]) -> bool {
        let Some(definition) = definitions.iter().find(|tool| tool.name == call.name) else {
            return false;
        };
        match self.settings.mode {
            AgentMode::Observe => !definition.mutating,
            AgentMode::Guided => !definition.mutating,
            AgentMode::WorkspaceAutonomy => true,
        }
    }
}

fn is_controller_only_tool(name: &str) -> bool {
    matches!(
        name,
        "source.commit"
            | "dependency.ground"
            | "dependency.ensure"
            | "build.project_checkpoint"
            | "runtime.verify_project"
    )
}

fn is_repair_model_tool(name: &str) -> bool {
    matches!(
        name,
        "workspace.status"
            | "project.status"
            | "project.profile"
            | "source.list"
            | "source.read"
            | "source.search"
            | "source.transaction_status"
            | "source.transaction_files"
            | "source.write_text"
            | "source.replace_text"
    )
}

fn is_dependency_grounding_recovery_tool(name: &str) -> bool {
    matches!(
        name,
        "workspace.status"
            | "project.status"
            | "project.profile"
            | "source.search"
            | "dependency.ground"
            | "source.read"
            | "source.list"
            | "source.transaction_status"
    )
}

fn dependency_grounding_recovery_tools(definitions: &[ToolDefinition]) -> Vec<ToolDefinition> {
    definitions
        .iter()
        .filter(|tool| is_dependency_grounding_recovery_tool(&tool.name))
        .cloned()
        .collect()
}

fn tool_results_require_dependency_grounding(results: &[ToolResultInput]) -> bool {
    results.iter().any(|result| {
        result.is_error
            && result.output.get("failure_class").and_then(Value::as_str)
                == Some("dependency_grounding")
    })
}

fn dependency_grounding_packages(results: &[ToolResultInput]) -> Vec<String> {
    let mut packages = results
        .iter()
        .filter(|result| result.is_error)
        .filter(|result| {
            result.output.get("failure_class").and_then(Value::as_str)
                == Some("dependency_grounding")
        })
        .flat_map(|result| {
            result
                .output
                .get("missing_dependency_packages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    packages.sort();
    packages.dedup();
    packages
}

fn tool_results_complete_dependency_grounding(results: &[ToolResultInput]) -> bool {
    results.iter().any(|result| {
        !result.is_error
            && result
                .output
                .get("dependency_grounding_complete")
                .and_then(Value::as_bool)
                == Some(true)
    })
}

fn dependency_grounding_recovery_message() -> String {
    "[CORTEX GROUND-BEFORE-WRITE RECOVERY]\nA source mutation was blocked because exact dependency-source grounding is missing. Do not retry the blocked write yet. Call source.search with dependency_package for the required package(s), using the exact local dependency source, then return to the mutation after grounding succeeds.".into()
}

fn dependency_grounding_recovery_instructions(
    base: &str,
    iteration: usize,
    max_iterations: usize,
) -> String {
    format!(
        "{base}\n\nCortex Ground Before Write recovery: the previous API-sensitive mutation was blocked because exact local dependency-source evidence is missing. This recovery lane intentionally re-enables read-only grounding tools even when the normal mutation-only phase would otherwise be active. You MUST call source.search with dependency_package for the required package(s) before retrying any source mutation. Do not guess APIs from memory and do not emit prose instead of the grounding tool call. This is structured-tool iteration {iteration} of {max_iterations}."
    )
}

fn is_project_file_mutation_tool(name: &str) -> bool {
    matches!(
        name,
        "source.write_text"
            | "source.replace_text"
            | "vscode.apply_workspace_edit"
            | "image.promote"
    )
}

fn mutation_evidence_summary(evidence: &[AgentToolEvidence]) -> String {
    if evidence.is_empty() {
        return "none".into();
    }
    evidence
        .iter()
        .map(|item| {
            format!(
                "{}{}{}",
                item.tool,
                if item.mutating { "[write]" } else { "[read]" },
                if item.is_error { "[error]" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn required_tool_protocol_recovery_message(output_text: &str) -> String {
    format!(
        "[CORTEX REQUIRED-TOOL TRANSPORT RECOVERY]\nThe previous model turn returned text instead of an executable structured tool call while tool choice was REQUIRED. Do not quote, repeat, or reconstruct prior [CORTEX STRUCTURED TOOL REQUESTS] or [CORTEX AUTHORITATIVE TOOL RESULTS] blocks. Emit a NEW real structured project-file mutation tool call using one of the tools offered on this turn. Previous text classification: {}.",
        required_tool_protocol_failure_summary(output_text)
    )
}

fn required_tool_protocol_recovery_instructions(
    base: &str,
    iteration: usize,
    max_iterations: usize,
    output_text: &str,
) -> String {
    format!(
        "{base}\n\nCortex required-tool transport recovery: the previous model turn violated REQUIRED tool choice by returning text instead of an executable tool call. {} You are now in a mutation-only recovery turn. Do NOT echo transcript markers or describe a patch. Call one or more of the offered project-file mutation tools now, using the authoritative evidence already collected. This is structured-tool iteration {iteration} of {max_iterations}.",
        required_tool_protocol_failure_summary(output_text)
    )
}

fn required_tool_protocol_failure_summary(output_text: &str) -> String {
    let trimmed = output_text.trim();
    if trimmed.contains("[CORTEX STRUCTURED TOOL REQUESTS]")
        || trimmed.contains("[CORTEX AUTHORITATIVE TOOL RESULTS]")
    {
        "The model echoed Cortex's stateless structured-tool transcript as plain text.".into()
    } else if trimmed.is_empty() {
        "The model returned an empty response instead of a structured tool call.".into()
    } else {
        let preview = trimmed.chars().take(160).collect::<String>();
        format!("Plain-text preview: {preview}")
    }
}

fn mutation_required_instructions(base: &str, iteration: usize, max_iterations: usize) -> String {
    format!(
        "{}\n\nCortex mutation phase: source evidence has already been collected. You MUST now call one or more of the offered project-file mutation tools and apply ALL bounded project-file changes needed for the user's requested implementation or repair. Do not stop after only repairing a manifest when the requested feature also requires source edits. Do not answer with prose, a code block, a plan, or another read-only inspection. This is structured-tool iteration {} of {}.",
        base, iteration, max_iterations
    )
}

fn append_stateless_tool_exchange(
    messages: &mut Vec<ModelMessage>,
    response: &ModelResponse,
    results: &[ToolResultInput],
) {
    // Stateless providers must receive prior tool traffic as structured Responses
    // items, not prose marker blocks. MessageRole::Tool is an internal transport
    // envelope; cortex_provider_lmstudio converts these JSON payloads into native
    // `function_call` / `function_call_output` input items before HTTP dispatch.
    for call in &response.tool_calls {
        messages.push(ModelMessage {
            role: MessageRole::Tool,
            content: json!({
                "cortex_replay_type": "function_call",
                "call_id": call.call_id,
                "name": call.name,
                "arguments": call.arguments,
            })
            .to_string(),
        });
    }

    for result in results {
        messages.push(ModelMessage {
            role: MessageRole::Tool,
            content: json!({
                "cortex_replay_type": "function_call_output",
                "call_id": result.call_id,
                "output": result.output,
                "is_error": result.is_error,
            })
            .to_string(),
        });
    }
}

fn sanitize_internal_transport_transcript(text: &str) -> String {
    let mut sanitized = text.to_string();
    sanitized = redact_complete_transport_block(
        sanitized,
        "[CORTEX STRUCTURED TOOL REQUESTS]",
        "[END CORTEX STRUCTURED TOOL REQUESTS]",
        "[Cortex internal structured-tool request transcript omitted]",
    );
    sanitized = redact_complete_transport_block(
        sanitized,
        "[CORTEX AUTHORITATIVE TOOL RESULTS]",
        "[END CORTEX AUTHORITATIVE TOOL RESULTS]",
        "[Cortex internal authoritative tool-result transcript omitted]",
    );

    // Error previews can be truncated before a matching end marker. Never leave
    // the reserved transport tokens in ordinary model-visible project context.
    for marker in [
        "[CORTEX STRUCTURED TOOL REQUESTS]",
        "[END CORTEX STRUCTURED TOOL REQUESTS]",
        "[CORTEX AUTHORITATIVE TOOL RESULTS]",
        "[END CORTEX AUTHORITATIVE TOOL RESULTS]",
    ] {
        sanitized = sanitized.replace(marker, "[Cortex internal transport marker omitted]");
    }
    sanitized
}

fn redact_complete_transport_block(
    mut text: String,
    start_marker: &str,
    end_marker: &str,
    replacement: &str,
) -> String {
    while let Some(start) = text.find(start_marker) {
        let search_from = start.saturating_add(start_marker.len());
        let Some(relative_end) = text[search_from..].find(end_marker) else {
            break;
        };
        let end = search_from
            .saturating_add(relative_end)
            .saturating_add(end_marker.len());
        text.replace_range(start..end, replacement);
    }
    text
}

fn compact_stateless_messages(messages: &mut Vec<ModelMessage>, max_chars: usize) {
    if messages.len() <= 1 {
        return;
    }

    let max_chars = max_chars.max(8 * 1024);
    let mut kept_reversed = Vec::new();

    // Preserve the authoritative first request, but bound it independently so a
    // large workspace preflight can never consume the entire native context.
    let first = messages.first().cloned().unwrap();
    let first_budget = (max_chars / 2).max(4 * 1024);
    let first_content = if first.content.chars().count() > first_budget {
        head_tail_text(&first.content, first_budget)
    } else {
        first.content
    };
    let first = ModelMessage {
        role: first.role,
        content: first_content,
    };
    let mut used = first.content.chars().count();

    for message in messages.iter().skip(1).rev() {
        if used >= max_chars {
            break;
        }
        let remaining = max_chars.saturating_sub(used);
        let content_len = message.content.chars().count();
        let content = if content_len > remaining {
            tail_text(&message.content, remaining)
        } else {
            message.content.clone()
        };
        used = used.saturating_add(content.chars().count());
        kept_reversed.push(ModelMessage {
            role: message.role.clone(),
            content,
        });
    }

    kept_reversed.reverse();
    messages.clear();
    messages.push(first);
    messages.extend(kept_reversed);
}

fn head_tail_text(value: &str, max_chars: usize) -> String {
    let total = value.chars().count();
    if total <= max_chars {
        return value.to_string();
    }
    if max_chars < 96 {
        return value.chars().take(max_chars).collect();
    }
    let marker = format!(
        "\n[CORTEX CONTEXT COMPACTED: {} middle character(s) omitted]\n",
        total.saturating_sub(max_chars)
    );
    let marker_chars = marker.chars().count();
    let content_budget = max_chars.saturating_sub(marker_chars);
    let head_budget = content_budget.saturating_mul(3) / 5;
    let tail_budget = content_budget.saturating_sub(head_budget);
    let head = value.chars().take(head_budget).collect::<String>();
    let tail = value
        .chars()
        .skip(total.saturating_sub(tail_budget))
        .collect::<String>();
    format!("{head}{marker}{tail}")
}

fn tail_text(value: &str, max_chars: usize) -> String {
    let total = value.chars().count();
    if total <= max_chars {
        return value.to_string();
    }
    if max_chars < 96 {
        return value
            .chars()
            .skip(total.saturating_sub(max_chars))
            .collect();
    }
    let marker = format!(
        "[CORTEX CONTEXT COMPACTED: {} older character(s) omitted]\n",
        total.saturating_sub(max_chars)
    );
    let marker_chars = marker.chars().count();
    let tail_budget = max_chars.saturating_sub(marker_chars);
    let tail = value
        .chars()
        .skip(total.saturating_sub(tail_budget))
        .collect::<String>();
    format!("{marker}{tail}")
}

fn project_iteration_instructions(base: &str, iteration: usize, max_iterations: usize) -> String {
    let remaining = max_iterations.saturating_sub(iteration.saturating_sub(1));
    if remaining <= 2 {
        format!(
            "{base}\n\nCortex evidence budget: approximately {remaining} structured-tool turn(s) remain. Do not repeat already answered searches. Call only tools that are strictly necessary to answer the user's request. If the collected evidence is sufficient, stop calling tools and provide the final grounded answer now."
        )
    } else {
        base.to_string()
    }
}

fn final_synthesis_instructions(base: &str, tool_iterations: usize) -> String {
    format!(
        "{base}\n\nCortex has reached its bounded evidence budget after {tool_iterations} structured-tool turn(s). No more tools are available on this turn. Produce the final grounded answer now using only the project evidence and tool results already present in this response chain. State concrete files/symbols discovered, answer the user's request directly, and explicitly note any remaining uncertainty instead of requesting another tool."
    )
}

fn bound_tool_result(mut result: ToolResultInput, max_bytes: usize) -> ToolResultInput {
    let max_bytes = max_bytes.max(4096);
    let Ok(encoded) = serde_json::to_string(&result.output) else {
        return result;
    };
    if encoded.len() <= max_bytes {
        return result;
    }

    let preview_budget = max_bytes.saturating_sub(768).max(1024);
    let preview = encoded.chars().take(preview_budget).collect::<String>();
    result.output = json!({
        "truncated": true,
        "original_bytes": encoded.len(),
        "preview": preview,
        "hint": "Cortex bounded this tool result to protect the model context window. Use a narrower source.read/source.search/source.list request if more detail is required."
    });
    result
}

fn repair_action_required(call: &ToolCall) -> ToolResultInput {
    ToolResultInput {
        call_id: call.call_id.clone(),
        is_error: true,
        output: json!({
            "failure_class": "repair_action_required",
            "error": "M11U4 bounded Repair exploration reached its per-round observation limit before a source mutation",
            "tool": call.name,
            "observation_limit": REPAIR_PRE_MUTATION_OBSERVATION_CALL_LIMIT,
            "instruction": "Compiler/dependency evidence is already available. Stop broad source/status exploration and make one bounded project-file mutation against the controller-selected repair target. The controller will format/validate immediately after the mutation."
        }),
    }
}

fn denied(call: &ToolCall, mode: AgentMode) -> ToolResultInput {
    ToolResultInput {
        call_id: call.call_id.clone(),
        is_error: true,
        output: json!({
            "error": "tool requires mutation permission",
            "tool": call.name,
            "agent_mode": mode,
            "hint": "switch this Cortex session to workspace_autonomy or explicitly approve the transaction"
        }),
    }
}

pub fn default_chat_instructions() -> String {
    r#"You are Cortex, a persistent local software-development assistant.

Speak naturally as the user's developer/collaborator. Do not make the user learn or select internal Cortex modes.
This particular conversational turn has no structured project tools attached, so never invent file/runtime/build facts.
When the prompt includes project attachment or recent-conversation context, use that supplied context and preserve the project's actual identity.
Do not say "switch to Inspect mode", "open a coding session", or otherwise send the user away from the conversation.
If a request needs project tools or source mutation and the necessary evidence is not already supplied, explain briefly that Cortex can continue the request through its project workflow; the Desktop router is responsible for performing that handoff. Never tell the user they must edit project files manually merely because this individual chat-only turn lacks mutation tools.
For ordinary non-project questions, answer normally and concisely."#
        .into()
}

pub fn default_system_instructions() -> String {
    r#"You are Cortex, a project-aware local development agent.

Work from current source truth. Prefer source/project/runtime evidence over old planning notes.
Use the provided structured tools rather than inventing file contents or shell results. On the first turn of every project-aware request, obtain structured project evidence before making project-specific claims.
Do not guess filenames, frameworks, or target implementations from ambiguous wording. Establish the exact active target with workspace/source evidence before drawing conclusions. Prefer current build/workspace members and active source paths over archived, donor, reference, generated, payload, artifact, or similarly named legacy implementations unless the user explicitly asks for them. If the exact target cannot be identified, state that uncertainty instead of substituting a different subsystem.
Prefer focused source.search queries over broad repository-root source.list calls when locating an implementation.
When Cortex supplies an AUTHORITATIVE WORKSPACE PREFLIGHT, use its Cargo-member and source-manifest facts instead of recomputing counts from partial searches. For broad project inspections, organize the final answer around current health, active applications, canonical authorities, compatibility/reference boundaries, concrete gaps/risks, and recommended next work. Do not end with a generic invitation to explore more.
Before source mutation, begin a Cortex transaction. Keep edits bounded to the active workspace.
When this is a Code/Apply or Repair execution turn and source-mutation tools are offered, you MUST make the requested bounded change with an actual project-file mutation tool such as source.write_text, source.replace_text, vscode.apply_workspace_edit, or an appropriate project-content promotion tool before presenting the work as implemented. A prose-only answer, pasted source snippet, pseudo-code, or instructions telling the user to edit files manually is not implementation and must not be reported as success. If mutation authority or a required tool is unavailable, report the concrete blocker instead.
After Rust edits, verify with structured Cargo checks. Read diagnostics and repair compile failures.
When runtime or UI behavior matters, launch the owned development process, capture evidence, and inspect it.
Generated images are draft Cortex artifacts until explicitly promoted into project content.
Do not access paths outside the active workspace, install software, change global settings, or control unowned processes.
Summarize concrete files changed, verification performed, runtime/visual evidence, and any remaining uncertainty."#.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_protocol::{ProviderError, ToolDefinition, ToolExecutor, ToolResultInput};
    use serde_json::json;

    struct PlainTextProvider;

    impl TextProvider for PlainTextProvider {
        fn provider_id(&self) -> &str {
            "plain"
        }

        fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(vec!["plain".into()])
        }

        fn respond(&self, _request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
            Ok(ModelResponse {
                id: Some("resp-1".into()),
                output_text: "I inspected /invented/project with grep.".into(),
                tool_calls: Vec::new(),
                raw: json!({}),
            })
        }
    }

    struct ReadTool;

    impl ToolExecutor for ReadTool {
        fn definitions(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "read".into(),
                description: "Read project source".into(),
                parameters: json!({"type":"object","properties":{}}),
                mutating: false,
            }]
        }

        fn execute(&mut self, call: &ToolCall) -> ToolResultInput {
            ToolResultInput {
                call_id: call.call_id.clone(),
                output: json!({"ok":true}),
                is_error: false,
            }
        }
    }

    #[test]
    fn first_project_response_without_tool_call_is_rejected() {
        let mut engine = AgentEngine::new(PlainTextProvider, ReadTool, AgentSettings::default());
        let error = engine.run("Inspect this project").unwrap_err();
        assert!(error.contains("ungrounded model response"));
    }

    #[test]
    fn authoritative_workspace_preflight_allows_direct_grounded_synthesis() {
        struct PreflightProvider;

        impl TextProvider for PreflightProvider {
            fn provider_id(&self) -> &str {
                "preflight"
            }

            fn list_models(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec!["preflight".into()])
            }

            fn respond(&self, request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
                let prompt = request
                    .messages
                    .first()
                    .map(|message| message.content.as_str())
                    .unwrap_or_default();
                assert!(prompt.contains("CORTEX AUTHORITATIVE WORKSPACE PREFLIGHT"));
                assert!(prompt.contains("\"member_count\":70"));
                assert!(prompt.contains("\"verification_authority\""));
                assert!(prompt.contains("\"dependency_grounding\""));
                assert!(prompt.contains("Ground Before Write"));
                assert!(prompt.len() < AUTHORITATIVE_PREFLIGHT_MAX_BYTES + 2048);
                Ok(ModelResponse {
                    id: Some("preflight-final".into()),
                    output_text: "Grounded project overview.".into(),
                    tool_calls: Vec::new(),
                    raw: json!({}),
                })
            }
        }

        struct PreflightTools;

        impl ToolExecutor for PreflightTools {
            fn definitions(&self) -> Vec<ToolDefinition> {
                vec![ToolDefinition {
                    name: "workspace.status".into(),
                    description: "status".into(),
                    parameters: json!({"type":"object","properties":{}}),
                    mutating: false,
                }]
            }

            fn execute(&mut self, call: &ToolCall) -> ToolResultInput {
                assert_eq!(call.name, "workspace.status");
                ToolResultInput {
                    call_id: call.call_id.clone(),
                    output: json!({
                        "inspection_snapshot": {
                            "cargo_workspace": {"member_count": 70}
                        },
                        "dependency_grounding": {
                            "ecosystems": [{"ecosystem":"cargo","dependencies":[{"name":"wgpu","resolved_versions":["0.19.4"]}]}]
                        },
                        "dependency_grounding_state": {
                            "required": false,
                            "required_packages": [],
                            "grounded_packages": []
                        },
                        "verification_authority": {
                            "state": "unknown"
                        }
                    }),
                    is_error: false,
                }
            }
        }

        let mut engine =
            AgentEngine::new(PreflightProvider, PreflightTools, AgentSettings::default());
        let result = engine.run("Inspect the current project").unwrap();
        assert_eq!(result.text, "Grounded project overview.");
        assert_eq!(result.evidence.len(), 1);
        assert_eq!(result.evidence[0].tool, "workspace.status");
    }

    #[test]
    fn chat_identity_keeps_user_in_conversation() {
        let instructions = default_chat_instructions();
        assert!(instructions.contains("persistent local software-development assistant"));
        assert!(instructions.contains("Do not make the user learn or select internal Cortex modes"));
        assert!(!instructions.contains("tell them to use Cortex inspect mode"));
    }

    #[test]
    fn chat_does_not_offer_project_tools() {
        struct ChatProvider;

        impl TextProvider for ChatProvider {
            fn provider_id(&self) -> &str {
                "chat"
            }

            fn list_models(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec!["chat".into()])
            }

            fn respond(&self, request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
                assert!(request.tools.is_empty());
                Ok(ModelResponse {
                    id: Some("chat-1".into()),
                    output_text: "ok".into(),
                    tool_calls: Vec::new(),
                    raw: json!({}),
                })
            }
        }

        let engine = AgentEngine::new(ChatProvider, ReadTool, AgentSettings::default());
        let result = engine.chat("test").unwrap();
        assert_eq!(result.text, "ok");
        assert_eq!(result.iterations, 1);
    }

    #[test]
    fn empty_chat_response_is_rejected_instead_of_becoming_blank_ui() {
        struct EmptyProvider;

        impl TextProvider for EmptyProvider {
            fn provider_id(&self) -> &str {
                "empty"
            }

            fn list_models(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec!["empty".into()])
            }

            fn respond(&self, _request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
                Ok(ModelResponse {
                    id: Some("empty-chat".into()),
                    output_text: String::new(),
                    tool_calls: Vec::new(),
                    raw: json!({}),
                })
            }
        }

        let engine = AgentEngine::new(EmptyProvider, ReadTool, AgentSettings::default());
        let error = engine.chat("test").unwrap_err();
        assert!(error.contains("without any user-visible text"));
    }

    #[test]
    fn empty_grounded_agent_response_gets_one_text_only_recovery_turn() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct RecoveringProvider {
            turns: AtomicUsize,
        }

        impl TextProvider for RecoveringProvider {
            fn provider_id(&self) -> &str {
                "recovering"
            }

            fn list_models(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec!["recovering".into()])
            }

            fn respond(&self, request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
                let turn = self.turns.fetch_add(1, Ordering::Relaxed);
                if turn == 0 {
                    return Ok(ModelResponse {
                        id: Some("empty-agent".into()),
                        output_text: String::new(),
                        tool_calls: Vec::new(),
                        raw: json!({}),
                    });
                }

                assert!(request.tools.is_empty());
                assert_eq!(request.previous_response_id.as_deref(), Some("empty-agent"));
                assert!(request
                    .instructions
                    .as_deref()
                    .unwrap_or_default()
                    .contains("without any user-visible final text"));
                Ok(ModelResponse {
                    id: Some("recovered-agent".into()),
                    output_text: "Recovered visible answer.".into(),
                    tool_calls: Vec::new(),
                    raw: json!({}),
                })
            }
        }

        struct PreflightOnlyTools;

        impl ToolExecutor for PreflightOnlyTools {
            fn definitions(&self) -> Vec<ToolDefinition> {
                vec![ToolDefinition {
                    name: "workspace.status".into(),
                    description: "status".into(),
                    parameters: json!({"type":"object","properties":{}}),
                    mutating: false,
                }]
            }

            fn execute(&mut self, call: &ToolCall) -> ToolResultInput {
                ToolResultInput {
                    call_id: call.call_id.clone(),
                    output: json!({"inspection_snapshot":{"cargo_workspace":{"member_count":1}}}),
                    is_error: false,
                }
            }
        }

        let provider = RecoveringProvider {
            turns: AtomicUsize::new(0),
        };
        let mut engine = AgentEngine::new(provider, PreflightOnlyTools, AgentSettings::default());
        let result = engine.run("Inspect this project").unwrap();
        assert_eq!(result.text, "Recovered visible answer.");
        assert_eq!(result.iterations, 2);
    }

    #[test]
    fn stateless_provider_recovery_replays_messages_without_previous_response_id() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct StatelessRecoveringProvider {
            turns: AtomicUsize,
        }

        impl TextProvider for StatelessRecoveringProvider {
            fn provider_id(&self) -> &str {
                "stateless-recovering"
            }

            fn list_models(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec!["stateless-recovering".into()])
            }

            fn supports_previous_response_id(&self) -> bool {
                false
            }

            fn respond(&self, request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
                let turn = self.turns.fetch_add(1, Ordering::Relaxed);
                if turn == 0 {
                    return Ok(ModelResponse {
                        id: Some("native-empty".into()),
                        output_text: String::new(),
                        tool_calls: Vec::new(),
                        raw: json!({}),
                    });
                }

                assert!(request.previous_response_id.is_none());
                assert!(request.tool_results.is_empty());
                assert!(request.tools.is_empty());
                assert!(request.messages.iter().any(|message| message
                    .content
                    .contains("CORTEX AUTHORITATIVE WORKSPACE PREFLIGHT")));
                Ok(ModelResponse {
                    id: Some("native-recovered".into()),
                    output_text: "Recovered without server-side response chaining.".into(),
                    tool_calls: Vec::new(),
                    raw: json!({}),
                })
            }
        }

        struct StatelessPreflightTools;

        impl ToolExecutor for StatelessPreflightTools {
            fn definitions(&self) -> Vec<ToolDefinition> {
                vec![ToolDefinition {
                    name: "workspace.status".into(),
                    description: "status".into(),
                    parameters: json!({"type":"object","properties":{}}),
                    mutating: false,
                }]
            }

            fn execute(&mut self, call: &ToolCall) -> ToolResultInput {
                ToolResultInput {
                    call_id: call.call_id.clone(),
                    output: json!({"inspection_snapshot":{"cargo_workspace":{"member_count":76}}}),
                    is_error: false,
                }
            }
        }

        let provider = StatelessRecoveringProvider {
            turns: AtomicUsize::new(0),
        };
        let mut engine =
            AgentEngine::new(provider, StatelessPreflightTools, AgentSettings::default());
        let result = engine.run("Inspect this project").unwrap();
        assert_eq!(
            result.text,
            "Recovered without server-side response chaining."
        );
    }

    #[test]
    fn stateless_provider_tool_loop_replays_authoritative_results_as_messages() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct StatelessToolProvider {
            turns: AtomicUsize,
        }

        impl TextProvider for StatelessToolProvider {
            fn provider_id(&self) -> &str {
                "stateless-tools"
            }

            fn list_models(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec!["stateless-tools".into()])
            }

            fn supports_previous_response_id(&self) -> bool {
                false
            }

            fn respond(&self, request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
                let turn = self.turns.fetch_add(1, Ordering::Relaxed);
                assert!(request.previous_response_id.is_none());
                assert!(request.tool_results.is_empty());
                if turn == 0 {
                    return Ok(ModelResponse {
                        id: Some("native-tool-1".into()),
                        output_text: String::new(),
                        tool_calls: vec![ToolCall {
                            call_id: "native-call-1".into(),
                            name: "read".into(),
                            arguments: json!({}),
                        }],
                        raw: json!({}),
                    });
                }

                let replay_items = request
                    .messages
                    .iter()
                    .filter(|message| message.role == MessageRole::Tool)
                    .map(|message| {
                        serde_json::from_str::<serde_json::Value>(&message.content).unwrap()
                    })
                    .collect::<Vec<_>>();
                assert_eq!(replay_items.len(), 2);
                assert_eq!(
                    replay_items[0]
                        .get("cortex_replay_type")
                        .and_then(serde_json::Value::as_str),
                    Some("function_call")
                );
                assert_eq!(
                    replay_items[1]
                        .get("cortex_replay_type")
                        .and_then(serde_json::Value::as_str),
                    Some("function_call_output")
                );
                assert!(request.messages.iter().all(|message| {
                    !message
                        .content
                        .contains("[CORTEX STRUCTURED TOOL REQUESTS]")
                        && !message
                            .content
                            .contains("[CORTEX AUTHORITATIVE TOOL RESULTS]")
                }));
                Ok(ModelResponse {
                    id: Some("native-final".into()),
                    output_text: "Grounded stateless final answer.".into(),
                    tool_calls: Vec::new(),
                    raw: json!({}),
                })
            }
        }

        let provider = StatelessToolProvider {
            turns: AtomicUsize::new(0),
        };
        let mut engine = AgentEngine::new(provider, ReadTool, AgentSettings::default());
        let result = engine.run("Inspect this project").unwrap();
        assert_eq!(result.text, "Grounded stateless final answer.");
        assert_eq!(result.evidence.len(), 1);
    }

    #[test]
    fn observe_mode_does_not_offer_mutating_tools() {
        struct CatalogProvider;

        impl TextProvider for CatalogProvider {
            fn provider_id(&self) -> &str {
                "catalog"
            }

            fn list_models(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec!["catalog".into()])
            }

            fn respond(&self, request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
                assert!(request.tools.iter().all(|tool| !tool.mutating));
                Ok(ModelResponse {
                    id: Some("catalog-1".into()),
                    output_text: "no tool call".into(),
                    tool_calls: Vec::new(),
                    raw: json!({}),
                })
            }
        }

        struct MixedTools;

        impl ToolExecutor for MixedTools {
            fn definitions(&self) -> Vec<ToolDefinition> {
                vec![
                    ToolDefinition {
                        name: "read".into(),
                        description: "read".into(),
                        parameters: json!({"type":"object","properties":{}}),
                        mutating: false,
                    },
                    ToolDefinition {
                        name: "write".into(),
                        description: "write".into(),
                        parameters: json!({"type":"object","properties":{}}),
                        mutating: true,
                    },
                ]
            }

            fn execute(&mut self, call: &ToolCall) -> ToolResultInput {
                ToolResultInput {
                    call_id: call.call_id.clone(),
                    output: json!({"ok":true}),
                    is_error: false,
                }
            }
        }

        let settings = AgentSettings {
            mode: AgentMode::Observe,
            ..Default::default()
        };
        let mut engine = AgentEngine::new(CatalogProvider, MixedTools, settings);
        let error = engine.run("inspect").unwrap_err();
        assert!(error.contains("ungrounded model response"));
    }

    #[test]
    fn exhausted_tool_budget_forces_grounded_final_synthesis() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct BudgetProvider {
            turns: AtomicUsize,
        }

        impl TextProvider for BudgetProvider {
            fn provider_id(&self) -> &str {
                "budget"
            }

            fn list_models(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec!["budget".into()])
            }

            fn respond(&self, request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
                let turn = self.turns.fetch_add(1, Ordering::Relaxed);

                if request.tools.is_empty() {
                    assert!(request.previous_response_id.is_some());
                    assert!(!request.tool_results.is_empty());
                    assert!(request
                        .instructions
                        .as_deref()
                        .unwrap_or_default()
                        .contains("No more tools are available"));
                    return Ok(ModelResponse {
                        id: Some("final".into()),
                        output_text: "Grounded final answer from collected evidence.".into(),
                        tool_calls: Vec::new(),
                        raw: json!({}),
                    });
                }

                Ok(ModelResponse {
                    id: Some(format!("tool-{turn}")),
                    output_text: String::new(),
                    tool_calls: vec![ToolCall {
                        call_id: format!("call-{turn}"),
                        name: "read".into(),
                        arguments: json!({}),
                    }],
                    raw: json!({}),
                })
            }
        }

        let settings = AgentSettings {
            mode: AgentMode::Observe,
            max_tool_iterations: 2,
            ..Default::default()
        };
        let provider = BudgetProvider {
            turns: AtomicUsize::new(0),
        };
        let mut engine = AgentEngine::new(provider, ReadTool, settings);
        let result = engine.run("Inspect this project").unwrap();

        assert_eq!(
            result.text,
            "Grounded final answer from collected evidence."
        );
        assert_eq!(result.iterations, 3);
        assert_eq!(result.evidence.len(), 2);
        assert_eq!(result.tool_results.len(), 2);
    }

    #[test]
    fn late_iterations_warn_model_to_stop_repeating_tools() {
        let instructions = project_iteration_instructions("base", 8, 8);
        assert!(instructions.contains("evidence budget"));
        assert!(instructions.contains("Do not repeat"));
        assert!(instructions.contains("final grounded answer"));
    }

    #[test]
    fn stateless_replay_compaction_preserves_request_and_bounds_history() {
        let mut messages = vec![
            ModelMessage {
                role: MessageRole::User,
                content: format!("REQUEST-BEGIN\n{}\nPREFLIGHT-END", "a".repeat(12_000)),
            },
            ModelMessage {
                role: MessageRole::Assistant,
                content: "b".repeat(12_000),
            },
            ModelMessage {
                role: MessageRole::User,
                content: format!(
                    "LATEST-EVIDENCE-BEGIN\n{}\nLATEST-EVIDENCE-END",
                    "c".repeat(12_000)
                ),
            },
        ];
        compact_stateless_messages(&mut messages, 8 * 1024);
        let total = messages
            .iter()
            .map(|message| message.content.chars().count())
            .sum::<usize>();
        assert!(total <= 8 * 1024);
        assert!(messages.first().unwrap().content.contains("REQUEST-BEGIN"));
        assert!(messages
            .last()
            .unwrap()
            .content
            .contains("LATEST-EVIDENCE-END"));
        assert!(messages
            .iter()
            .any(|message| message.content.contains("CORTEX CONTEXT COMPACTED")));
    }

    #[test]
    fn ground_before_write_failure_opens_read_only_grounding_recovery_lane() {
        let blocked = ToolResultInput {
            call_id: "blocked-write".into(),
            output: json!({
                "failure_class": "dependency_grounding",
                "error": "Ground Before Write blocked this API-sensitive mutation."
            }),
            is_error: true,
        };
        assert!(tool_results_require_dependency_grounding(&[blocked]));
        assert!(is_dependency_grounding_recovery_tool("source.search"));
        assert!(is_dependency_grounding_recovery_tool("workspace.status"));
        assert!(!is_dependency_grounding_recovery_tool("source.write_text"));
    }

    #[test]
    fn complete_dependency_grounding_closes_recovery_lane() {
        let grounded = ToolResultInput {
            call_id: "ground-wgpu".into(),
            output: json!({
                "grounding_recorded": true,
                "grounded_package": "wgpu",
                "dependency_grounding_complete": true,
                "remaining_required_dependency_packages": []
            }),
            is_error: false,
        };
        assert!(tool_results_complete_dependency_grounding(&[grounded]));
    }

    #[test]
    fn blocked_write_exposes_all_packages_for_controller_batch_grounding() {
        let blocked = ToolResultInput {
            call_id: "blocked-write".into(),
            output: json!({
                "failure_class": "dependency_grounding",
                "missing_dependency_packages": ["chrono", "wgpu", "winit"]
            }),
            is_error: true,
        };
        assert_eq!(
            dependency_grounding_packages(&[blocked]),
            vec![
                "chrono".to_string(),
                "wgpu".to_string(),
                "winit".to_string(),
            ]
        );
    }

    #[test]
    fn source_commit_is_controller_only_authority() {
        assert!(is_controller_only_tool("source.commit"));
        assert!(!is_project_file_mutation_tool("source.commit"));
        assert!(!is_controller_only_tool("source.write_text"));
    }

    #[test]
    fn repair_tool_profile_excludes_controller_orchestration_tools() {
        assert!(is_repair_model_tool("source.read"));
        assert!(is_repair_model_tool("source.replace_text"));
        assert!(!is_repair_model_tool("vscode.get_diagnostics"));
        assert!(!is_repair_model_tool("vscode.apply_workspace_edit"));
        assert!(!is_repair_model_tool("dependency.ground"));
        assert!(is_controller_only_tool("dependency.ground"));
        assert!(!is_repair_model_tool("source.begin_transaction"));
        assert!(!is_repair_model_tool("source.commit"));
        assert!(!is_repair_model_tool("build.project_validate"));
        assert!(!is_repair_model_tool("development.run_advance"));
    }

    #[test]
    fn oversized_tool_results_are_bounded() {
        let large = "x".repeat(200_000);
        let result = ToolResultInput {
            call_id: "large".into(),
            output: json!({"text": large}),
            is_error: false,
        };
        let bounded = bound_tool_result(result, 32 * 1024);
        assert_eq!(
            bounded
                .output
                .get("truncated")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
        assert!(serde_json::to_string(&bounded.output).unwrap().len() < 40 * 1024);
    }

    #[test]
    fn workspace_autonomy_requires_structured_tools_until_file_mutation() {
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct MutationProvider {
            requests: Arc<Mutex<Vec<ModelRequest>>>,
            turn: Arc<Mutex<usize>>,
        }

        impl TextProvider for MutationProvider {
            fn provider_id(&self) -> &str {
                "mutation-provider"
            }

            fn list_models(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec!["mutation-provider".into()])
            }

            fn respond(&self, request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
                self.requests.lock().unwrap().push(request.clone());
                let mut turn = self.turn.lock().unwrap();
                *turn += 1;
                let response = match *turn {
                    1..=3 => ModelResponse {
                        id: Some(format!("read-{}", *turn)),
                        output_text: String::new(),
                        tool_calls: vec![ToolCall {
                            call_id: format!("read-call-{}", *turn),
                            name: "source.read".into(),
                            arguments: json!({"path":"Cargo.toml"}),
                        }],
                        raw: json!({}),
                    },
                    4 => ModelResponse {
                        id: Some("write-4".into()),
                        output_text: String::new(),
                        tool_calls: vec![ToolCall {
                            call_id: "write-call-4".into(),
                            name: "source.write_text".into(),
                            arguments: json!({"path":"Cargo.toml","content":"[package]\nname = \"demo\"\n"}),
                        }],
                        raw: json!({}),
                    },
                    _ => ModelResponse {
                        id: Some("final-4".into()),
                        output_text: "Mutation complete.".into(),
                        tool_calls: Vec::new(),
                        raw: json!({}),
                    },
                };
                Ok(response)
            }
        }

        struct MutationTools;
        impl ToolExecutor for MutationTools {
            fn definitions(&self) -> Vec<ToolDefinition> {
                vec![
                    ToolDefinition {
                        name: "workspace.status".into(),
                        description: "status".into(),
                        parameters: json!({"type":"object","properties":{}}),
                        mutating: false,
                    },
                    ToolDefinition {
                        name: "source.read".into(),
                        description: "read".into(),
                        parameters: json!({"type":"object","properties":{}}),
                        mutating: false,
                    },
                    ToolDefinition {
                        name: "source.write_text".into(),
                        description: "write".into(),
                        parameters: json!({"type":"object","properties":{}}),
                        mutating: true,
                    },
                ]
            }

            fn execute(&mut self, call: &ToolCall) -> ToolResultInput {
                ToolResultInput {
                    call_id: call.call_id.clone(),
                    output: json!({"ok":true}),
                    is_error: false,
                }
            }
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = MutationProvider {
            requests: requests.clone(),
            turn: Arc::new(Mutex::new(0)),
        };
        let settings = AgentSettings {
            mode: AgentMode::WorkspaceAutonomy,
            ..Default::default()
        };
        let mut engine = AgentEngine::new(provider, MutationTools, settings);
        let result = engine.run("Repair Cargo.toml").unwrap();
        assert_eq!(result.text, "Mutation complete.");
        assert!(result
            .evidence
            .iter()
            .any(|item| { item.tool == "source.write_text" && item.mutating && !item.is_error }));

        let captured = requests.lock().unwrap();
        assert_eq!(captured[0].tool_choice, ToolChoicePolicy::Required);
        assert_eq!(captured[1].tool_choice, ToolChoicePolicy::Required);
        assert_eq!(captured[2].tool_choice, ToolChoicePolicy::Required);
        assert_eq!(captured[3].tool_choice, ToolChoicePolicy::Required);
        assert_eq!(captured[3].tools.len(), 1);
        assert_eq!(captured[3].tools[0].name, "source.write_text");
        assert_eq!(captured[4].tool_choice, ToolChoicePolicy::Auto);
    }

    #[test]
    fn repair_profile_bounds_batched_observations_and_forces_mutation_next_round() {
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct RepairProvider {
            requests: Arc<Mutex<Vec<ModelRequest>>>,
            turn: Arc<Mutex<usize>>,
        }

        impl TextProvider for RepairProvider {
            fn provider_id(&self) -> &str {
                "repair-provider"
            }

            fn list_models(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec!["repair-provider".into()])
            }

            fn supports_previous_response_id(&self) -> bool {
                false
            }

            fn respond(&self, request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
                self.requests.lock().unwrap().push(request.clone());
                let mut turn = self.turn.lock().unwrap();
                *turn += 1;
                Ok(match *turn {
                    1 => ModelResponse {
                        id: Some("repair-observe".into()),
                        output_text: String::new(),
                        tool_calls: (0..8)
                            .map(|index| ToolCall {
                                call_id: format!("search-{index}"),
                                name: "source.search".into(),
                                arguments: json!({"query": format!("symbol-{index}")}),
                            })
                            .collect(),
                        raw: json!({}),
                    },
                    2 => ModelResponse {
                        id: Some("repair-write".into()),
                        output_text: String::new(),
                        tool_calls: vec![ToolCall {
                            call_id: "repair-write-call".into(),
                            name: "source.write_text".into(),
                            arguments: json!({
                                "path": "src/main.rs",
                                "content": "fn main() {}\\n"
                            }),
                        }],
                        raw: json!({}),
                    },
                    _ => ModelResponse {
                        id: Some("repair-final".into()),
                        output_text: "Repair mutation completed.".into(),
                        tool_calls: Vec::new(),
                        raw: json!({}),
                    },
                })
            }
        }

        #[derive(Clone)]
        struct RepairTools {
            executed: Arc<Mutex<Vec<String>>>,
        }

        impl ToolExecutor for RepairTools {
            fn definitions(&self) -> Vec<ToolDefinition> {
                vec![
                    ToolDefinition {
                        name: "workspace.status".into(),
                        description: "status".into(),
                        parameters: json!({"type":"object","properties":{}}),
                        mutating: false,
                    },
                    ToolDefinition {
                        name: "source.search".into(),
                        description: "search".into(),
                        parameters: json!({"type":"object","properties":{}}),
                        mutating: false,
                    },
                    ToolDefinition {
                        name: "source.write_text".into(),
                        description: "write".into(),
                        parameters: json!({"type":"object","properties":{}}),
                        mutating: true,
                    },
                ]
            }

            fn execute(&mut self, call: &ToolCall) -> ToolResultInput {
                self.executed.lock().unwrap().push(call.name.clone());
                ToolResultInput {
                    call_id: call.call_id.clone(),
                    output: if call.name == "workspace.status" {
                        json!({
                            "inspection_snapshot": {},
                            "dependency_grounding": {},
                            "dependency_grounding_state": {
                                "required": false,
                                "required_packages": [],
                                "grounded_packages": []
                            },
                            "verification_authority": {"state": "failing"}
                        })
                    } else {
                        json!({"ok": true})
                    },
                    is_error: false,
                }
            }
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let executed = Arc::new(Mutex::new(Vec::new()));
        let provider = RepairProvider {
            requests: requests.clone(),
            turn: Arc::new(Mutex::new(0)),
        };
        let tools = RepairTools {
            executed: executed.clone(),
        };
        let settings = AgentSettings {
            mode: AgentMode::WorkspaceAutonomy,
            tool_profile: AgentToolProfile::Repair,
            ..Default::default()
        };
        let mut engine = AgentEngine::new(provider, tools, settings);
        let result = engine
            .run("Repair src/main.rs from the supplied compiler evidence")
            .unwrap();

        assert_eq!(result.text, "Repair mutation completed.");
        assert_eq!(
            executed
                .lock()
                .unwrap()
                .iter()
                .filter(|tool| tool.as_str() == "source.search")
                .count(),
            REPAIR_PRE_MUTATION_OBSERVATION_CALL_LIMIT
        );
        assert!(result.tool_results.iter().any(|result| {
            result.is_error
                && result.output.get("failure_class").and_then(Value::as_str)
                    == Some("repair_action_required")
        }));

        let captured = requests.lock().unwrap();
        assert_eq!(captured[1].tool_choice, ToolChoicePolicy::Required);
        assert_eq!(captured[1].tools.len(), 1);
        assert_eq!(captured[1].tools[0].name, "source.write_text");
        assert!(captured[1]
            .instructions
            .as_deref()
            .unwrap_or_default()
            .contains("project-file mutation"));
    }

    #[test]
    fn stale_internal_transport_markers_are_removed_from_project_prompt() {
        let source = "RECENT PROJECT CONTEXT:\nold failure\n[CORTEX STRUCTURED TOOL REQUESTS]\n[{\"name\":\"source.read\"}]\n[END CORTEX STRUCTURED TOOL REQUESTS]\n[CORTEX AUTHORITATIVE TOOL RESULTS]\n[{\"ok\":true}]\n[END CORTEX AUTHORITATIVE TOOL RESULTS]\ncontinue project";
        let sanitized = sanitize_internal_transport_transcript(source);
        assert!(!sanitized.contains("[CORTEX STRUCTURED TOOL REQUESTS]"));
        assert!(!sanitized.contains("[CORTEX AUTHORITATIVE TOOL RESULTS]"));
        assert!(sanitized.contains("internal structured-tool request transcript omitted"));
        assert!(sanitized.contains("internal authoritative tool-result transcript omitted"));
        assert!(sanitized.contains("continue project"));
    }

    #[test]
    fn required_tool_transcript_echo_recovers_into_mutation_only_turn() {
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct EchoRecoveryProvider {
            requests: Arc<Mutex<Vec<ModelRequest>>>,
            turn: Arc<Mutex<usize>>,
        }

        impl TextProvider for EchoRecoveryProvider {
            fn provider_id(&self) -> &str {
                "echo-recovery-provider"
            }

            fn list_models(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec!["echo-recovery-provider".into()])
            }

            fn supports_previous_response_id(&self) -> bool {
                false
            }

            fn respond(&self, request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
                self.requests.lock().unwrap().push(request.clone());
                let mut turn = self.turn.lock().unwrap();
                *turn += 1;
                Ok(match *turn {
                    1 => ModelResponse {
                        id: Some("read-1".into()),
                        output_text: String::new(),
                        tool_calls: vec![ToolCall {
                            call_id: "read-call-1".into(),
                            name: "source.read".into(),
                            arguments: json!({"path":"src/main.rs"}),
                        }],
                        raw: json!({}),
                    },
                    2 => ModelResponse {
                        id: Some("echo-2".into()),
                        output_text: "[CORTEX STRUCTURED TOOL REQUESTS]\n[{\"name\":\"source.read\"}]\n[END CORTEX STRUCTURED TOOL REQUESTS]\n[CORTEX AUTHORITATIVE TOOL RESULTS]".into(),
                        tool_calls: Vec::new(),
                        raw: json!({}),
                    },
                    3 => ModelResponse {
                        id: Some("write-3".into()),
                        output_text: String::new(),
                        tool_calls: vec![ToolCall {
                            call_id: "write-call-3".into(),
                            name: "source.write_text".into(),
                            arguments: json!({"path":"src/main.rs","content":"fn main() {}\\n"}),
                        }],
                        raw: json!({}),
                    },
                    _ => ModelResponse {
                        id: Some("final-4".into()),
                        output_text: "Recovered and mutated the project.".into(),
                        tool_calls: Vec::new(),
                        raw: json!({}),
                    },
                })
            }
        }

        struct EchoRecoveryTools;
        impl ToolExecutor for EchoRecoveryTools {
            fn definitions(&self) -> Vec<ToolDefinition> {
                vec![
                    ToolDefinition {
                        name: "source.read".into(),
                        description: "read".into(),
                        parameters: json!({"type":"object","properties":{}}),
                        mutating: false,
                    },
                    ToolDefinition {
                        name: "source.write_text".into(),
                        description: "write".into(),
                        parameters: json!({"type":"object","properties":{}}),
                        mutating: true,
                    },
                ]
            }

            fn execute(&mut self, call: &ToolCall) -> ToolResultInput {
                ToolResultInput {
                    call_id: call.call_id.clone(),
                    output: json!({"ok":true}),
                    is_error: false,
                }
            }
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = EchoRecoveryProvider {
            requests: requests.clone(),
            turn: Arc::new(Mutex::new(0)),
        };
        let settings = AgentSettings {
            mode: AgentMode::WorkspaceAutonomy,
            ..Default::default()
        };
        let mut engine = AgentEngine::new(provider, EchoRecoveryTools, settings);
        let result = engine.run("Repair and finish the application").unwrap();

        assert_eq!(result.text, "Recovered and mutated the project.");
        assert!(result
            .evidence
            .iter()
            .any(|item| item.tool == "source.write_text" && !item.is_error));

        let captured = requests.lock().unwrap();
        assert_eq!(captured[0].tool_choice, ToolChoicePolicy::Required);
        assert_eq!(captured[1].tool_choice, ToolChoicePolicy::Required);
        assert_eq!(captured[2].tool_choice, ToolChoicePolicy::Required);
        assert_eq!(captured[2].tools.len(), 1);
        assert_eq!(captured[2].tools[0].name, "source.write_text");
        assert!(captured[2]
            .instructions
            .as_deref()
            .unwrap_or_default()
            .contains("mutation-only recovery turn"));
        assert!(captured[2]
            .messages
            .iter()
            .any(|message| message.content.contains("REQUIRED-TOOL TRANSPORT RECOVERY")));
        assert_eq!(captured[3].tool_choice, ToolChoicePolicy::Auto);
    }
}

#[cfg(test)]
mod w12_evidence_tests {
    use super::*;

    #[test]
    fn receipt_exposes_only_bounded_path_and_fixed_error_class() {
        let call = ToolCall {
            call_id: "redacted-call".into(),
            name: "source.replace_text".into(),
            arguments: json!({"path":"Cargo.toml","new":"secret-value-in-source"}),
        };
        let result = ToolResultInput {
            call_id: call.call_id.clone(),
            output: json!({"error":"expected 1 occurrence of secret-value-in-source; found 0"}),
            is_error: true,
        };
        let evidence = AgentToolEvidence::from_result(&call, true, &result);
        assert_eq!(evidence.target_path.as_deref(), Some("Cargo.toml"));
        assert_eq!(
            evidence.failure_class.as_deref(),
            Some("exact_replace_mismatch")
        );
        assert!(!serde_json::to_string(&evidence)
            .unwrap()
            .contains("secret-value"));
        assert!(safe_evidence_relative_path("C:\\private\\secret.txt").is_none());
        assert!(safe_evidence_relative_path("../token.txt").is_none());
    }

    #[test]
    fn successful_mutation_receipt_records_controller_rollback() {
        let call = ToolCall {
            call_id: "candidate-edit".into(),
            name: "source.write_text".into(),
            arguments: json!({"path":"Cargo.toml"}),
        };
        let result = ToolResultInput {
            call_id: call.call_id.clone(),
            output: json!({"controller_verification":{"candidate":{"decision":"rollback_unchanged"}}}),
            is_error: false,
        };
        let evidence = AgentToolEvidence::from_result(&call, true, &result);
        assert_eq!(
            evidence.candidate_decision.as_deref(),
            Some("rollback_unchanged")
        );
    }
}
