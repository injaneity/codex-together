use anyhow::Context;
use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use codex_together_protocol::TogetherJoinRequest;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvitePayload {
    pub endpoint: String,
    pub server_id: String,
    pub owner_email: String,
    pub exp: i64,
}

pub fn extract_token(invite: &str) -> String {
    if let Some(stripped) = invite.strip_prefix("codex://together/") {
        return stripped.to_string();
    }
    if let Some(index) = invite.find("/together/invite/") {
        return invite[(index + "/together/invite/".len())..].to_string();
    }
    invite.to_string()
}

pub fn decode_invite(invite: &str) -> Result<InvitePayload> {
    let token = extract_token(invite);
    let bytes = URL_SAFE_NO_PAD
        .decode(token.as_bytes())
        .context("failed to decode invite token")?;
    serde_json::from_slice(&bytes).context("failed to parse invite payload")
}

pub fn build_join_request(invite: impl Into<String>) -> TogetherJoinRequest {
    TogetherJoinRequest {
        invite: invite.into(),
    }
}

pub fn status_env_key() -> &'static str {
    "CODEX_TOGETHER_STATUS"
}
