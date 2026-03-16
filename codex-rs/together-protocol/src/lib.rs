use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::SandboxPolicy;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

pub const METHOD_INITIALIZE: &str = "initialize";
pub const METHOD_INITIALIZED: &str = "initialized";

pub const METHOD_TOGETHER_AUTH: &str = "together/auth";

// V2 collaboration RPC surface.
pub const METHOD_HOST_START: &str = "host/start";
pub const METHOD_HOST_STATUS: &str = "host/status";
pub const METHOD_HOST_STOP: &str = "host/stop";
pub const METHOD_SESSION_JOIN: &str = "session/join";
pub const METHOD_SESSION_LEAVE: &str = "session/leave";
pub const METHOD_THREAD_SHARE: &str = "thread/share";
pub const METHOD_THREAD_LIST: &str = "thread/list";
pub const METHOD_THREAD_INSPECT: &str = "thread/inspect";
pub const METHOD_CONTEXT_SEARCH: &str = "context/search";
pub const METHOD_CONTEXT_GRAPH: &str = "context/graph";
pub const METHOD_CONTEXT_PREVIEW: &str = "context/preview";
pub const METHOD_CONTEXT_RESOLVE_BUNDLE: &str = "context/resolveBundle";
pub const METHOD_HANDOFF_PLAN: &str = "handoff/plan";
pub const METHOD_HANDOFF_COMMIT: &str = "handoff/commit";
pub const METHOD_CONTEXT_WRITE_PLAN: &str = "context/writePlan";
pub const METHOD_CONTEXT_WRITE_COMMIT: &str = "context/writeCommit";

pub const NOTIFY_HOST_STOPPED: &str = "host/stopped";
pub const NOTIFY_TOGETHER_MEMBER_UPDATED: &str = "together/memberUpdated";
pub const NOTIFY_TOGETHER_THREAD_SHARED: &str = "together/threadShared";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn ok<T: Serialize>(id: Value, result: T) -> Result<Self, serde_json::Error> {
        Ok(Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(serde_json::to_value(result)?),
            error: None,
        })
    }

    pub fn err(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TogetherRole {
    Owner,
    Member,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TogetherClientMode {
    Disconnected,
    Host,
    Member,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherAuthRequest {
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherAuthResponse {
    pub role: TogetherRole,
    pub server_id: String,
    pub owner_email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherServerCreateRequest {
    pub public_base_url: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherServerCreateResponse {
    pub server_id: String,
    pub owner_email: String,
    pub invite_token: String,
    pub invite_link: String,
    pub local_ws_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStopResponse {
    pub stopped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedMember {
    pub email: String,
    pub role: TogetherRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherServerInfoResponse {
    pub server_id: String,
    pub owner_email: String,
    pub public_base_url: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub commit: Option<String>,
    pub role: TogetherRole,
    pub connected_members: Vec<ConnectedMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherThreadShareRequest {
    pub thread_id: String,
    #[serde(default)]
    pub history: Option<Vec<RolloutItem>>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub repo_root: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub git_sha: Option<String>,
    #[serde(default)]
    pub git_origin_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherThreadShareResponse {
    pub thread_id: String,
    pub owner_email: String,
    #[serde(default)]
    pub preview: Option<String>,
    pub shared_at: String,
    #[serde(default)]
    pub visibility: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherThreadReadRequest {
    pub thread_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TogetherReplayRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherReplayMessage {
    pub role: TogetherReplayRole,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherThreadReadResponse {
    pub thread_id: String,
    pub owner_email: String,
    pub history: Option<Vec<RolloutItem>>,
    pub messages: Vec<TogetherReplayMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherThreadListRequest {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub search_term: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherThreadSummary {
    pub thread_id: String,
    pub owner_email: String,
    #[serde(default)]
    pub preview: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub repo_root: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub git_sha: Option<String>,
    #[serde(default)]
    pub git_origin_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherThreadListResponse {
    pub data: Vec<TogetherThreadSummary>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSearchParams {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ContextKind {
    SharedThread,
    RepoContextFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSearchResult {
    pub ref_id: String,
    pub kind: ContextKind,
    pub title: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSearchResponse {
    pub data: Vec<ContextSearchResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextGraphParams {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextGraphEdge {
    pub from_ref_id: String,
    pub to_ref_id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextGraphResponse {
    pub nodes: Vec<ContextSearchResult>,
    pub edges: Vec<ContextGraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPreviewParams {
    pub ref_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPreviewResponse {
    #[serde(default)]
    pub item: Option<ContextSearchResult>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ContextStaleState {
    Fresh,
    BranchMismatch,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextRef {
    pub ref_id: String,
    pub kind: ContextKind,
    pub display_label: String,
    #[serde(default)]
    pub source_thread_id: Option<String>,
    #[serde(default)]
    pub repo_context_id: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub stale_state: Option<ContextStaleState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextResolveBundleParams {
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub context_refs: Vec<ContextRef>,
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextResolveBundleResponse {
    pub bundle_text: String,
    pub kept_refs: Vec<ContextRef>,
    pub dropped_refs: Vec<ContextRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextWritePlanParams {
    #[serde(default)]
    pub selected_ref_ids: Vec<String>,
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextWriteFilePlan {
    pub path: String,
    pub title: String,
    pub kind: String,
    pub exists: bool,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextWritePlanResponse {
    pub plan_id: String,
    pub files: Vec<ContextWriteFilePlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextWriteCommitParams {
    pub plan_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextWriteCommitResponse {
    pub written_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffPlanParams {
    #[serde(default)]
    pub source_thread_id: Option<String>,
    #[serde(default)]
    pub selected_ref_ids: Vec<String>,
    #[serde(default)]
    pub goal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffPlanResponse {
    pub plan_id: String,
    pub source_thread_id: String,
    #[serde(default)]
    pub goal: Option<String>,
    pub selected_node_ids: Vec<String>,
    pub kept_refs: Vec<ContextRef>,
    pub dropped_refs: Vec<ContextRef>,
    pub token_estimate: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffCommitParams {
    pub plan_id: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub approval_policy: Option<AskForApproval>,
    #[serde(default)]
    pub sandbox: Option<SandboxPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffCommitResponse {
    pub thread_id: String,
    pub source_thread_id: String,
    #[serde(default)]
    pub rollout_path: Option<String>,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherJoinRequest {
    pub invite: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherJoinResponse {
    pub server_id: String,
    pub owner_email: String,
    pub endpoint: String,
    pub role: TogetherRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherLeaveResponse {
    pub left: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TogetherErrorCode {
    NotConnected,
    Forbidden,
    MemberNotAllowed,
    ServerClosed,
    SingletonConflict,
    IdentityUnavailable,
    Overloaded,
}

#[derive(Debug, Error)]
pub enum TogetherError {
    #[error("not connected to a together server")]
    NotConnected,
    #[error("forbidden")]
    Forbidden,
    #[error("server closed")]
    ServerClosed,
    #[error("chatgpt email required for together in v1")]
    IdentityUnavailable,
}

impl TogetherError {
    pub fn rpc_code(&self) -> i64 {
        match self {
            Self::NotConnected => -39000,
            Self::Forbidden => -39001,
            Self::ServerClosed => -39003,
            Self::IdentityUnavailable => -39004,
        }
    }
}
