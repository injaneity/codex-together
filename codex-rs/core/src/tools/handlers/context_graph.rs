use std::collections::HashSet;

use async_trait::async_trait;
use chrono::Utc;
use codex_app_server_protocol::Thread;
use codex_app_server_protocol::ThreadStatus;
use codex_app_server_protocol::build_turns_from_rollout_items;
use codex_context_graph::ContextDocument;
use codex_context_graph::context_graph_edges;
use codex_context_graph::context_hotspots;
use codex_context_graph::context_neighbor_ref_ids;
use codex_context_graph::context_operation_summary;
use codex_context_graph::local_scope_documents;
use codex_context_graph::repo_context_documents;
use codex_context_graph::search_context_documents;
use codex_context_graph::thread_context_documents;
use codex_protocol::context_graph::ContextGraphToolArgs;
use codex_protocol::context_graph::ContextGraphToolOperation;
use codex_protocol::context_graph::ContextGraphToolOutput;
use codex_protocol::context_graph::ContextGraphToolScope;
use codex_protocol::models::FunctionCallOutputBody;
use serde_json::json;

use crate::function_tool::FunctionCallError;
use crate::git_info::collect_git_info;
use crate::git_info::get_git_repo_root;
use crate::rollout::RolloutRecorder;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ContextGraphHandler;

const DEFAULT_CONTEXT_GRAPH_LIMIT: u32 = 8;

#[async_trait]
impl ToolHandler for ContextGraphHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let ToolInvocation {
            payload,
            session,
            turn,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::Fatal(
                    "context_graph handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: ContextGraphToolArgs = parse_arguments(&arguments)?;
        let current_thread_id = session.conversation_id.to_string();
        let repo_root = get_git_repo_root(&turn.cwd).unwrap_or_else(|| turn.cwd.clone());

        session.ensure_rollout_materialized().await;
        session.flush_rollout().await;

        let rollout_path = {
            let guard = session.services.rollout.lock().await;
            guard
                .as_ref()
                .map(|recorder| recorder.rollout_path().to_path_buf())
        };
        let turns = if let Some(rollout_path) = &rollout_path {
            let (items, _, _) = RolloutRecorder::load_rollout_items(rollout_path)
                .await
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!(
                        "failed to load current thread context: {err}"
                    ))
                })?;
            build_turns_from_rollout_items(&items)
        } else {
            Vec::new()
        };

        let now = Utc::now().timestamp();
        let mut documents = repo_context_documents(repo_root.as_path());
        documents.extend(thread_context_documents(
            &Thread {
                id: current_thread_id.clone(),
                preview: String::new(),
                ephemeral: false,
                model_provider: turn.config.model_provider_id.clone(),
                created_at: now,
                updated_at: now,
                status: ThreadStatus::Idle,
                path: rollout_path,
                cwd: turn.cwd.clone(),
                cli_version: env!("CARGO_PKG_VERSION").to_string(),
                source: turn.session_source.clone().into(),
                agent_nickname: None,
                agent_role: None,
                git_info: collect_git_info(&turn.cwd).await.map(|info| {
                    codex_app_server_protocol::GitInfo {
                        sha: info.commit_hash,
                        branch: info.branch,
                        origin_url: info.repository_url,
                    }
                }),
                name: None,
                turns,
            },
            true,
            Some(repo_root.as_path()),
        ));

        let output = context_graph_tool_output(documents, args, Some(current_thread_id.as_str()))?;
        let body = serde_json::to_string(&output).map_err(|err| {
            FunctionCallError::Fatal(format!("failed to serialize context_graph output: {err}"))
        })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(body),
            success: Some(true),
        })
    }
}

fn context_graph_tool_output(
    documents: Vec<ContextDocument>,
    args: ContextGraphToolArgs,
    current_thread_id: Option<&str>,
) -> Result<ContextGraphToolOutput, FunctionCallError> {
    let ContextGraphToolArgs {
        op,
        scope,
        query,
        ref_id,
        ref_ids: requested_ref_ids,
        limit,
    } = args;
    let limit = limit.unwrap_or(DEFAULT_CONTEXT_GRAPH_LIMIT).clamp(1, 200);
    let mut ref_ids = Vec::new();
    if let Some(ref_id) = ref_id.as_deref().filter(|ref_id| !ref_id.trim().is_empty()) {
        ref_ids.push(ref_id.to_string());
    }
    for ref_id in requested_ref_ids {
        if ref_id.trim().is_empty() || ref_ids.iter().any(|candidate| candidate == &ref_id) {
            continue;
        }
        ref_ids.push(ref_id);
    }

    let documents = match scope {
        ContextGraphToolScope::Local => local_scope_documents(&documents, current_thread_id),
        ContextGraphToolScope::Global => documents,
    };

    let (summary, result_ref_ids, data) = match op {
        ContextGraphToolOperation::Search => {
            let query = query
                .as_deref()
                .map(str::trim)
                .filter(|query| !query.is_empty())
                .ok_or_else(|| {
                    FunctionCallError::RespondToModel(
                        "context_graph search requires a non-empty query".to_string(),
                    )
                })?;
            let results =
                search_context_documents(documents, Some(query), limit, current_thread_id);
            let result_ref_ids = results.iter().map(|result| result.ref_id.clone()).collect();
            (
                context_operation_summary(op, scope, results.len(), Some(query)),
                result_ref_ids,
                json!({ "results": results }),
            )
        }
        ContextGraphToolOperation::Neighbors => {
            if ref_ids.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "context_graph neighbors requires at least one ref_id".to_string(),
                ));
            }
            let edges = context_graph_edges(&documents);
            let seed_ref_ids = ref_ids.iter().cloned().collect::<HashSet<_>>();
            let mut included_ref_ids = seed_ref_ids.clone();
            included_ref_ids.extend(context_neighbor_ref_ids(&edges, &seed_ref_ids));
            let nodes = search_context_documents(
                documents
                    .iter()
                    .filter(|document| included_ref_ids.contains(&document.ref_id))
                    .cloned()
                    .collect(),
                None,
                limit,
                current_thread_id,
            );
            let node_ref_ids = nodes
                .iter()
                .map(|node| node.ref_id.clone())
                .collect::<HashSet<_>>();
            let relevant_edges = edges
                .into_iter()
                .filter(|edge| {
                    node_ref_ids.contains(&edge.from_ref_id)
                        && node_ref_ids.contains(&edge.to_ref_id)
                })
                .collect::<Vec<_>>();
            let result_ref_ids = nodes.iter().map(|node| node.ref_id.clone()).collect();
            (
                context_operation_summary(op, scope, nodes.len(), None),
                result_ref_ids,
                json!({
                    "seedRefIds": ref_ids,
                    "nodes": nodes,
                    "edges": relevant_edges,
                }),
            )
        }
        ContextGraphToolOperation::Open => {
            let ref_id = ref_ids.first().ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "context_graph open requires a ref_id".to_string(),
                )
            })?;
            let node = documents
                .into_iter()
                .find(|document| document.ref_id == *ref_id)
                .ok_or_else(|| {
                    FunctionCallError::RespondToModel(format!("context node not found: {ref_id}"))
                })?
                .into_search_result();
            (
                context_operation_summary(op, scope, 1, Some(&node.title)),
                vec![node.ref_id.clone()],
                json!({ "node": node }),
            )
        }
        ContextGraphToolOperation::Hotspots => {
            let selected_ref_ids = ref_ids.iter().cloned().collect::<HashSet<_>>();
            let hotspots = context_hotspots(&documents, &selected_ref_ids, limit as usize);
            let result_ref_ids = hotspots
                .iter()
                .map(|hotspot| hotspot.ref_id.clone())
                .collect();
            (
                context_operation_summary(op, scope, hotspots.len(), None),
                result_ref_ids,
                json!({ "hotspots": hotspots }),
            )
        }
    };

    Ok(ContextGraphToolOutput {
        op,
        scope,
        summary,
        result_ref_ids,
        data,
    })
}

#[cfg(test)]
mod tests {
    use codex_context_graph::ContextDocumentGraphMetadata;
    use codex_context_graph::ContextKind;
    use codex_protocol::context_graph::ContextGraphToolOperation;
    use codex_protocol::context_graph::ContextGraphToolScope;
    use pretty_assertions::assert_eq;

    use super::*;

    fn sample_documents() -> Vec<ContextDocument> {
        vec![
            ContextDocument {
                ref_id: "ctx:thread:thread-1".to_string(),
                kind: ContextKind::SharedThread,
                title: "Current Thread".to_string(),
                summary: Some("thread".to_string()),
                location: Some("thread/thread-1".to_string()),
                body: None,
                search_text: "current thread".to_string(),
                graph: ContextDocumentGraphMetadata {
                    branches: vec!["main".to_string()],
                    source_threads: vec!["thread-1".to_string()],
                    source_files: Vec::new(),
                    source_refs: Vec::new(),
                },
            },
            ContextDocument {
                ref_id: "ctx:thread-search:thread-1:search-1".to_string(),
                kind: ContextKind::ThreadSearch,
                title: "robot dog search".to_string(),
                summary: Some("thread search result".to_string()),
                location: Some("web/search".to_string()),
                body: Some("dog robotics".to_string()),
                search_text: "robot dog search dog robotics".to_string(),
                graph: ContextDocumentGraphMetadata {
                    branches: vec!["main".to_string()],
                    source_threads: vec!["thread-1".to_string()],
                    source_files: Vec::new(),
                    source_refs: Vec::new(),
                },
            },
            ContextDocument {
                ref_id: "ctx:repo:note-1".to_string(),
                kind: ContextKind::RepoContextFile,
                title: "robotics note".to_string(),
                summary: Some("persistent note".to_string()),
                location: Some(".codex/context/concepts/robotics.md".to_string()),
                body: Some("biomimicry".to_string()),
                search_text: "robotics note biomimicry".to_string(),
                graph: ContextDocumentGraphMetadata {
                    branches: vec!["main".to_string()],
                    source_threads: vec!["thread-1".to_string()],
                    source_files: Vec::new(),
                    source_refs: vec!["ctx:thread-search:thread-1:search-1".to_string()],
                },
            },
        ]
    }

    #[test]
    fn local_search_prefers_current_thread_artifacts() {
        let output = context_graph_tool_output(
            sample_documents(),
            ContextGraphToolArgs {
                op: ContextGraphToolOperation::Search,
                scope: ContextGraphToolScope::Local,
                query: Some("dog".to_string()),
                ref_id: None,
                ref_ids: Vec::new(),
                limit: Some(8),
            },
            Some("thread-1"),
        )
        .expect("search should succeed");

        assert_eq!(
            output.result_ref_ids,
            vec!["ctx:thread-search:thread-1:search-1".to_string()]
        );
        assert_eq!(output.summary, "1 local context match(es) for dog");
    }

    #[test]
    fn global_open_returns_requested_node() {
        let output = context_graph_tool_output(
            sample_documents(),
            ContextGraphToolArgs {
                op: ContextGraphToolOperation::Open,
                scope: ContextGraphToolScope::Global,
                query: None,
                ref_id: Some("ctx:repo:note-1".to_string()),
                ref_ids: Vec::new(),
                limit: None,
            },
            Some("thread-1"),
        )
        .expect("open should succeed");

        assert_eq!(output.result_ref_ids, vec!["ctx:repo:note-1".to_string()]);
        assert_eq!(output.summary, "opened robotics note");
    }
}
