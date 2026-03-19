use codex_protocol::protocol::AskForApproval;
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
pub const METHOD_CONTEXT_SEARCH: &str = "context/search";
pub const METHOD_CONTEXT_GRAPH: &str = "context/graph";
pub const METHOD_CONTEXT_QUERY: &str = "context/query";
pub const METHOD_CONTEXT_PREVIEW: &str = "context/preview";
pub const METHOD_CONTEXT_RESOLVE_BUNDLE: &str = "context/resolveBundle";
pub const METHOD_HANDOFF_PLAN: &str = "handoff/plan";
pub const METHOD_HANDOFF_COMMIT: &str = "handoff/commit";
pub const METHOD_CONTEXT_WRITE_PLAN: &str = "context/writePlan";
pub const METHOD_CONTEXT_WRITE_COMMIT: &str = "context/writeCommit";
pub const METHOD_MEMORY_PROMOTE: &str = "memory/promote";
pub const METHOD_THREAD_START: &str = "thread/start";
pub const METHOD_THREAD_APPEND_ITEMS: &str = "thread/appendItems";
pub const METHOD_THREAD_READ: &str = "thread/read";
pub const METHOD_THREAD_LIST: &str = "thread/list";

pub const NOTIFY_HOST_STOPPED: &str = "host/stopped";
pub const NOTIFY_HANDOFF_ASSIGNED: &str = "handoff/assigned";
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

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TogetherActorKind {
    #[default]
    Human,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TogetherAuthRequest {
    pub email: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub actor_kind: Option<TogetherActorKind>,
    #[serde(default)]
    pub agent_role: Option<String>,
    #[serde(default)]
    pub advertise_session: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TogetherAuthResponse {
    pub connection_id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedMember {
    pub connection_id: String,
    pub email: String,
    pub role: TogetherRole,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub actor_kind: TogetherActorKind,
    #[serde(default)]
    pub agent_role: Option<String>,
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
pub struct ContextSearchParams {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub current_thread_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ContextKind {
    SharedThread,
    ThreadInsight,
    ThreadFile,
    ThreadSearch,
    ThreadTool,
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
    #[serde(default)]
    pub current_thread_id: Option<String>,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ContextPrecursorKind {
    Fork,
    Handoff,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ThreadArtifactKind {
    Plan,
    FileRead,
    FileChange,
    Search,
    ToolOutput,
    GraphQuery,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RepoMemoryKind {
    Concept,
    Decision,
    Playbook,
    Hotspot,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ContextMountReason {
    Local,
    ForkSeed,
    HandoffSeed,
    RepoNeighbor,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ContextEdgeType {
    Mounted,
    Related,
    CoveredBy,
    PromotedTo,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ThreadGraphQueryOperation {
    Search,
    Open,
    Neighbors,
    Hotspots,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ThreadGraphQueryScope {
    Current,
    Rooted,
    Repo,
    Workspace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextQueryAnchor {
    pub anchor_id: String,
    #[serde(default)]
    pub current_thread_id: Option<String>,
    #[serde(default)]
    pub precursor_thread_id: Option<String>,
    #[serde(default)]
    pub precursor_kind: Option<ContextPrecursorKind>,
    #[serde(default)]
    pub actor_id: Option<String>,
    #[serde(default)]
    pub repo_root: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub goal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextThreadNode {
    pub node_id: String,
    pub artifact_kind: ThreadArtifactKind,
    pub title: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    pub origin_thread_id: String,
    #[serde(default)]
    pub source_files: Vec<String>,
    #[serde(default)]
    pub source_refs: Vec<String>,
    #[serde(default)]
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextRepoNode {
    pub node_id: String,
    pub repo_kind: RepoMemoryKind,
    pub title: String,
    #[serde(default)]
    pub summary: Option<String>,
    pub path: String,
    #[serde(default)]
    pub source_threads: Vec<String>,
    #[serde(default)]
    pub source_refs: Vec<String>,
    #[serde(default)]
    pub source_files: Vec<String>,
    #[serde(default)]
    pub last_validated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "nodeType", rename_all = "camelCase")]
pub enum ContextQueryNode {
    Thread(ContextThreadNode),
    Repo(ContextRepoNode),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextQueryEdge {
    pub from_node_id: String,
    pub to_node_id: String,
    pub edge_type: ContextEdgeType,
    #[serde(default)]
    pub mount_reason: Option<ContextMountReason>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextQueryParams {
    #[serde(default)]
    pub current_thread_id: Option<String>,
    #[serde(default)]
    pub precursor_thread_id: Option<String>,
    #[serde(default)]
    pub precursor_kind: Option<ContextPrecursorKind>,
    #[serde(default)]
    pub actor_id: Option<String>,
    #[serde(default)]
    pub repo_root: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub seed_ref_ids: Vec<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextQueryResponse {
    pub anchor: ContextQueryAnchor,
    pub nodes: Vec<ContextQueryNode>,
    pub edges: Vec<ContextQueryEdge>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPromoteParams {
    #[serde(default)]
    pub current_thread_id: Option<String>,
    #[serde(default)]
    pub selected_node_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPromoteResponse {
    pub created: Vec<String>,
    pub already_covered: Vec<String>,
    pub proposal_required: Vec<String>,
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
    #[serde(default)]
    pub target_actor_id: Option<String>,
    #[serde(default)]
    pub preview_only: bool,
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
    pub target_connection_id: Option<String>,
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
    pub target_actor_id: Option<String>,
    #[serde(default)]
    pub target_connection_id: Option<String>,
    #[serde(default)]
    pub rollout_path: Option<String>,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HandoffAssignedNotification {
    pub thread_id: String,
    pub source_thread_id: String,
    pub source_actor_id: String,
    pub target_actor_id: String,
    pub target_connection_id: String,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    pub rollout_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    pub actor_id: String,
    #[serde(default)]
    pub repo_root: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub precursor_thread_id: Option<String>,
    #[serde(default)]
    pub precursor_kind: Option<ContextPrecursorKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartResponse {
    pub thread_id: String,
    pub actor_id: String,
    #[serde(default)]
    pub repo_root: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub precursor_thread_id: Option<String>,
    #[serde(default)]
    pub precursor_kind: Option<ContextPrecursorKind>,
    #[serde(default)]
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    pub thread_id: String,
    #[serde(default)]
    pub actor_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub repo_root: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub precursor_thread_id: Option<String>,
    #[serde(default)]
    pub precursor_kind: Option<ContextPrecursorKind>,
    #[serde(default)]
    pub updated_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadParams {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadResponse {
    pub thread: ThreadSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListParams {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub repo_root: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListResponse {
    pub data: Vec<ThreadSummary>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ThreadAppendItem {
    Plan {
        #[serde(default)]
        id: Option<String>,
        text: String,
        #[serde(default)]
        created_at: Option<i64>,
    },
    FileRead {
        #[serde(default)]
        id: Option<String>,
        path: String,
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        content: Option<String>,
        #[serde(default)]
        created_at: Option<i64>,
    },
    FileChange {
        #[serde(default)]
        id: Option<String>,
        path: String,
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        diff: Option<String>,
        #[serde(default)]
        created_at: Option<i64>,
    },
    Search {
        #[serde(default)]
        id: Option<String>,
        query: String,
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        result_body: Option<String>,
        #[serde(default)]
        created_at: Option<i64>,
    },
    ToolOutput {
        #[serde(default)]
        id: Option<String>,
        tool_name: String,
        #[serde(default)]
        summary: Option<String>,
        output: String,
        #[serde(default)]
        created_at: Option<i64>,
    },
    GraphQuery {
        #[serde(default)]
        id: Option<String>,
        operation: ThreadGraphQueryOperation,
        scope: ThreadGraphQueryScope,
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        ref_ids: Vec<String>,
        #[serde(default)]
        result_ref_ids: Vec<String>,
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        created_at: Option<i64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadAppendItemsParams {
    pub thread_id: String,
    pub items: Vec<ThreadAppendItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadAppendItemsResponse {
    pub appended_count: usize,
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

#[cfg(test)]
mod tests {
    use super::ConnectedMember;
    use super::ContextEdgeType;
    use super::ContextMountReason;
    use super::ContextPrecursorKind;
    use super::ContextQueryAnchor;
    use super::ContextQueryEdge;
    use super::ContextQueryNode;
    use super::ContextQueryResponse;
    use super::ContextRepoNode;
    use super::ContextThreadNode;
    use super::HandoffAssignedNotification;
    use super::MemoryPromoteParams;
    use super::RepoMemoryKind;
    use super::ThreadAppendItem;
    use super::ThreadAppendItemsParams;
    use super::ThreadArtifactKind;
    use super::ThreadGraphQueryOperation;
    use super::ThreadGraphQueryScope;
    use super::TogetherActorKind;
    use super::TogetherAuthRequest;
    use super::TogetherRole;
    use pretty_assertions::assert_eq;

    #[test]
    fn context_query_response_round_trips() {
        let response = ContextQueryResponse {
            anchor: ContextQueryAnchor {
                anchor_id: "anchor:thread-2".to_string(),
                current_thread_id: Some("thread-2".to_string()),
                precursor_thread_id: Some("thread-1".to_string()),
                precursor_kind: Some(ContextPrecursorKind::Handoff),
                actor_id: Some("reviewer@local".to_string()),
                repo_root: Some("/repo".to_string()),
                git_branch: Some("rewrite-codex-2gether-v2".to_string()),
                goal: Some("Verify the simplified /context and /handoff flow.".to_string()),
            },
            nodes: vec![
                ContextQueryNode::Thread(ContextThreadNode {
                    node_id: "ctx:thread-insight:thread-1:plan-2".to_string(),
                    artifact_kind: ThreadArtifactKind::Plan,
                    title: "Simplify /context selection flow".to_string(),
                    summary: Some("thread insight · retained plan output".to_string()),
                    location: Some("insight/plan-2".to_string()),
                    body: Some(
                        "Only show one-line nodes and let Enter toggle selection.".to_string(),
                    ),
                    origin_thread_id: "thread-1".to_string(),
                    source_files: Vec::new(),
                    source_refs: Vec::new(),
                    created_at: Some(1_773_792_000),
                }),
                ContextQueryNode::Repo(ContextRepoNode {
                    node_id: "ctx:file:.codex/context/playbooks/handoff-selection-flow.md"
                        .to_string(),
                    repo_kind: RepoMemoryKind::Playbook,
                    title: "Handoff selection flow".to_string(),
                    summary: Some(
                        "Selection-only handoff UI with auto-promotion on commit.".to_string(),
                    ),
                    path: ".codex/context/playbooks/handoff-selection-flow.md".to_string(),
                    source_threads: vec!["thread-1".to_string()],
                    source_refs: vec!["ctx:thread-insight:thread-1:plan-2".to_string()],
                    source_files: vec!["tui/src/chatwidget.rs".to_string()],
                    last_validated_at: Some("2026-03-18".to_string()),
                }),
            ],
            edges: vec![
                ContextQueryEdge {
                    from_node_id: "anchor:thread-2".to_string(),
                    to_node_id: "ctx:thread-insight:thread-1:plan-2".to_string(),
                    edge_type: ContextEdgeType::Mounted,
                    mount_reason: Some(ContextMountReason::HandoffSeed),
                    reason: None,
                },
                ContextQueryEdge {
                    from_node_id: "ctx:thread-insight:thread-1:plan-2".to_string(),
                    to_node_id: "ctx:file:.codex/context/playbooks/handoff-selection-flow.md"
                        .to_string(),
                    edge_type: ContextEdgeType::CoveredBy,
                    mount_reason: None,
                    reason: None,
                },
            ],
        };

        let json = serde_json::to_string(&response).expect("serialize context query response");
        let round_trip = serde_json::from_str::<ContextQueryResponse>(&json)
            .expect("deserialize context query response");
        assert_eq!(round_trip, response);
    }

    #[test]
    fn thread_append_items_params_round_trip_graph_query_variant() {
        let params = ThreadAppendItemsParams {
            thread_id: "thread-2".to_string(),
            items: vec![ThreadAppendItem::GraphQuery {
                id: Some("graph-1".to_string()),
                operation: ThreadGraphQueryOperation::Hotspots,
                scope: ThreadGraphQueryScope::Rooted,
                query: Some("handoff".to_string()),
                ref_ids: vec!["ctx:thread-insight:thread-1:plan-2".to_string()],
                result_ref_ids: vec![
                    "ctx:file:.codex/context/playbooks/handoff-selection-flow.md".to_string(),
                ],
                summary: Some("1 rooted context hotspot(s)".to_string()),
                created_at: Some(1_773_792_200),
            }],
        };

        let json = serde_json::to_string(&params).expect("serialize thread append items params");
        let round_trip = serde_json::from_str::<ThreadAppendItemsParams>(&json)
            .expect("deserialize thread append items params");
        assert_eq!(round_trip, params);
    }

    #[test]
    fn memory_promote_params_serialize_selected_node_ids() {
        let params = MemoryPromoteParams {
            current_thread_id: Some("thread-2".to_string()),
            selected_node_ids: vec![
                "ctx:thread-insight:thread-2:plan-1".to_string(),
                "ctx:thread-file:thread-2:tui-src-chatwidget-rs".to_string(),
            ],
        };

        let value = serde_json::to_value(&params).expect("serialize memory promote params");
        assert_eq!(
            value,
            serde_json::json!({
                "currentThreadId": "thread-2",
                "selectedNodeIds": [
                    "ctx:thread-insight:thread-2:plan-1",
                    "ctx:thread-file:thread-2:tui-src-chatwidget-rs"
                ]
            })
        );
    }

    #[test]
    fn together_auth_request_round_trips_actor_metadata() {
        let request = TogetherAuthRequest {
            email: "lobster-worker@local".to_string(),
            display_name: Some("Lobster Worker".to_string()),
            actor_kind: Some(TogetherActorKind::Agent),
            agent_role: Some("research".to_string()),
            advertise_session: true,
        };

        let json = serde_json::to_string(&request).expect("serialize auth request");
        let round_trip =
            serde_json::from_str::<TogetherAuthRequest>(&json).expect("deserialize auth request");

        assert_eq!(round_trip, request);
    }

    #[test]
    fn connected_member_round_trips_live_actor_metadata() {
        let member = ConnectedMember {
            connection_id: "6d6ae1c6-5c40-4fe7-80e9-44f4f92cb865".to_string(),
            email: "lobster-worker@local".to_string(),
            role: TogetherRole::Member,
            display_name: Some("Lobster Worker".to_string()),
            actor_kind: TogetherActorKind::Agent,
            agent_role: Some("research".to_string()),
        };

        let json = serde_json::to_string(&member).expect("serialize connected member");
        let round_trip =
            serde_json::from_str::<ConnectedMember>(&json).expect("deserialize connected member");

        assert_eq!(round_trip, member);
    }

    #[test]
    fn handoff_assigned_notification_round_trips_rollout_payload() {
        let notification = HandoffAssignedNotification {
            thread_id: "thread-2".to_string(),
            source_thread_id: "thread-1".to_string(),
            source_actor_id: "sender@local".to_string(),
            target_actor_id: "recipient@local".to_string(),
            target_connection_id: "6d6ae1c6-5c40-4fe7-80e9-44f4f92cb865".to_string(),
            goal: Some("Continue the bug investigation.".to_string()),
            cwd: Some("/tmp/repo".to_string()),
            rollout_path: "/tmp/repo/.codex/sessions/thread-2.jsonl".to_string(),
        };

        let json =
            serde_json::to_string(&notification).expect("serialize handoff assigned notification");
        let round_trip = serde_json::from_str::<HandoffAssignedNotification>(&json)
            .expect("deserialize handoff assigned notification");

        assert_eq!(round_trip, notification);
    }
}
