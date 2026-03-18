use crate::agent::AgentStatus;
use crate::codex::Codex;
use crate::codex::SteerInputError;
use crate::codex_delegate::run_codex_thread_one_shot_with_schema;
use crate::config::Constrained;
use crate::config::ConstraintResult;
use crate::error::CodexErr;
use crate::error::Result as CodexResult;
use crate::features::Feature;
use crate::file_watcher::WatchRegistration;
use crate::protocol::Event;
use crate::protocol::EventMsg;
use crate::protocol::Op;
use crate::protocol::Submission;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::user_input::UserInput;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use std::collections::HashSet;
use std::path::PathBuf;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::state_db::StateDbHandle;

const HANDOFF_SELECTION_SYSTEM_PROMPT: &str = "You are preparing a Codex-to-Codex handoff. Review the anchored /context tree provided by the user, choose the smallest useful subset of candidate ref_ids to mount into the receiving thread, and write a short loading prompt for the receiving agent. Prefer concrete files when the goal is about inspecting or improving specific files. The receiving agent will rediscover the mounted context through /context, so do not paste raw context into the loading prompt. Return only JSON that matches the provided schema.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandoffSelectionRequest {
    pub prompt: String,
    pub allowed_ref_ids: Vec<String>,
    pub max_selected_ref_ids: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandoffSelectionResult {
    pub selected_ref_ids: Vec<String>,
    pub loading_prompt: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandoffSelectionOutput {
    selected_ref_ids: Vec<String>,
    loading_prompt: String,
}

#[derive(Clone, Debug)]
pub struct ThreadConfigSnapshot {
    pub model: String,
    pub model_provider_id: String,
    pub approval_policy: AskForApproval,
    pub sandbox_policy: SandboxPolicy,
    pub cwd: PathBuf,
    pub ephemeral: bool,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub personality: Option<Personality>,
    pub session_source: SessionSource,
}

pub struct CodexThread {
    pub(crate) codex: Codex,
    rollout_path: Option<PathBuf>,
    _watch_registration: WatchRegistration,
}

/// Conduit for the bidirectional stream of messages that compose a thread
/// (formerly called a conversation) in Codex.
impl CodexThread {
    pub(crate) fn new(
        codex: Codex,
        rollout_path: Option<PathBuf>,
        watch_registration: WatchRegistration,
    ) -> Self {
        Self {
            codex,
            rollout_path,
            _watch_registration: watch_registration,
        }
    }

    pub async fn submit(&self, op: Op) -> CodexResult<String> {
        self.codex.submit(op).await
    }

    pub async fn steer_input(
        &self,
        input: Vec<UserInput>,
        expected_turn_id: Option<&str>,
    ) -> Result<String, SteerInputError> {
        self.codex.steer_input(input, expected_turn_id).await
    }

    pub async fn set_app_server_client_name(
        &self,
        app_server_client_name: Option<String>,
    ) -> ConstraintResult<()> {
        self.codex
            .set_app_server_client_name(app_server_client_name)
            .await
    }

    /// Use sparingly: this is intended to be removed soon.
    pub async fn submit_with_id(&self, sub: Submission) -> CodexResult<()> {
        self.codex.submit_with_id(sub).await
    }

    pub async fn next_event(&self) -> CodexResult<Event> {
        self.codex.next_event().await
    }

    pub async fn agent_status(&self) -> AgentStatus {
        self.codex.agent_status().await
    }

    pub(crate) fn subscribe_status(&self) -> watch::Receiver<AgentStatus> {
        self.codex.agent_status.clone()
    }

    pub(crate) async fn total_token_usage(&self) -> Option<TokenUsage> {
        self.codex.session.total_token_usage().await
    }

    /// Records a user-role session-prefix message without creating a new user turn boundary.
    pub(crate) async fn inject_user_message_without_turn(&self, message: String) {
        let pending_item = ResponseInputItem::Message {
            role: "user".to_string(),
            content: vec![ContentItem::InputText { text: message }],
        };
        let pending_items = vec![pending_item];
        let Err(items_without_active_turn) = self
            .codex
            .session
            .inject_response_items(pending_items)
            .await
        else {
            return;
        };

        let turn_context = self.codex.session.new_default_turn().await;
        let items: Vec<ResponseItem> = items_without_active_turn
            .into_iter()
            .map(ResponseItem::from)
            .collect();
        self.codex
            .session
            .record_conversation_items(turn_context.as_ref(), &items)
            .await;
    }

    pub fn rollout_path(&self) -> Option<PathBuf> {
        self.rollout_path.clone()
    }

    pub async fn ensure_rollout_materialized(&self) {
        self.codex.session.ensure_rollout_materialized().await;
    }

    pub fn state_db(&self) -> Option<StateDbHandle> {
        self.codex.state_db()
    }

    pub async fn config_snapshot(&self) -> ThreadConfigSnapshot {
        self.codex.thread_config_snapshot().await
    }

    pub fn enabled(&self, feature: Feature) -> bool {
        self.codex.enabled(feature)
    }

    pub async fn select_handoff_context(
        &self,
        request: HandoffSelectionRequest,
    ) -> CodexResult<HandoffSelectionResult> {
        let prompt = request.prompt.trim();
        if prompt.is_empty() {
            return Err(CodexErr::InvalidRequest(
                "handoff selection prompt cannot be empty".to_string(),
            ));
        }

        let mut allowed_ref_ids = Vec::new();
        let mut seen_ref_ids = HashSet::new();
        for ref_id in request.allowed_ref_ids {
            if seen_ref_ids.insert(ref_id.clone()) {
                allowed_ref_ids.push(ref_id);
            }
        }
        if allowed_ref_ids.is_empty() {
            return Err(CodexErr::InvalidRequest(
                "handoff selection requires at least one candidate ref id".to_string(),
            ));
        }

        let turn_context = self.codex.session.new_default_turn().await;
        let mut sub_agent_config = turn_context.config.as_ref().clone();
        sub_agent_config.base_instructions = Some(HANDOFF_SELECTION_SYSTEM_PROMPT.to_string());
        sub_agent_config.permissions.approval_policy =
            Constrained::allow_only(AskForApproval::Never);
        sub_agent_config.permissions.sandbox_policy =
            Constrained::allow_any(SandboxPolicy::new_read_only_policy());
        if let Err(err) = sub_agent_config
            .web_search_mode
            .set(WebSearchMode::Disabled)
        {
            return Err(CodexErr::Fatal(format!(
                "failed to disable web search for handoff selection: {err}"
            )));
        }
        sub_agent_config.features.disable(Feature::Collab);

        let selection_codex = run_codex_thread_one_shot_with_schema(
            sub_agent_config,
            self.codex.session.services.auth_manager.clone(),
            self.codex.session.services.models_manager.clone(),
            vec![UserInput::Text {
                text: prompt.to_string(),
                text_elements: Vec::new(),
            }],
            Some(handoff_selection_output_schema()),
            self.codex.session.clone(),
            turn_context.clone(),
            CancellationToken::new(),
            None,
        )
        .await?;

        let response_text = loop {
            match selection_codex.next_event().await? {
                Event {
                    msg: EventMsg::TurnComplete(event),
                    ..
                } => break event.last_agent_message,
                Event {
                    msg: EventMsg::TurnAborted(_),
                    ..
                } => return Err(CodexErr::TurnAborted),
                _ => {}
            }
        }
        .ok_or_else(|| {
            CodexErr::Fatal("handoff selection finished without a final response".to_string())
        })?;
        parse_handoff_selection_output(
            &response_text,
            allowed_ref_ids.as_slice(),
            request.max_selected_ref_ids,
        )
    }
}

fn handoff_selection_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "selected_ref_ids": {
                "type": "array",
                "items": { "type": "string" }
            },
            "loading_prompt": { "type": "string" }
        },
        "required": ["selected_ref_ids", "loading_prompt"],
        "additionalProperties": false
    })
}

fn parse_handoff_selection_output(
    text: &str,
    allowed_ref_ids: &[String],
    max_selected_ref_ids: usize,
) -> CodexResult<HandoffSelectionResult> {
    let output = serde_json::from_str::<HandoffSelectionOutput>(text)
        .or_else(|_| extract_handoff_selection_output(text))
        .map_err(|err| {
            CodexErr::Fatal(format!(
                "failed to parse handoff selection response as JSON: {err}"
            ))
        })?;

    let allowed_ref_ids = allowed_ref_ids.iter().cloned().collect::<HashSet<_>>();
    let mut selected_ref_ids = Vec::new();
    let mut seen_ref_ids = HashSet::new();
    for ref_id in output.selected_ref_ids {
        if !allowed_ref_ids.contains(&ref_id) || !seen_ref_ids.insert(ref_id.clone()) {
            continue;
        }
        selected_ref_ids.push(ref_id);
        if max_selected_ref_ids > 0 && selected_ref_ids.len() >= max_selected_ref_ids {
            break;
        }
    }
    if selected_ref_ids.is_empty() {
        return Err(CodexErr::Fatal(
            "handoff selection did not return any valid candidate ref ids".to_string(),
        ));
    }

    let loading_prompt = output.loading_prompt.trim().to_string();
    if loading_prompt.is_empty() {
        return Err(CodexErr::Fatal(
            "handoff selection did not return a loading prompt".to_string(),
        ));
    }

    Ok(HandoffSelectionResult {
        selected_ref_ids,
        loading_prompt,
    })
}

fn extract_handoff_selection_output(
    text: &str,
) -> Result<HandoffSelectionOutput, serde_json::Error> {
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        return serde_json::from_str(text);
    };
    serde_json::from_str(&text[start..=end])
}
