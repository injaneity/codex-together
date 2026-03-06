use codex_protocol::protocol::RolloutItem;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

pub const METHOD_INITIALIZE: &str = "initialize";
pub const METHOD_INITIALIZED: &str = "initialized";

pub const METHOD_TOGETHER_AUTH: &str = "together/auth";
pub const METHOD_TOGETHER_SERVER_CREATE: &str = "together/server/create";
pub const METHOD_TOGETHER_SERVER_CLOSE: &str = "together/server/close";
pub const METHOD_TOGETHER_MEMBER_ADD: &str = "together/member/add";
pub const METHOD_TOGETHER_MEMBER_REMOVE: &str = "together/member/remove";
pub const METHOD_TOGETHER_SERVER_INFO: &str = "together/server/info";
pub const METHOD_TOGETHER_THREAD_SHARE: &str = "together/thread/share";
pub const METHOD_TOGETHER_THREAD_CHECKOUT: &str = "together/thread/checkout";
pub const METHOD_TOGETHER_THREAD_READ: &str = "together/thread/read";
pub const METHOD_TOGETHER_THREAD_FORK: &str = "together/thread/fork";
pub const METHOD_TOGETHER_THREAD_DELETE: &str = "together/thread/delete";
pub const METHOD_TOGETHER_THREAD_LIST: &str = "together/thread/list";
pub const METHOD_TOGETHER_HISTORY_LINEAGE: &str = "together/history/lineage";
pub const METHOD_TOGETHER_JOIN: &str = "together/join";
pub const METHOD_TOGETHER_LEAVE: &str = "together/leave";

pub const NOTIFY_TOGETHER_SERVER_CLOSED: &str = "together/serverClosed";
pub const NOTIFY_TOGETHER_MEMBER_UPDATED: &str = "together/memberUpdated";
pub const NOTIFY_TOGETHER_THREAD_SHARED: &str = "together/threadShared";
pub const NOTIFY_TOGETHER_THREAD_FORKED: &str = "together/threadForked";
pub const NOTIFY_TOGETHER_CONNECTION_REVOKED: &str = "together/connectionRevoked";

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckoutReason {
    NonOwnerMustFork,
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
pub struct TogetherServerCloseResponse {
    pub closed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherMemberUpdateRequest {
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherMemberUpdateResponse {
    pub updated: bool,
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
    pub role: TogetherRole,
    pub connected_members: Vec<ConnectedMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherThreadShareRequest {
    pub thread_id: String,
    #[serde(default)]
    pub history: Option<Vec<RolloutItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherThreadShareResponse {
    pub thread_id: String,
    pub owner_email: String,
    #[serde(default)]
    pub preview: Option<String>,
    pub shared_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherThreadCheckoutRequest {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherThreadCheckoutResponse {
    pub thread_id: String,
    pub writable: bool,
    pub owner_email: String,
    #[serde(default)]
    pub reason: Option<CheckoutReason>,
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
pub struct TogetherThreadForkRequest {
    pub thread_id: String,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherThreadForkResponse {
    pub parent_thread_id: String,
    pub child_thread_id: String,
    pub owner_email: String,
    pub history: Option<Vec<RolloutItem>>,
    pub writable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherThreadDeleteRequest {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherThreadDeleteResponse {
    pub thread_id: String,
    pub deleted: bool,
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
pub struct TogetherHistoryLineageRequest {
    pub root_thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineageNode {
    pub thread_id: String,
    pub owner_email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineageEdge {
    pub parent_thread_id: String,
    pub child_thread_id: String,
    pub actor_email: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherHistoryLineageResponse {
    pub root: String,
    pub nodes: Vec<LineageNode>,
    pub edges: Vec<LineageEdge>,
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
    NonOwnerMustFork,
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
    #[error("non-owner must fork before writing")]
    NonOwnerMustFork,
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
            Self::NonOwnerMustFork => -39002,
            Self::ServerClosed => -39003,
            Self::IdentityUnavailable => -39004,
        }
    }
}
