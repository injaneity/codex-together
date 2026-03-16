use serde::Deserialize;
use serde::Serialize;

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
pub struct TogetherClientSession {
    pub mode: TogetherClientMode,
    pub server_id: Option<String>,
    pub owner_email: Option<String>,
    pub endpoint: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
