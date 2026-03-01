use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TogetherRole {
    Owner,
    Member,
}

impl TogetherRole {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Member => "member",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TogetherClientMode {
    Disconnected,
    Host,
    Member,
}

impl TogetherClientMode {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Host => "host",
            Self::Member => "member",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TogetherServerRecord {
    pub server_id: String,
    pub owner_email: String,
    pub public_base_url: String,
    pub invite_token: String,
    pub created_at: i64,
    pub closed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TogetherMemberRecord {
    pub server_id: String,
    pub email: String,
    pub role: TogetherRole,
    pub added_at: i64,
    pub removed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TogetherThreadAclRecord {
    pub server_id: String,
    pub thread_id: String,
    pub owner_email: String,
    pub shared_by_email: String,
    pub shared_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TogetherThreadForkRecord {
    pub server_id: String,
    pub child_thread_id: String,
    pub parent_thread_id: String,
    pub actor_email: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TogetherClientSession {
    pub mode: TogetherClientMode,
    pub server_id: Option<String>,
    pub owner_email: Option<String>,
    pub endpoint: Option<String>,
    pub checked_out_thread_id: Option<String>,
    pub host_pid: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}
