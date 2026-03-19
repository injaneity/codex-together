use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use ts_rs::TS;

pub const CONTEXT_GRAPH_TOOL_NAME: &str = "context_graph";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ContextGraphToolOperation {
    #[default]
    Search,
    Neighbors,
    Open,
    Hotspots,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ContextGraphToolScope {
    Local,
    #[default]
    Global,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContextGraphToolArgs {
    pub op: ContextGraphToolOperation,
    #[serde(default)]
    pub scope: ContextGraphToolScope,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub ref_id: Option<String>,
    #[serde(default)]
    pub ref_ids: Vec<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContextGraphToolOutput {
    pub op: ContextGraphToolOperation,
    pub scope: ContextGraphToolScope,
    pub summary: String,
    #[serde(default)]
    pub result_ref_ids: Vec<String>,
    pub data: Value,
}
