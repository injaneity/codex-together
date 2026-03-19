use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::Path;
use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use codex_app_server_protocol::CommandAction;
use codex_app_server_protocol::CommandExecutionStatus;
use codex_app_server_protocol::DynamicToolCallOutputContentItem;
use codex_app_server_protocol::DynamicToolCallStatus;
use codex_app_server_protocol::McpToolCallStatus;
use codex_app_server_protocol::PatchApplyStatus;
use codex_app_server_protocol::Thread;
use codex_app_server_protocol::ThreadItem;
use codex_protocol::context_graph::ContextGraphToolOperation;
use codex_protocol::context_graph::ContextGraphToolScope;
use codex_together_protocol::ContextEdgeType;
use codex_together_protocol::ContextGraphEdge;
use codex_together_protocol::ContextGraphResponse;
use codex_together_protocol::ContextMountReason;
use codex_together_protocol::ContextPrecursorKind;
use codex_together_protocol::ContextQueryAnchor;
use codex_together_protocol::ContextQueryEdge;
use codex_together_protocol::ContextQueryNode;
use codex_together_protocol::ContextQueryParams;
use codex_together_protocol::ContextQueryResponse;
use codex_together_protocol::ContextRepoNode;
use codex_together_protocol::ContextSearchResult;
use codex_together_protocol::ContextThreadNode;
use tracing::warn;

pub use codex_together_protocol::ContextKind;
pub use codex_together_protocol::RepoMemoryKind;
pub use codex_together_protocol::ThreadArtifactKind;

const CONTEXT_MAX_LIMIT: u32 = 200;
const CONTEXT_BODY_CHAR_LIMIT: usize = 4_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextDocument {
    pub ref_id: String,
    pub kind: ContextKind,
    pub title: String,
    pub summary: Option<String>,
    pub location: Option<String>,
    pub body: Option<String>,
    pub search_text: String,
    pub graph: ContextDocumentGraphMetadata,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextDocumentGraphMetadata {
    pub branches: Vec<String>,
    pub source_threads: Vec<String>,
    pub source_files: Vec<String>,
    pub source_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextHotspot {
    pub ref_id: String,
    pub kind: ContextKind,
    pub title: String,
    pub summary: Option<String>,
    pub location: Option<String>,
    pub score: usize,
    pub promotion_kind: String,
    pub reason: String,
}

#[derive(Debug, Default)]
struct RepoContextMetadata {
    id: Option<String>,
    title: Option<String>,
    kind: Option<String>,
    branches: Vec<String>,
    source_threads: Vec<String>,
    source_files: Vec<String>,
    source_refs: Vec<String>,
}

impl ContextDocument {
    pub fn into_search_result(self) -> ContextSearchResult {
        ContextSearchResult {
            ref_id: self.ref_id,
            kind: self.kind,
            title: self.title,
            summary: self.summary,
            location: self.location,
            body: self.body,
        }
    }
}

pub fn build_context_graph(
    documents: Vec<ContextDocument>,
    query: Option<&str>,
    limit: u32,
    current_thread_id: Option<&str>,
) -> ContextGraphResponse {
    let limit = limit.clamp(1, CONTEXT_MAX_LIMIT) as usize;
    let all_edges = context_graph_edges(&documents);
    let mut nodes = context_graph_documents(documents, &all_edges, query, limit, current_thread_id);
    sort_context_documents(&mut nodes, query, current_thread_id);
    if nodes.len() > limit {
        nodes.truncate(limit);
    }
    let node_ref_ids = nodes
        .iter()
        .map(|document| document.ref_id.clone())
        .collect::<HashSet<_>>();
    let edges = all_edges
        .into_iter()
        .filter(|edge| {
            node_ref_ids.contains(&edge.from_ref_id) && node_ref_ids.contains(&edge.to_ref_id)
        })
        .collect();
    ContextGraphResponse {
        nodes: nodes
            .into_iter()
            .map(ContextDocument::into_search_result)
            .collect(),
        edges,
    }
}

pub fn build_context_query(
    documents: Vec<ContextDocument>,
    params: &ContextQueryParams,
) -> ContextQueryResponse {
    let limit = params
        .limit
        .unwrap_or(CONTEXT_MAX_LIMIT)
        .clamp(1, CONTEXT_MAX_LIMIT) as usize;
    let anchor = ContextQueryAnchor {
        anchor_id: params
            .current_thread_id
            .as_deref()
            .map(|thread_id| format!("anchor:{thread_id}"))
            .unwrap_or_else(|| "anchor:workspace".to_string()),
        current_thread_id: params.current_thread_id.clone(),
        precursor_thread_id: params.precursor_thread_id.clone(),
        precursor_kind: params.precursor_kind,
        actor_id: params.actor_id.clone(),
        repo_root: params.repo_root.clone(),
        git_branch: params.git_branch.clone(),
        goal: params.goal.clone(),
    };
    let document_by_ref_id = documents
        .into_iter()
        .filter(|document| document.kind != ContextKind::SharedThread)
        .map(|document| (document.ref_id.clone(), document))
        .collect::<HashMap<_, _>>();
    let query_edges = context_query_edges(&document_by_ref_id);
    let mount_reason_by_node_id =
        context_query_mount_reasons(&document_by_ref_id, &query_edges, params);
    let mounted_node_ids = mount_reason_by_node_id
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    let mut included_node_ids = if mount_reason_by_node_id.is_empty() {
        HashSet::new()
    } else {
        let mut included_node_ids = mounted_node_ids.clone();
        included_node_ids.extend(context_query_neighbor_node_ids(
            &query_edges,
            &mounted_node_ids,
        ));
        included_node_ids
    };
    let query = params
        .query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(query) = query {
        let matched_node_ids = search_context_documents_internal(
            document_by_ref_id.values().cloned().collect(),
            Some(query),
            params.limit.unwrap_or(CONTEXT_MAX_LIMIT),
            params.current_thread_id.as_deref(),
        )
        .into_iter()
        .map(|document| document.ref_id)
        .collect::<HashSet<_>>();
        included_node_ids.extend(matched_node_ids.clone());
        included_node_ids.extend(context_query_neighbor_node_ids(
            &query_edges,
            &matched_node_ids,
        ));
    }
    if included_node_ids.is_empty() {
        included_node_ids.extend(document_by_ref_id.keys().cloned());
    }

    let mut mounted_documents = included_node_ids
        .iter()
        .filter(|node_id| mount_reason_by_node_id.contains_key(*node_id))
        .filter_map(|node_id| document_by_ref_id.get(node_id).cloned())
        .collect::<Vec<_>>();
    sort_context_query_documents(
        &mut mounted_documents,
        query,
        params.current_thread_id.as_deref(),
        &mount_reason_by_node_id,
    );

    let mut neighbor_documents = included_node_ids
        .iter()
        .filter(|node_id| !mount_reason_by_node_id.contains_key(*node_id))
        .filter_map(|node_id| document_by_ref_id.get(node_id).cloned())
        .collect::<Vec<_>>();
    sort_context_query_documents(
        &mut neighbor_documents,
        query,
        params.current_thread_id.as_deref(),
        &mount_reason_by_node_id,
    );

    let mut selected_documents = mounted_documents;
    selected_documents.extend(neighbor_documents);
    if selected_documents.len() > limit {
        selected_documents.truncate(limit);
    }
    let selected_node_ids = selected_documents
        .iter()
        .map(|document| document.ref_id.clone())
        .collect::<HashSet<_>>();
    let mut edges = query_edges
        .into_iter()
        .filter(|edge| {
            selected_node_ids.contains(&edge.from_node_id)
                && selected_node_ids.contains(&edge.to_node_id)
        })
        .collect::<Vec<_>>();
    edges.extend(
        mount_reason_by_node_id
            .into_iter()
            .filter(|(node_id, _)| selected_node_ids.contains(node_id))
            .map(|(node_id, mount_reason)| ContextQueryEdge {
                from_node_id: anchor.anchor_id.clone(),
                to_node_id: node_id,
                edge_type: ContextEdgeType::Mounted,
                mount_reason: Some(mount_reason),
                reason: None,
            }),
    );
    sort_context_query_edges(&mut edges);

    ContextQueryResponse {
        anchor,
        nodes: selected_documents
            .into_iter()
            .filter_map(context_query_node_from_document)
            .collect(),
        edges,
    }
}

pub fn search_context_documents(
    documents: Vec<ContextDocument>,
    query: Option<&str>,
    limit: u32,
    current_thread_id: Option<&str>,
) -> Vec<ContextSearchResult> {
    search_context_documents_internal(documents, query, limit, current_thread_id)
        .into_iter()
        .map(ContextDocument::into_search_result)
        .collect()
}

pub fn repo_context_documents(repo_root: &Path) -> Vec<ContextDocument> {
    let context_root = repo_root.join(".codex").join("context");
    let mut markdown_files = Vec::new();
    collect_markdown_files(context_root.as_path(), &mut markdown_files);
    markdown_files.sort();

    markdown_files
        .into_iter()
        .filter_map(|path| repo_context_document(repo_root, path.as_path()))
        .collect()
}

pub fn thread_context_document(thread: Thread) -> ContextDocument {
    thread_context_documents(&thread, false, None)
        .into_iter()
        .next()
        .unwrap_or_else(|| ContextDocument {
            ref_id: format!("ctx:thread:{}", thread.id),
            kind: ContextKind::SharedThread,
            title: thread.id.clone(),
            summary: None,
            location: Some(format!("thread/{}", thread.id)),
            body: None,
            search_text: thread.id.to_ascii_lowercase(),
            graph: ContextDocumentGraphMetadata::default(),
        })
}

pub fn thread_context_documents(
    thread: &Thread,
    is_current_thread: bool,
    repo_root: Option<&Path>,
) -> Vec<ContextDocument> {
    let artifact_documents = thread_artifact_documents(thread, repo_root);
    let branch = thread
        .git_info
        .as_ref()
        .and_then(|info| info.branch.clone());
    let location = format!("thread/{}", thread.id);
    let title = agent_thread_title(
        thread_context_title(thread, is_current_thread),
        thread.agent_nickname.as_deref(),
        thread.agent_role.as_deref(),
    );
    let insight_count = artifact_documents
        .iter()
        .filter(|document| document.kind == ContextKind::ThreadInsight)
        .count();
    let file_count = artifact_documents
        .iter()
        .filter(|document| document.kind == ContextKind::ThreadFile)
        .count();
    let search_count = artifact_documents
        .iter()
        .filter(|document| document.kind == ContextKind::ThreadSearch)
        .count();
    let tool_count = artifact_documents
        .iter()
        .filter(|document| document.kind == ContextKind::ThreadTool)
        .count();
    let artifact_count = artifact_documents.len();
    let mut summary_parts = vec![if is_current_thread {
        "current thread".to_string()
    } else {
        "thread".to_string()
    }];
    if artifact_count > 0 {
        summary_parts.push(format!("{artifact_count} retained artifacts"));
    }
    summary_parts.push(thread_context_summary(thread));
    let body = thread_context_body(
        thread,
        insight_count,
        file_count,
        search_count,
        tool_count,
        &artifact_documents,
    );
    let search_text = thread_context_search_text(thread, &title, &location, body.as_deref());

    let mut documents = Vec::with_capacity(artifact_documents.len().saturating_add(1));
    documents.push(ContextDocument {
        ref_id: format!("ctx:thread:{}", thread.id),
        kind: ContextKind::SharedThread,
        title,
        summary: Some(summary_parts.join(" · ")),
        location: Some(location),
        body,
        search_text,
        graph: ContextDocumentGraphMetadata {
            branches: branch.into_iter().collect(),
            source_threads: vec![thread.id.clone()],
            ..ContextDocumentGraphMetadata::default()
        },
    });
    documents.extend(artifact_documents);
    documents
}

pub fn context_graph_edges(documents: &[ContextDocument]) -> Vec<ContextGraphEdge> {
    let thread_ref_ids = documents
        .iter()
        .filter_map(|document| {
            document
                .ref_id
                .strip_prefix("ctx:thread:")
                .map(|thread_id| (thread_id, document.ref_id.as_str()))
        })
        .collect::<HashMap<_, _>>();
    let file_ref_ids = documents
        .iter()
        .filter_map(|document| {
            document
                .location
                .as_deref()
                .filter(|_| {
                    matches!(
                        document.kind,
                        ContextKind::RepoContextFile | ContextKind::ThreadFile
                    )
                })
                .map(|location| (location, document.ref_id.as_str()))
        })
        .collect::<HashMap<_, _>>();

    let mut edges = Vec::new();
    for document in documents {
        if context_is_thread_artifact(document) {
            for thread_id in &document.graph.source_threads {
                if let Some(target_ref_id) = thread_ref_ids.get(thread_id.as_str()) {
                    edges.push(ContextGraphEdge {
                        from_ref_id: (*target_ref_id).to_string(),
                        to_ref_id: document.ref_id.clone(),
                        label: context_artifact_edge_label(document).to_string(),
                    });
                }
            }
        }
        if document.kind == ContextKind::RepoContextFile {
            for source_ref in &document.graph.source_refs {
                if documents
                    .iter()
                    .any(|candidate| candidate.ref_id == *source_ref)
                {
                    edges.push(ContextGraphEdge {
                        from_ref_id: source_ref.clone(),
                        to_ref_id: document.ref_id.clone(),
                        label: "derived".to_string(),
                    });
                }
            }
            for thread_id in &document.graph.source_threads {
                if let Some(target_ref_id) = thread_ref_ids.get(thread_id.as_str()) {
                    edges.push(ContextGraphEdge {
                        from_ref_id: (*target_ref_id).to_string(),
                        to_ref_id: document.ref_id.clone(),
                        label: "source".to_string(),
                    });
                }
            }
        }
        for source_file in &document.graph.source_files {
            if let Some(target_ref_id) = file_ref_ids.get(source_file.as_str())
                && *target_ref_id != document.ref_id
            {
                edges.push(ContextGraphEdge {
                    from_ref_id: document.ref_id.clone(),
                    to_ref_id: (*target_ref_id).to_string(),
                    label: "references".to_string(),
                });
            }
        }
        if document.kind == ContextKind::RepoContextFile {
            for thread_id in &document.graph.source_threads {
                if let Some(thread_ref_id) = thread_ref_ids.get(thread_id.as_str())
                    && document.graph.branches.iter().any(|branch| {
                        context_thread_ref_matches_branch(thread_ref_id, branch, documents)
                    })
                {
                    edges.push(ContextGraphEdge {
                        from_ref_id: (*thread_ref_id).to_string(),
                        to_ref_id: document.ref_id.clone(),
                        label: "branch".to_string(),
                    });
                }
            }
            for (thread_id, thread_ref_id) in &thread_ref_ids {
                if document
                    .graph
                    .source_threads
                    .iter()
                    .any(|source| source == thread_id)
                {
                    continue;
                }
                if document.graph.branches.iter().any(|branch| {
                    context_thread_ref_matches_branch(thread_ref_id, branch, documents)
                }) {
                    edges.push(ContextGraphEdge {
                        from_ref_id: (*thread_ref_id).to_string(),
                        to_ref_id: document.ref_id.clone(),
                        label: "branch".to_string(),
                    });
                }
            }
        }
    }

    edges.sort_by(|a, b| {
        a.from_ref_id
            .cmp(&b.from_ref_id)
            .then_with(|| a.to_ref_id.cmp(&b.to_ref_id))
            .then_with(|| a.label.cmp(&b.label))
    });
    edges.dedup_by(|left, right| {
        left.from_ref_id == right.from_ref_id
            && left.to_ref_id == right.to_ref_id
            && left.label == right.label
    });
    edges
}

pub fn context_neighbor_ref_ids(
    edges: &[ContextGraphEdge],
    seed_ref_ids: &HashSet<String>,
) -> HashSet<String> {
    let mut neighbor_ref_ids = HashSet::new();
    for edge in edges {
        if seed_ref_ids.contains(&edge.from_ref_id) {
            neighbor_ref_ids.insert(edge.to_ref_id.clone());
        }
        if seed_ref_ids.contains(&edge.to_ref_id) {
            neighbor_ref_ids.insert(edge.from_ref_id.clone());
        }
    }
    neighbor_ref_ids
}

pub fn context_thread_id_from_ref_id(ref_id: &str) -> Option<String> {
    [
        "ctx:thread-insight:",
        "ctx:thread-file:",
        "ctx:thread-search:",
        "ctx:thread-tool:",
    ]
    .into_iter()
    .find_map(|prefix| {
        ref_id.strip_prefix(prefix).and_then(|rest| {
            rest.split_once(':')
                .map(|(thread_id, _)| thread_id.to_string())
        })
    })
    .or_else(|| ref_id.strip_prefix("ctx:thread:").map(str::to_string))
}

pub fn context_short_hash(ref_id: &str) -> String {
    let candidate = ref_id.rsplit(':').next().unwrap_or(ref_id);
    let compact = candidate.split('-').next().unwrap_or(candidate);
    let compact = compact.chars().take(8).collect::<String>();
    if compact.len() >= 6 && compact.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return compact;
    }

    let hash = ref_id.bytes().fold(2_166_136_261_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    });
    format!("{hash:08x}")
}

pub fn context_kind_label(kind: ContextKind) -> &'static str {
    match kind {
        ContextKind::SharedThread => "thread",
        ContextKind::ThreadInsight => "insight",
        ContextKind::ThreadFile => "file",
        ContextKind::ThreadSearch => "search",
        ContextKind::ThreadTool => "tool",
        ContextKind::RepoContextFile => "note",
    }
}

pub fn inferred_context_write_kind(document: &ContextDocument) -> String {
    let haystack = format!(
        "{} {} {}",
        document.title,
        document.summary.clone().unwrap_or_default(),
        document.body.clone().unwrap_or_default()
    )
    .to_ascii_lowercase();
    if haystack.contains("decision") || haystack.contains("tradeoff") {
        "decision".to_string()
    } else if haystack.contains("playbook")
        || haystack.contains("workflow")
        || haystack.contains("debug")
    {
        "playbook".to_string()
    } else if haystack.contains("hotspot")
        || haystack.contains("sharp edge")
        || haystack.contains("failure")
        || haystack.contains("expiry")
    {
        "hotspot".to_string()
    } else {
        "concept".to_string()
    }
}

pub fn context_hotspots(
    documents: &[ContextDocument],
    selected_ref_ids: &HashSet<String>,
    limit: usize,
) -> Vec<ContextHotspot> {
    let edges = context_graph_edges(documents);
    let scores = context_hotspot_scores(documents, &edges, selected_ref_ids);
    let mut hotspots = documents
        .iter()
        .filter(|document| !matches!(document.kind, ContextKind::SharedThread))
        .map(|document| ContextHotspot {
            ref_id: document.ref_id.clone(),
            kind: document.kind,
            title: document.title.clone(),
            summary: document.summary.clone(),
            location: document.location.clone(),
            score: scores.get(&document.ref_id).copied().unwrap_or_default(),
            promotion_kind: inferred_context_write_kind(document),
            reason: context_hotspot_reason(document, &edges, selected_ref_ids),
        })
        .collect::<Vec<_>>();
    hotspots.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| {
                left.title
                    .to_ascii_lowercase()
                    .cmp(&right.title.to_ascii_lowercase())
            })
            .then_with(|| left.ref_id.cmp(&right.ref_id))
    });
    hotspots.truncate(limit);
    hotspots
}

pub fn local_scope_documents(
    documents: &[ContextDocument],
    current_thread_id: Option<&str>,
) -> Vec<ContextDocument> {
    let Some(current_thread_id) = current_thread_id else {
        return Vec::new();
    };

    documents
        .iter()
        .filter(|document| {
            context_is_thread_artifact(document)
                && document
                    .graph
                    .source_threads
                    .iter()
                    .any(|thread_id| thread_id == current_thread_id)
        })
        .cloned()
        .collect()
}

pub fn context_operation_summary(
    op: ContextGraphToolOperation,
    scope: ContextGraphToolScope,
    result_count: usize,
    detail: Option<&str>,
) -> String {
    let scope = match scope {
        ContextGraphToolScope::Local => "local",
        ContextGraphToolScope::Global => "global",
    };
    match op {
        ContextGraphToolOperation::Search => match detail {
            Some(detail) if !detail.is_empty() => {
                format!("{result_count} {scope} context match(es) for {detail}")
            }
            _ => format!("{result_count} {scope} context match(es)"),
        },
        ContextGraphToolOperation::Neighbors => {
            format!("{result_count} linked {scope} context node(s)")
        }
        ContextGraphToolOperation::Open => detail
            .filter(|detail| !detail.is_empty())
            .map(|detail| format!("opened {detail}"))
            .unwrap_or_else(|| "opened context node".to_string()),
        ContextGraphToolOperation::Hotspots => {
            format!("{result_count} {scope} context hotspot(s)")
        }
    }
}

fn search_context_documents_internal(
    mut documents: Vec<ContextDocument>,
    query: Option<&str>,
    limit: u32,
    current_thread_id: Option<&str>,
) -> Vec<ContextDocument> {
    let limit = limit.clamp(1, CONTEXT_MAX_LIMIT) as usize;
    let query = query.map(str::trim).filter(|value| !value.is_empty());

    if let Some(query) = query {
        let query_lower = query.to_ascii_lowercase();
        let tokens = query_lower.split_whitespace().collect::<Vec<_>>();
        documents.retain(|document| document_matches_query(document, &tokens));
    }

    sort_context_documents(&mut documents, query, current_thread_id);

    if documents.len() > limit {
        documents.truncate(limit);
    }

    documents
}

fn context_graph_documents(
    documents: Vec<ContextDocument>,
    edges: &[ContextGraphEdge],
    query: Option<&str>,
    limit: usize,
    current_thread_id: Option<&str>,
) -> Vec<ContextDocument> {
    let current_component_ref_ids = current_thread_id
        .map(|thread_id| format!("ctx:thread:{thread_id}"))
        .map(|thread_ref_id| context_connected_ref_ids(edges, thread_ref_id.as_str()))
        .unwrap_or_default();
    let query = query.map(str::trim).filter(|value| !value.is_empty());

    if query.is_none() {
        if current_component_ref_ids.is_empty() {
            return documents;
        }
        return documents
            .into_iter()
            .filter(|document| current_component_ref_ids.contains(&document.ref_id))
            .collect();
    }

    let matched_documents = search_context_documents_internal(
        documents.clone(),
        query,
        limit.try_into().unwrap_or(u32::MAX),
        current_thread_id,
    );
    let matched_ref_ids = matched_documents
        .iter()
        .map(|document| document.ref_id.clone())
        .collect::<HashSet<_>>();
    let mut included_ref_ids = current_component_ref_ids;
    included_ref_ids.extend(matched_ref_ids.iter().cloned());
    included_ref_ids.extend(context_neighbor_ref_ids(edges, &matched_ref_ids));

    documents
        .into_iter()
        .filter(|document| included_ref_ids.contains(&document.ref_id))
        .collect()
}

fn sort_context_documents(
    documents: &mut [ContextDocument],
    query: Option<&str>,
    current_thread_id: Option<&str>,
) {
    let query = query.map(str::trim).filter(|value| !value.is_empty());
    let mut score_by_ref_id = HashMap::new();
    if let Some(query) = query {
        let query_lower = query.to_ascii_lowercase();
        let tokens = query_lower.split_whitespace().collect::<Vec<_>>();
        score_by_ref_id = documents
            .iter()
            .map(|document| {
                let score = if document_matches_query(document, &tokens) {
                    context_match_score(document, &query_lower, &tokens)
                } else {
                    0
                };
                (document.ref_id.clone(), score)
            })
            .collect();
    }

    let distance_by_ref_id = context_focus_distance_by_ref_id(documents, current_thread_id);
    documents.sort_by(|left, right| {
        score_by_ref_id
            .get(&right.ref_id)
            .copied()
            .unwrap_or_default()
            .cmp(
                &score_by_ref_id
                    .get(&left.ref_id)
                    .copied()
                    .unwrap_or_default(),
            )
            .then_with(|| {
                context_focus_sort_key(left, &distance_by_ref_id)
                    .cmp(&context_focus_sort_key(right, &distance_by_ref_id))
            })
    });
}

fn context_connected_ref_ids(edges: &[ContextGraphEdge], root_ref_id: &str) -> HashSet<String> {
    let mut connected_ref_ids = HashSet::from([root_ref_id.to_string()]);
    let mut queue = VecDeque::from([root_ref_id.to_string()]);
    while let Some(ref_id) = queue.pop_front() {
        for neighbor_ref_id in context_neighbor_ref_ids(edges, &HashSet::from([ref_id.clone()])) {
            if connected_ref_ids.insert(neighbor_ref_id.clone()) {
                queue.push_back(neighbor_ref_id);
            }
        }
    }
    connected_ref_ids
}

fn context_thread_ref_matches_branch(
    thread_ref_id: &str,
    branch: &str,
    documents: &[ContextDocument],
) -> bool {
    documents.iter().any(|document| {
        document.ref_id == thread_ref_id
            && document.graph.branches.iter().any(|value| value == branch)
    })
}

fn context_focus_distance_by_ref_id(
    documents: &[ContextDocument],
    current_thread_id: Option<&str>,
) -> HashMap<String, usize> {
    let Some(current_thread_id) = current_thread_id else {
        return HashMap::new();
    };
    let current_ref_id = format!("ctx:thread:{current_thread_id}");
    if !documents
        .iter()
        .any(|document| document.ref_id == current_ref_id)
    {
        return HashMap::new();
    }

    let mut adjacency = HashMap::<String, Vec<String>>::new();
    for edge in context_graph_edges(documents) {
        adjacency
            .entry(edge.from_ref_id.clone())
            .or_default()
            .push(edge.to_ref_id.clone());
        adjacency
            .entry(edge.to_ref_id)
            .or_default()
            .push(edge.from_ref_id);
    }

    let mut distance_by_ref_id = HashMap::from([(current_ref_id.clone(), 0usize)]);
    let mut queue = VecDeque::from([(current_ref_id, 0usize)]);
    while let Some((ref_id, distance)) = queue.pop_front() {
        for neighbor_ref_id in adjacency.get(&ref_id).into_iter().flatten() {
            if distance_by_ref_id.contains_key(neighbor_ref_id) {
                continue;
            }
            let next_distance = distance.saturating_add(1);
            distance_by_ref_id.insert(neighbor_ref_id.clone(), next_distance);
            queue.push_back((neighbor_ref_id.clone(), next_distance));
        }
    }

    distance_by_ref_id
}

fn context_focus_sort_key(
    document: &ContextDocument,
    distance_by_ref_id: &HashMap<String, usize>,
) -> (usize, u8, String, String) {
    let (kind_rank, title, ref_id) = context_default_sort_key(document);
    (
        distance_by_ref_id
            .get(&document.ref_id)
            .copied()
            .unwrap_or(usize::MAX),
        kind_rank,
        title,
        ref_id,
    )
}

fn context_match_score(document: &ContextDocument, query: &str, tokens: &[&str]) -> usize {
    let title = document.title.to_ascii_lowercase();
    let summary = document
        .summary
        .clone()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let location = document
        .location
        .clone()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let body = document
        .body
        .clone()
        .unwrap_or_default()
        .to_ascii_lowercase();

    let mut score = 0usize;
    if title.contains(query) {
        score += 100;
    }
    if location.contains(query) {
        score += 80;
    }
    if summary.contains(query) {
        score += 60;
    }
    if body.contains(query) {
        score += 40;
    }
    score
        + tokens
            .iter()
            .filter(|token| {
                title.contains(**token)
                    || summary.contains(**token)
                    || location.contains(**token)
                    || body.contains(**token)
            })
            .count()
}

fn document_matches_query(document: &ContextDocument, tokens: &[&str]) -> bool {
    if tokens.is_empty() {
        return true;
    }

    tokens
        .iter()
        .all(|token| document.search_text.contains(*token))
}

fn context_default_sort_key(document: &ContextDocument) -> (u8, String, String) {
    let kind_rank = context_kind_rank(document.kind);
    (
        kind_rank,
        document.title.to_ascii_lowercase(),
        document.ref_id.clone(),
    )
}

fn context_kind_rank(kind: ContextKind) -> u8 {
    match kind {
        ContextKind::SharedThread => 0,
        ContextKind::ThreadInsight => 1,
        ContextKind::ThreadFile => 2,
        ContextKind::ThreadSearch => 3,
        ContextKind::ThreadTool => 4,
        ContextKind::RepoContextFile => 5,
    }
}

fn context_is_thread_artifact(document: &ContextDocument) -> bool {
    matches!(
        document.kind,
        ContextKind::ThreadInsight
            | ContextKind::ThreadFile
            | ContextKind::ThreadSearch
            | ContextKind::ThreadTool
    )
}

fn context_artifact_edge_label(document: &ContextDocument) -> &'static str {
    match document.kind {
        ContextKind::ThreadInsight => "insight",
        ContextKind::ThreadFile => "file",
        ContextKind::ThreadSearch => "search",
        ContextKind::ThreadTool => "tool",
        ContextKind::SharedThread | ContextKind::RepoContextFile => "context",
    }
}

fn context_query_node_from_document(document: ContextDocument) -> Option<ContextQueryNode> {
    match document.kind {
        ContextKind::ThreadInsight
        | ContextKind::ThreadFile
        | ContextKind::ThreadSearch
        | ContextKind::ThreadTool => {
            let artifact_kind = context_thread_artifact_kind(&document);
            let origin_thread_id = document
                .graph
                .source_threads
                .first()
                .cloned()
                .or_else(|| context_thread_id_from_ref_id(&document.ref_id))
                .unwrap_or_else(|| "unknown".to_string());
            Some(ContextQueryNode::Thread(ContextThreadNode {
                node_id: document.ref_id,
                artifact_kind,
                title: document.title,
                summary: document.summary,
                location: document.location,
                body: document.body,
                origin_thread_id,
                source_files: document.graph.source_files,
                source_refs: document.graph.source_refs,
                created_at: None,
            }))
        }
        ContextKind::RepoContextFile => {
            let repo_kind = context_repo_memory_kind(&document);
            Some(ContextQueryNode::Repo(ContextRepoNode {
                node_id: document.ref_id,
                repo_kind,
                title: document.title,
                summary: document.summary,
                path: document.location.unwrap_or_default(),
                source_threads: document.graph.source_threads,
                source_refs: document.graph.source_refs,
                source_files: document.graph.source_files,
                last_validated_at: None,
            }))
        }
        ContextKind::SharedThread => None,
    }
}

fn context_query_edges(
    document_by_ref_id: &HashMap<String, ContextDocument>,
) -> Vec<ContextQueryEdge> {
    let documents = document_by_ref_id.values().cloned().collect::<Vec<_>>();
    let mut edges = context_graph_edges(&documents)
        .into_iter()
        .filter_map(|edge| {
            let from_document = document_by_ref_id.get(&edge.from_ref_id)?;
            let to_document = document_by_ref_id.get(&edge.to_ref_id)?;
            if from_document.kind == ContextKind::SharedThread
                || to_document.kind == ContextKind::SharedThread
            {
                return None;
            }
            let (edge_type, reason) = match edge.label.as_str() {
                "derived" => (ContextEdgeType::PromotedTo, Some("source_ref".to_string())),
                "references" => (ContextEdgeType::Related, Some("same_file".to_string())),
                "branch" => (ContextEdgeType::Related, Some("same_branch".to_string())),
                label => (ContextEdgeType::Related, Some(label.to_string())),
            };
            Some(ContextQueryEdge {
                from_node_id: edge.from_ref_id,
                to_node_id: edge.to_ref_id,
                edge_type,
                mount_reason: None,
                reason,
            })
        })
        .collect::<Vec<_>>();

    for document in document_by_ref_id
        .values()
        .filter(|document| context_is_thread_artifact(document))
    {
        for source_ref in &document.graph.source_refs {
            if !document_by_ref_id.contains_key(source_ref) || source_ref == &document.ref_id {
                continue;
            }
            edges.push(ContextQueryEdge {
                from_node_id: document.ref_id.clone(),
                to_node_id: source_ref.clone(),
                edge_type: ContextEdgeType::Related,
                mount_reason: None,
                reason: Some(
                    if context_thread_artifact_kind(document) == ThreadArtifactKind::GraphQuery {
                        "query_result".to_string()
                    } else {
                        "source_ref".to_string()
                    },
                ),
            });
        }
    }

    for repo_document in document_by_ref_id
        .values()
        .filter(|document| document.kind == ContextKind::RepoContextFile)
    {
        for source_file in &repo_document.graph.source_files {
            for thread_document in document_by_ref_id.values().filter(|document| {
                context_is_thread_artifact(document)
                    && document.location.as_deref() == Some(source_file.as_str())
                    && !repo_document.graph.source_refs.contains(&document.ref_id)
            }) {
                edges.push(ContextQueryEdge {
                    from_node_id: thread_document.ref_id.clone(),
                    to_node_id: repo_document.ref_id.clone(),
                    edge_type: ContextEdgeType::CoveredBy,
                    mount_reason: None,
                    reason: Some("source_file".to_string()),
                });
            }
        }
    }

    sort_context_query_edges(&mut edges);
    edges
}

fn context_query_mount_reasons(
    document_by_ref_id: &HashMap<String, ContextDocument>,
    _edges: &[ContextQueryEdge],
    params: &ContextQueryParams,
) -> HashMap<String, ContextMountReason> {
    let mut mount_reason_by_node_id = document_by_ref_id
        .values()
        .filter(|document| context_is_thread_artifact(document))
        .filter(|document| {
            params
                .current_thread_id
                .as_deref()
                .is_some_and(|thread_id| {
                    document
                        .graph
                        .source_threads
                        .iter()
                        .any(|source_thread_id| source_thread_id == thread_id)
                })
        })
        .map(|document| (document.ref_id.clone(), ContextMountReason::Local))
        .collect::<HashMap<_, _>>();

    for seed_ref_id in &params.seed_ref_ids {
        let Some(document) = document_by_ref_id.get(seed_ref_id) else {
            continue;
        };
        if !context_is_thread_artifact(document) {
            continue;
        }
        let mount_reason = if params
            .current_thread_id
            .as_deref()
            .is_some_and(|thread_id| {
                document
                    .graph
                    .source_threads
                    .iter()
                    .any(|source_thread_id| source_thread_id == thread_id)
            }) {
            ContextMountReason::Local
        } else {
            match params.precursor_kind {
                Some(ContextPrecursorKind::Fork) => ContextMountReason::ForkSeed,
                Some(ContextPrecursorKind::Handoff) => ContextMountReason::HandoffSeed,
                None => ContextMountReason::Local,
            }
        };
        mount_reason_by_node_id.insert(seed_ref_id.clone(), mount_reason);
    }

    mount_reason_by_node_id
}

fn context_query_neighbor_node_ids(
    edges: &[ContextQueryEdge],
    seed_node_ids: &HashSet<String>,
) -> HashSet<String> {
    let mut neighbor_node_ids = HashSet::new();
    for edge in edges {
        if seed_node_ids.contains(&edge.from_node_id) {
            neighbor_node_ids.insert(edge.to_node_id.clone());
        }
        if seed_node_ids.contains(&edge.to_node_id) {
            neighbor_node_ids.insert(edge.from_node_id.clone());
        }
    }
    neighbor_node_ids
}

fn sort_context_query_documents(
    documents: &mut [ContextDocument],
    query: Option<&str>,
    current_thread_id: Option<&str>,
    mount_reason_by_node_id: &HashMap<String, ContextMountReason>,
) {
    let query = query.map(str::trim).filter(|value| !value.is_empty());
    let query_lower = query.map(str::to_ascii_lowercase);
    let tokens = query_lower
        .as_deref()
        .map(|query| query.split_whitespace().collect::<Vec<_>>())
        .unwrap_or_default();
    documents.sort_by(|left, right| {
        let left_mount_rank =
            mount_reason_by_node_id
                .get(&left.ref_id)
                .map_or(u8::MAX, |mount_reason| match mount_reason {
                    ContextMountReason::Local => 0,
                    ContextMountReason::ForkSeed => 1,
                    ContextMountReason::HandoffSeed => 2,
                    ContextMountReason::RepoNeighbor => 3,
                });
        let right_mount_rank =
            mount_reason_by_node_id
                .get(&right.ref_id)
                .map_or(u8::MAX, |mount_reason| match mount_reason {
                    ContextMountReason::Local => 0,
                    ContextMountReason::ForkSeed => 1,
                    ContextMountReason::HandoffSeed => 2,
                    ContextMountReason::RepoNeighbor => 3,
                });
        let left_query_score = query_lower
            .as_deref()
            .filter(|_| document_matches_query(left, &tokens))
            .map(|query| context_match_score(left, query, &tokens))
            .unwrap_or_default();
        let right_query_score = query_lower
            .as_deref()
            .filter(|_| document_matches_query(right, &tokens))
            .map(|query| context_match_score(right, query, &tokens))
            .unwrap_or_default();
        let left_current_thread_rank = current_thread_id.is_some_and(|thread_id| {
            left.graph
                .source_threads
                .iter()
                .any(|source_thread_id| source_thread_id == thread_id)
        });
        let right_current_thread_rank = current_thread_id.is_some_and(|thread_id| {
            right
                .graph
                .source_threads
                .iter()
                .any(|source_thread_id| source_thread_id == thread_id)
        });
        let left_kind_key = context_default_sort_key(left);
        let right_kind_key = context_default_sort_key(right);
        let left_thread_artifact_rank = context_query_thread_artifact_rank(left);
        let right_thread_artifact_rank = context_query_thread_artifact_rank(right);
        left_mount_rank
            .cmp(&right_mount_rank)
            .then_with(|| right_query_score.cmp(&left_query_score))
            .then_with(|| right_current_thread_rank.cmp(&left_current_thread_rank))
            .then_with(|| {
                (left.kind == ContextKind::RepoContextFile)
                    .cmp(&(right.kind == ContextKind::RepoContextFile))
            })
            .then_with(|| left_thread_artifact_rank.cmp(&right_thread_artifact_rank))
            .then_with(|| left_kind_key.cmp(&right_kind_key))
    });
}

fn context_query_thread_artifact_rank(document: &ContextDocument) -> u8 {
    if !context_is_thread_artifact(document) {
        return u8::MAX;
    }

    match context_thread_artifact_kind(document) {
        ThreadArtifactKind::Plan => 0,
        ThreadArtifactKind::FileChange => 1,
        ThreadArtifactKind::FileRead => 2,
        ThreadArtifactKind::Search => 3,
        ThreadArtifactKind::ToolOutput => 4,
        ThreadArtifactKind::GraphQuery => 5,
    }
}

fn sort_context_query_edges(edges: &mut Vec<ContextQueryEdge>) {
    edges.sort_by(|left, right| {
        left.from_node_id
            .cmp(&right.from_node_id)
            .then_with(|| left.to_node_id.cmp(&right.to_node_id))
            .then_with(|| {
                context_query_edge_type_sort_key(left.edge_type)
                    .cmp(&context_query_edge_type_sort_key(right.edge_type))
            })
            .then_with(|| {
                left.mount_reason
                    .map(context_mount_reason_sort_key)
                    .cmp(&right.mount_reason.map(context_mount_reason_sort_key))
            })
            .then_with(|| left.reason.cmp(&right.reason))
    });
    edges.dedup_by(|left, right| {
        left.from_node_id == right.from_node_id
            && left.to_node_id == right.to_node_id
            && left.edge_type == right.edge_type
            && left.mount_reason == right.mount_reason
            && left.reason == right.reason
    });
}

fn context_query_edge_type_sort_key(edge_type: ContextEdgeType) -> u8 {
    match edge_type {
        ContextEdgeType::Mounted => 0,
        ContextEdgeType::Related => 1,
        ContextEdgeType::CoveredBy => 2,
        ContextEdgeType::PromotedTo => 3,
    }
}

fn context_mount_reason_sort_key(mount_reason: ContextMountReason) -> u8 {
    match mount_reason {
        ContextMountReason::Local => 0,
        ContextMountReason::ForkSeed => 1,
        ContextMountReason::HandoffSeed => 2,
        ContextMountReason::RepoNeighbor => 3,
    }
}

fn context_thread_artifact_kind(document: &ContextDocument) -> ThreadArtifactKind {
    match document.kind {
        ContextKind::ThreadInsight => ThreadArtifactKind::Plan,
        ContextKind::ThreadFile => document
            .summary
            .as_deref()
            .filter(|summary| {
                summary.contains("read during thread") || summary.contains("viewed in thread")
            })
            .map(|_| ThreadArtifactKind::FileRead)
            .unwrap_or(ThreadArtifactKind::FileChange),
        ContextKind::ThreadSearch => document
            .location
            .as_deref()
            .filter(|location| *location == "context/graph")
            .map(|_| ThreadArtifactKind::GraphQuery)
            .or_else(|| {
                document
                    .summary
                    .as_deref()
                    .filter(|summary| summary.contains("context graph query"))
                    .map(|_| ThreadArtifactKind::GraphQuery)
            })
            .unwrap_or(ThreadArtifactKind::Search),
        ContextKind::ThreadTool => ThreadArtifactKind::ToolOutput,
        ContextKind::SharedThread | ContextKind::RepoContextFile => ThreadArtifactKind::Plan,
    }
}

fn context_repo_memory_kind(document: &ContextDocument) -> RepoMemoryKind {
    let location = document.location.as_deref().unwrap_or_default();
    if location.contains("/concepts/") {
        return RepoMemoryKind::Concept;
    }
    if location.contains("/decisions/") {
        return RepoMemoryKind::Decision;
    }
    if location.contains("/playbooks/") {
        return RepoMemoryKind::Playbook;
    }
    if location.contains("/hotspots/") {
        return RepoMemoryKind::Hotspot;
    }
    match inferred_context_write_kind(document).as_str() {
        "concept" => RepoMemoryKind::Concept,
        "decision" => RepoMemoryKind::Decision,
        "playbook" => RepoMemoryKind::Playbook,
        _ => RepoMemoryKind::Hotspot,
    }
}

fn collect_markdown_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_markdown_files(path.as_path(), out);
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            out.push(path);
        }
    }
}

fn repo_context_document(repo_root: &Path, path: &Path) -> Option<ContextDocument> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) => {
            warn!(error = %err, path = %path.display(), "failed to read repo context file");
            return None;
        }
    };

    let relative_path = path
        .strip_prefix(repo_root)
        .ok()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| path.to_path_buf());
    let (frontmatter, body) = split_optional_frontmatter(&content);
    let metadata = frontmatter
        .as_deref()
        .map(parse_repo_context_metadata)
        .unwrap_or_default();
    let body = normalize_repo_context_body(body.trim());
    let title = metadata
        .title
        .clone()
        .or_else(|| first_markdown_heading(body.as_str()))
        .or_else(|| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| relative_path.display().to_string());
    let location = relative_path.display().to_string();
    let summary = repo_context_summary(&metadata, body.as_str());
    let body = non_empty_string(truncate_context_body(body.as_str()));
    let search_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        title,
        summary.clone().unwrap_or_default(),
        location,
        metadata.source_refs.join("\n"),
        frontmatter.unwrap_or_default(),
        body.clone().unwrap_or_default()
    )
    .to_ascii_lowercase();

    Some(ContextDocument {
        ref_id: format!("ctx:file:{location}"),
        kind: ContextKind::RepoContextFile,
        title,
        summary,
        location: Some(location),
        body,
        search_text,
        graph: ContextDocumentGraphMetadata {
            branches: metadata.branches,
            source_threads: metadata.source_threads,
            source_files: metadata.source_files,
            source_refs: metadata.source_refs,
        },
    })
}

fn repo_context_summary(metadata: &RepoContextMetadata, body: &str) -> Option<String> {
    let summary_line = first_meaningful_body_line(body)
        .filter(|line| !is_repo_context_detail_line(line))
        .or_else(|| repo_context_metadata_summary_line(metadata));
    match (metadata.kind.as_deref(), summary_line) {
        (Some(kind), Some(line)) => Some(format!("{kind} · {line}")),
        (Some(kind), None) => Some(kind.to_string()),
        (None, Some(line)) => Some(line),
        (None, None) => None,
    }
}

fn repo_context_metadata_summary_line(metadata: &RepoContextMetadata) -> Option<String> {
    let mut parts = Vec::new();
    match metadata.branches.as_slice() {
        [branch] => parts.push(format!("branch={branch}")),
        branches if !branches.is_empty() => parts.push(format!("branches={}", branches.len())),
        _ => {}
    }
    match metadata.source_threads.as_slice() {
        [_] => parts.push("1 source thread".to_string()),
        threads if !threads.is_empty() => parts.push(format!("{} source threads", threads.len())),
        _ => {}
    }
    match metadata.source_files.as_slice() {
        [_] => parts.push("1 source file".to_string()),
        files if !files.is_empty() => parts.push(format!("{} source files", files.len())),
        _ => {}
    }
    match metadata.source_refs.as_slice() {
        [_] => parts.push("1 source ref".to_string()),
        refs if !refs.is_empty() => parts.push(format!("{} source refs", refs.len())),
        _ => {}
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn split_optional_frontmatter(content: &str) -> (Option<String>, &str) {
    let Some(rest) = content.strip_prefix("---\n") else {
        return (None, content);
    };
    let Some(frontmatter_end) = rest.find("\n---\n") else {
        return (None, content);
    };
    let frontmatter = rest[..frontmatter_end].to_string();
    let body_start = frontmatter_end + "\n---\n".len();
    (Some(frontmatter), &rest[body_start..])
}

fn parse_repo_context_metadata(frontmatter: &str) -> RepoContextMetadata {
    #[derive(Clone, Copy)]
    enum RepoContextList {
        Branches,
        SourceThreads,
        SourceFiles,
        SourceRefs,
    }

    let mut metadata = RepoContextMetadata::default();
    let mut in_applies_to = false;
    let mut current_list = None;
    for line in frontmatter.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(value) = trimmed.strip_prefix("- ") {
            let value = strip_yaml_quotes(value).to_string();
            match current_list {
                Some(RepoContextList::Branches) => metadata.branches.push(value),
                Some(RepoContextList::SourceThreads) => metadata.source_threads.push(value),
                Some(RepoContextList::SourceFiles) => metadata.source_files.push(value),
                Some(RepoContextList::SourceRefs) => metadata.source_refs.push(value),
                None => {}
            }
            continue;
        }

        current_list = None;
        if indent == 0 {
            in_applies_to = trimmed == "applies_to:";
        }

        if let Some(value) = trimmed.strip_prefix("id:") {
            metadata.id = non_empty_string(strip_yaml_quotes(value).to_string());
        } else if let Some(value) = trimmed.strip_prefix("title:") {
            metadata.title = non_empty_string(strip_yaml_quotes(value).to_string());
        } else if let Some(value) = trimmed.strip_prefix("kind:") {
            metadata.kind = non_empty_string(strip_yaml_quotes(value).to_string());
        } else if indent == 0 {
            if let Some(value) = trimmed.strip_prefix("source_threads:")
                && value.trim().is_empty()
            {
                current_list = Some(RepoContextList::SourceThreads);
            } else if let Some(value) = trimmed.strip_prefix("source_files:")
                && value.trim().is_empty()
            {
                current_list = Some(RepoContextList::SourceFiles);
            } else if let Some(value) = trimmed.strip_prefix("source_refs:")
                && value.trim().is_empty()
            {
                current_list = Some(RepoContextList::SourceRefs);
            }
        } else if in_applies_to && indent == 2 && trimmed == "branches:" {
            current_list = Some(RepoContextList::Branches);
        }
    }
    metadata
}

fn strip_yaml_quotes(value: &str) -> &str {
    value.trim().trim_matches('"').trim_matches('\'')
}

fn first_markdown_heading(body: &str) -> Option<String> {
    body.lines()
        .find_map(|line| line.trim().strip_prefix("# ").map(str::trim))
        .map(str::to_string)
}

fn first_meaningful_body_line(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
}

fn normalize_repo_context_body(body: &str) -> String {
    let mut normalized = Vec::new();
    let mut previous_blank = false;
    for raw_line in body.lines() {
        let trimmed = raw_line.trim();
        if is_legacy_repo_context_line(trimmed) {
            continue;
        }
        if trimmed.is_empty() {
            if !previous_blank {
                normalized.push(String::new());
                previous_blank = true;
            }
            continue;
        }
        normalized.push(raw_line.to_string());
        previous_blank = false;
    }

    while normalized.first().is_some_and(String::is_empty) {
        normalized.remove(0);
    }
    while normalized.last().is_some_and(String::is_empty) {
        normalized.pop();
    }
    normalized.join("\n")
}

fn is_legacy_repo_context_line(line: &str) -> bool {
    line.starts_with("owner=")
        || matches!(
            line,
            line if line.starts_with("Owner:")
                || line.starts_with("Shared by:")
                || line.starts_with("Shared at:")
                || line.starts_with("Visibility:")
        )
}

fn is_repo_context_detail_line(line: &str) -> bool {
    matches!(
        line,
        line if line.starts_with("Thread:")
            || line.starts_with("Preview:")
            || line.starts_with("Repo root:")
            || line.starts_with("Git branch:")
            || line.starts_with("Git SHA:")
            || line.starts_with("Git origin:")
            || line.starts_with("Recent transcript:")
            || line.starts_with("- thread:")
            || line.starts_with("- repo context:")
            || line.starts_with("- ref:")
            || line.starts_with("- shared thread:")
    )
}

fn thread_context_title(thread: &Thread, is_current_thread: bool) -> String {
    thread
        .name
        .clone()
        .and_then(non_empty_string)
        .unwrap_or_else(|| {
            if is_current_thread {
                "Current Thread".to_string()
            } else {
                let short_id = thread.id.chars().take(8).collect::<String>();
                format!("Thread {short_id}")
            }
        })
}

fn agent_thread_title(
    base_title: String,
    agent_nickname: Option<&str>,
    agent_role: Option<&str>,
) -> String {
    let Some(agent_label) = agent_nickname
        .filter(|label| !label.is_empty())
        .or(agent_role.filter(|label| !label.is_empty()))
    else {
        return base_title;
    };

    if base_title.eq_ignore_ascii_case(agent_label) {
        format!("🦞 {base_title}")
    } else {
        format!("🦞 {agent_label} · {base_title}")
    }
}

fn thread_context_summary(thread: &Thread) -> String {
    let mut parts = Vec::new();
    if let Some(nickname) = thread.agent_nickname.as_deref() {
        parts.push(format!("agent={nickname}"));
    }
    if let Some(role) = thread.agent_role.as_deref() {
        parts.push(format!("role={role}"));
    }
    parts.push(format!("cwd={}", thread.cwd.display()));
    parts.push(format!("updated_at={}", thread.updated_at));
    if let Some(branch) = thread
        .git_info
        .as_ref()
        .and_then(|info| info.branch.as_deref())
    {
        parts.push(format!("branch={branch}"));
    }
    parts.join(" · ")
}

fn thread_context_search_text(
    thread: &Thread,
    title: &str,
    location: &str,
    body: Option<&str>,
) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        title,
        thread_context_summary(thread),
        location,
        thread.id,
        thread.cwd.display(),
        thread.agent_role.clone().unwrap_or_default(),
        thread.agent_nickname.clone().unwrap_or_default(),
        thread
            .git_info
            .as_ref()
            .and_then(|info| info.branch.clone())
            .unwrap_or_default(),
        thread
            .git_info
            .as_ref()
            .and_then(|info| info.sha.clone())
            .unwrap_or_default(),
        body.unwrap_or_default()
    )
    .to_ascii_lowercase()
}

fn thread_context_body(
    thread: &Thread,
    insight_count: usize,
    file_count: usize,
    search_count: usize,
    tool_count: usize,
    artifact_documents: &[ContextDocument],
) -> Option<String> {
    let mut lines = vec![
        format!("Thread: {}", thread.id),
        format!("Cwd: {}", thread.cwd.display()),
        format!("Updated at: {}", thread.updated_at),
    ];
    if let Some(name) = &thread.name {
        lines.push(format!("Title: {name}"));
    }
    if let Some(role) = thread.agent_role.as_deref() {
        lines.push(format!("Agent role: {role}"));
    }
    if let Some(nickname) = thread.agent_nickname.as_deref() {
        lines.push(format!("Agent: {nickname}"));
    }
    if let Some(git_info) = &thread.git_info {
        if let Some(branch) = git_info.branch.as_deref() {
            lines.push(format!("Git branch: {branch}"));
        }
        if let Some(sha) = git_info.sha.as_deref() {
            lines.push(format!("Git SHA: {sha}"));
        }
        if let Some(origin_url) = git_info.origin_url.as_deref() {
            lines.push(format!("Git origin: {origin_url}"));
        }
    }
    if !artifact_documents.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "Retained context: {insight_count} insight(s) · {file_count} file(s) · {search_count} search result(s) · {tool_count} tool result(s)"
        ));
        for document in artifact_documents.iter().take(6) {
            let kind = match document.kind {
                ContextKind::ThreadInsight => "insight",
                ContextKind::ThreadFile => "file",
                ContextKind::ThreadSearch => "search",
                ContextKind::ThreadTool => "tool",
                ContextKind::SharedThread | ContextKind::RepoContextFile => "context",
            };
            lines.push(format!("- {kind}: {}", document.title));
        }
    }

    non_empty_string(truncate_context_body(&lines.join("\n")))
}

fn thread_artifact_documents(thread: &Thread, repo_root: Option<&Path>) -> Vec<ContextDocument> {
    let branches = thread
        .git_info
        .as_ref()
        .and_then(|info| info.branch.clone())
        .into_iter()
        .collect::<Vec<_>>();
    let mut documents_by_ref_id = HashMap::<String, ContextDocument>::new();

    for turn in &thread.turns {
        for item in &turn.items {
            match item {
                ThreadItem::Plan { id, text } => {
                    if let Some(body) = non_empty_string(text.clone()) {
                        let title = body
                            .lines()
                            .map(str::trim)
                            .find(|line| !line.is_empty())
                            .map(|line| single_line_excerpt(line, 80))
                            .unwrap_or_else(|| "Plan update".to_string());
                        let summary = Some("thread insight · retained plan output".to_string());
                        let location = Some(format!("insight/{id}"));
                        let body = Some(truncate_context_body(body.as_str()));
                        let search_text = format!(
                            "{}\n{}\n{}\n{}",
                            title,
                            summary.clone().unwrap_or_default(),
                            location.clone().unwrap_or_default(),
                            body.clone().unwrap_or_default()
                        )
                        .to_ascii_lowercase();
                        documents_by_ref_id.insert(
                            format!("ctx:thread-insight:{}:{id}", thread.id),
                            ContextDocument {
                                ref_id: format!("ctx:thread-insight:{}:{id}", thread.id),
                                kind: ContextKind::ThreadInsight,
                                title,
                                summary,
                                location,
                                body,
                                search_text,
                                graph: ContextDocumentGraphMetadata {
                                    branches: branches.clone(),
                                    source_threads: vec![thread.id.clone()],
                                    ..ContextDocumentGraphMetadata::default()
                                },
                            },
                        );
                    }
                }
                ThreadItem::FileChange {
                    changes, status, ..
                } if !matches!(
                    status,
                    PatchApplyStatus::Failed | PatchApplyStatus::Declined
                ) =>
                {
                    for change in changes {
                        if let Some(location) = normalize_thread_file_location(
                            change.path.as_str(),
                            repo_root,
                            &thread.cwd,
                        ) {
                            let summary = Some(match &change.kind {
                                codex_app_server_protocol::PatchChangeKind::Add => {
                                    "linked file · added in thread".to_string()
                                }
                                codex_app_server_protocol::PatchChangeKind::Delete => {
                                    "linked file · deleted in thread".to_string()
                                }
                                codex_app_server_protocol::PatchChangeKind::Update {
                                    move_path,
                                } => match move_path {
                                    Some(move_path) => {
                                        format!("linked file · moved from {}", move_path.display())
                                    }
                                    None => "linked file · updated in thread".to_string(),
                                },
                            });
                            upsert_thread_file_document(
                                &mut documents_by_ref_id,
                                thread,
                                &branches,
                                location,
                                summary,
                                context_excerpt(change.diff.as_str(), 12, 1_200),
                            );
                        }
                    }
                }
                ThreadItem::CommandExecution {
                    id,
                    status: CommandExecutionStatus::Completed,
                    command_actions,
                    aggregated_output,
                    ..
                } => {
                    let output_excerpt = aggregated_output
                        .as_deref()
                        .and_then(|output| context_excerpt(output, 14, 1_600));
                    for action in command_actions {
                        match action {
                            CommandAction::Read { path, .. } => {
                                if let Some(location) =
                                    normalize_thread_path_location(path, repo_root, &thread.cwd)
                                {
                                    upsert_thread_file_document(
                                        &mut documents_by_ref_id,
                                        thread,
                                        &branches,
                                        location,
                                        Some("linked file · read during thread".to_string()),
                                        output_excerpt.clone(),
                                    );
                                }
                            }
                            CommandAction::Search { path, .. } => {
                                let Some(body) = output_excerpt.clone() else {
                                    continue;
                                };
                                let location = path
                                    .as_deref()
                                    .and_then(|value| {
                                        normalize_thread_file_location(
                                            value,
                                            repo_root,
                                            &thread.cwd,
                                        )
                                    })
                                    .unwrap_or_else(|| format!("search/{id}"));
                                let title = if location.starts_with("search/") {
                                    "Search results".to_string()
                                } else {
                                    format!("Search results in {location}")
                                };
                                let summary = Some(
                                    "thread search result · retained command output".to_string(),
                                );
                                let search_text = format!(
                                    "{}\n{}\n{}\n{}",
                                    title,
                                    summary.clone().unwrap_or_default(),
                                    location,
                                    body
                                )
                                .to_ascii_lowercase();
                                documents_by_ref_id.insert(
                                    format!("ctx:thread-search:{}:{id}", thread.id),
                                    ContextDocument {
                                        ref_id: format!("ctx:thread-search:{}:{id}", thread.id),
                                        kind: ContextKind::ThreadSearch,
                                        title,
                                        summary,
                                        location: Some(location),
                                        body: Some(body),
                                        search_text,
                                        graph: ContextDocumentGraphMetadata {
                                            branches: branches.clone(),
                                            source_threads: vec![thread.id.clone()],
                                            ..ContextDocumentGraphMetadata::default()
                                        },
                                    },
                                );
                            }
                            CommandAction::ListFiles { .. } | CommandAction::Unknown { .. } => {}
                        }
                    }
                }
                ThreadItem::DynamicToolCall {
                    id,
                    tool,
                    status,
                    content_items,
                    success,
                    ..
                } if matches!(status, DynamicToolCallStatus::Completed)
                    && success.unwrap_or(false) =>
                {
                    let body = content_items.as_ref().and_then(|items| {
                        context_excerpt(
                            &items
                                .iter()
                                .filter_map(|item| match item {
                                    DynamicToolCallOutputContentItem::InputText { text } => {
                                        non_empty_string(text.clone())
                                    }
                                    DynamicToolCallOutputContentItem::InputImage { image_url } => {
                                        Some(format!("[image] {image_url}"))
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join("\n"),
                            12,
                            1_200,
                        )
                    });
                    if let Some(body) = body {
                        let title = format!("{tool} result");
                        let summary = Some("thread tool result · retained tool output".to_string());
                        let location = Some(format!("tool/{tool}"));
                        let search_text = format!(
                            "{}\n{}\n{}\n{}",
                            title,
                            summary.clone().unwrap_or_default(),
                            location.clone().unwrap_or_default(),
                            body
                        )
                        .to_ascii_lowercase();
                        documents_by_ref_id.insert(
                            format!("ctx:thread-tool:{}:{id}", thread.id),
                            ContextDocument {
                                ref_id: format!("ctx:thread-tool:{}:{id}", thread.id),
                                kind: ContextKind::ThreadTool,
                                title,
                                summary,
                                location,
                                body: Some(body),
                                search_text,
                                graph: ContextDocumentGraphMetadata {
                                    branches: branches.clone(),
                                    source_threads: vec![thread.id.clone()],
                                    ..ContextDocumentGraphMetadata::default()
                                },
                            },
                        );
                    }
                }
                ThreadItem::McpToolCall {
                    id,
                    server,
                    tool,
                    status: McpToolCallStatus::Completed,
                    result,
                    ..
                } => {
                    let body = result.as_ref().and_then(|result| {
                        let mut parts = result
                            .content
                            .iter()
                            .filter_map(|value| serde_json::to_string_pretty(value).ok())
                            .collect::<Vec<_>>();
                        if let Some(structured_content) = &result.structured_content
                            && let Ok(value) = serde_json::to_string_pretty(structured_content)
                        {
                            parts.push(value);
                        }
                        context_excerpt(parts.join("\n\n").as_str(), 12, 1_200)
                    });
                    if let Some(body) = body {
                        let title = format!("{server}/{tool} result");
                        let summary = Some("thread tool result · retained MCP output".to_string());
                        let location = Some(format!("mcp/{server}/{tool}"));
                        let search_text = format!(
                            "{}\n{}\n{}\n{}",
                            title,
                            summary.clone().unwrap_or_default(),
                            location.clone().unwrap_or_default(),
                            body
                        )
                        .to_ascii_lowercase();
                        documents_by_ref_id.insert(
                            format!("ctx:thread-tool:{}:{id}", thread.id),
                            ContextDocument {
                                ref_id: format!("ctx:thread-tool:{}:{id}", thread.id),
                                kind: ContextKind::ThreadTool,
                                title,
                                summary,
                                location,
                                body: Some(body),
                                search_text,
                                graph: ContextDocumentGraphMetadata {
                                    branches: branches.clone(),
                                    source_threads: vec![thread.id.clone()],
                                    ..ContextDocumentGraphMetadata::default()
                                },
                            },
                        );
                    }
                }
                ThreadItem::ImageView { path, .. } => {
                    if let Some(location) =
                        normalize_thread_file_location(path, repo_root, &thread.cwd)
                    {
                        upsert_thread_file_document(
                            &mut documents_by_ref_id,
                            thread,
                            &branches,
                            location,
                            Some("linked file · viewed in thread".to_string()),
                            None,
                        );
                    }
                }
                ThreadItem::WebSearch { id, query, action } => {
                    let mut body_lines = vec![format!("Query: {query}")];
                    let (summary, location) = match action {
                        Some(codex_app_server_protocol::WebSearchAction::Search {
                            query: action_query,
                            queries,
                        }) => {
                            if let Some(action_query) =
                                action_query.as_deref().filter(|value| !value.is_empty())
                                && action_query != query
                            {
                                body_lines.push(format!("Primary query: {action_query}"));
                            }
                            if let Some(queries) =
                                queries.as_ref().filter(|values| !values.is_empty())
                            {
                                body_lines.push(format!("Queries: {}", queries.join(", ")));
                            }
                            (
                                Some("thread search result · retained web search".to_string()),
                                Some("web/search".to_string()),
                            )
                        }
                        Some(codex_app_server_protocol::WebSearchAction::OpenPage { url }) => {
                            if let Some(url) = url.as_deref().filter(|value| !value.is_empty()) {
                                body_lines.push(format!("Opened page: {url}"));
                            }
                            (
                                Some("thread search result · retained opened page".to_string()),
                                Some("web/open-page".to_string()),
                            )
                        }
                        Some(codex_app_server_protocol::WebSearchAction::FindInPage {
                            url,
                            pattern,
                        }) => {
                            if let Some(url) = url.as_deref().filter(|value| !value.is_empty()) {
                                body_lines.push(format!("Page: {url}"));
                            }
                            if let Some(pattern) =
                                pattern.as_deref().filter(|value| !value.is_empty())
                            {
                                body_lines.push(format!("Find: {pattern}"));
                            }
                            (
                                Some("thread search result · retained find-in-page".to_string()),
                                Some("web/find-in-page".to_string()),
                            )
                        }
                        Some(codex_app_server_protocol::WebSearchAction::Other) | None => (
                            Some("thread search result · retained web search".to_string()),
                            Some("web/search".to_string()),
                        ),
                    };
                    let body = context_excerpt(body_lines.join("\n").as_str(), 12, 1_200);
                    let title = single_line_excerpt(query, 80);
                    let search_text = format!(
                        "{}\n{}\n{}\n{}",
                        title,
                        summary.clone().unwrap_or_default(),
                        location.clone().unwrap_or_default(),
                        body.clone().unwrap_or_default()
                    )
                    .to_ascii_lowercase();
                    documents_by_ref_id.insert(
                        format!("ctx:thread-search:{}:{id}", thread.id),
                        ContextDocument {
                            ref_id: format!("ctx:thread-search:{}:{id}", thread.id),
                            kind: ContextKind::ThreadSearch,
                            title,
                            summary,
                            location,
                            body,
                            search_text,
                            graph: ContextDocumentGraphMetadata {
                                branches: branches.clone(),
                                source_threads: vec![thread.id.clone()],
                                ..ContextDocumentGraphMetadata::default()
                            },
                        },
                    );
                }
                ThreadItem::ContextGraphQuery {
                    id,
                    operation,
                    scope,
                    query,
                    ref_ids,
                    result_ref_ids,
                    summary,
                    success,
                } if *success => {
                    let detail = query
                        .as_deref()
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                        .or_else(|| ref_ids.first().cloned());
                    let body = context_excerpt(
                        &[
                            summary.clone().unwrap_or_default(),
                            format!("Operation: {operation:?}"),
                            format!("Scope: {scope:?}"),
                            if ref_ids.is_empty() {
                                String::new()
                            } else {
                                format!("Refs: {}", ref_ids.join(", "))
                            },
                            if result_ref_ids.is_empty() {
                                String::new()
                            } else {
                                format!("Results: {}", result_ref_ids.join(", "))
                            },
                        ]
                        .join("\n"),
                        12,
                        1_200,
                    );
                    let title = detail
                        .filter(|value| !value.is_empty())
                        .map(|value| format!("Context graph: {value}"))
                        .unwrap_or_else(|| "Context graph traversal".to_string());
                    let summary = Some(
                        summary
                            .clone()
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or_else(|| {
                                "thread search result · retained context graph query".to_string()
                            }),
                    );
                    let search_text = format!(
                        "{}\n{}\n{}\n{}",
                        title,
                        summary.clone().unwrap_or_default(),
                        "context/graph",
                        body.clone().unwrap_or_default()
                    )
                    .to_ascii_lowercase();
                    documents_by_ref_id.insert(
                        format!("ctx:thread-search:{}:{id}", thread.id),
                        ContextDocument {
                            ref_id: format!("ctx:thread-search:{}:{id}", thread.id),
                            kind: ContextKind::ThreadSearch,
                            title,
                            summary,
                            location: Some("context/graph".to_string()),
                            body,
                            search_text,
                            graph: ContextDocumentGraphMetadata {
                                branches: branches.clone(),
                                source_threads: vec![thread.id.clone()],
                                source_refs: result_ref_ids.clone(),
                                ..ContextDocumentGraphMetadata::default()
                            },
                        },
                    );
                }
                ThreadItem::CommandExecution { .. }
                | ThreadItem::FileChange { .. }
                | ThreadItem::McpToolCall { .. }
                | ThreadItem::DynamicToolCall { .. }
                | ThreadItem::ContextGraphQuery { .. } => {}
                ThreadItem::UserMessage { .. }
                | ThreadItem::AgentMessage { .. }
                | ThreadItem::Reasoning { .. }
                | ThreadItem::CollabAgentToolCall { .. }
                | ThreadItem::EnteredReviewMode { .. }
                | ThreadItem::ExitedReviewMode { .. }
                | ThreadItem::ContextCompaction { .. } => {}
            }
        }
    }

    if documents_by_ref_id.is_empty()
        && let Some((id, body, summary)) = thread.turns.iter().rev().find_map(|turn| {
            turn.items.iter().rev().find_map(|item| match item {
                ThreadItem::AgentMessage { id, text, .. } => {
                    non_empty_string(text.clone()).map(|body| {
                        (
                            id.clone(),
                            body,
                            "thread insight · recent assistant output".to_string(),
                        )
                    })
                }
                ThreadItem::UserMessage { id, content } => {
                    let body = content
                        .iter()
                        .filter_map(|input| match input {
                            codex_app_server_protocol::UserInput::Text { text, .. } => {
                                non_empty_string(text.clone())
                            }
                            codex_app_server_protocol::UserInput::Image { .. }
                            | codex_app_server_protocol::UserInput::LocalImage { .. }
                            | codex_app_server_protocol::UserInput::Skill { .. }
                            | codex_app_server_protocol::UserInput::Mention { .. } => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    non_empty_string(body).map(|body| {
                        (
                            id.clone(),
                            body,
                            "thread insight · recent user message".to_string(),
                        )
                    })
                }
                ThreadItem::Plan { .. }
                | ThreadItem::Reasoning { .. }
                | ThreadItem::CommandExecution { .. }
                | ThreadItem::FileChange { .. }
                | ThreadItem::McpToolCall { .. }
                | ThreadItem::DynamicToolCall { .. }
                | ThreadItem::ImageView { .. }
                | ThreadItem::WebSearch { .. }
                | ThreadItem::ContextGraphQuery { .. }
                | ThreadItem::CollabAgentToolCall { .. }
                | ThreadItem::EnteredReviewMode { .. }
                | ThreadItem::ExitedReviewMode { .. }
                | ThreadItem::ContextCompaction { .. } => None,
            })
        })
    {
        let title = body
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|line| single_line_excerpt(line, 80))
            .unwrap_or_else(|| "Recent thread insight".to_string());
        let location = Some(format!("message/{id}"));
        let body = Some(truncate_context_body(body.as_str()));
        let search_text = format!(
            "{}\n{}\n{}\n{}",
            title,
            summary,
            location.clone().unwrap_or_default(),
            body.clone().unwrap_or_default()
        )
        .to_ascii_lowercase();
        documents_by_ref_id.insert(
            format!("ctx:thread-insight:{}:{id}", thread.id),
            ContextDocument {
                ref_id: format!("ctx:thread-insight:{}:{id}", thread.id),
                kind: ContextKind::ThreadInsight,
                title,
                summary: Some(summary),
                location,
                body,
                search_text,
                graph: ContextDocumentGraphMetadata {
                    branches: branches.clone(),
                    source_threads: vec![thread.id.clone()],
                    ..ContextDocumentGraphMetadata::default()
                },
            },
        );
    }

    let mut documents = documents_by_ref_id.into_values().collect::<Vec<_>>();
    documents.sort_by_key(context_default_sort_key);
    documents
}

fn upsert_thread_file_document(
    documents_by_ref_id: &mut HashMap<String, ContextDocument>,
    thread: &Thread,
    branches: &[String],
    location: String,
    summary: Option<String>,
    body: Option<String>,
) {
    let ref_id = format!(
        "ctx:thread-file:{}:{}",
        thread.id,
        encode_context_ref_fragment(location.as_str())
    );
    let entry = documents_by_ref_id
        .entry(ref_id.clone())
        .or_insert_with(|| {
            let search_text = format!(
                "{}\n{}\n{}\n{}",
                location,
                summary.clone().unwrap_or_default(),
                location,
                body.clone().unwrap_or_default()
            )
            .to_ascii_lowercase();
            ContextDocument {
                ref_id: ref_id.clone(),
                kind: ContextKind::ThreadFile,
                title: location.clone(),
                summary: summary.clone(),
                location: Some(location.clone()),
                body: body.clone(),
                search_text,
                graph: ContextDocumentGraphMetadata {
                    branches: branches.to_vec(),
                    source_threads: vec![thread.id.clone()],
                    source_files: vec![location.clone()],
                    source_refs: Vec::new(),
                },
            }
        });

    if body.is_some() && entry.body.is_none() {
        entry.body = body;
    }
    if let Some(summary) = summary
        && entry
            .summary
            .as_deref()
            .is_none_or(|existing| !existing.contains("updated") && summary.contains("updated"))
    {
        entry.summary = Some(summary);
    }
    entry.search_text = format!(
        "{}\n{}\n{}\n{}",
        entry.title,
        entry.summary.clone().unwrap_or_default(),
        entry.location.clone().unwrap_or_default(),
        entry.body.clone().unwrap_or_default()
    )
    .to_ascii_lowercase();
}

fn normalize_thread_file_location(
    path: &str,
    repo_root: Option<&Path>,
    cwd: &Path,
) -> Option<String> {
    normalize_thread_path_location(Path::new(path), repo_root, cwd)
}

fn normalize_thread_path_location(
    path: &Path,
    repo_root: Option<&Path>,
    cwd: &Path,
) -> Option<String> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    if let Some(repo_root) = repo_root {
        return candidate
            .strip_prefix(repo_root)
            .ok()
            .map(|relative| relative.display().to_string());
    }

    Some(
        path.to_str()
            .map(str::to_string)
            .unwrap_or_else(|| candidate.display().to_string()),
    )
}

fn context_excerpt(text: &str, max_lines: usize, max_chars: usize) -> Option<String> {
    let excerpt = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n");
    if excerpt.is_empty() {
        return None;
    }
    let excerpt = if excerpt.chars().count() <= max_chars {
        excerpt
    } else {
        format!("{}…", excerpt.chars().take(max_chars).collect::<String>())
    };
    non_empty_string(truncate_context_body(excerpt.as_str()))
}

fn encode_context_ref_fragment(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(value.as_bytes())
}

fn truncate_context_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() <= CONTEXT_BODY_CHAR_LIMIT {
        return trimmed.to_string();
    }
    let truncated = trimmed
        .chars()
        .take(CONTEXT_BODY_CHAR_LIMIT)
        .collect::<String>();
    format!("{truncated}\n…")
}

fn single_line_excerpt(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    let truncated = collapsed.chars().take(max_chars).collect::<String>();
    format!("{truncated}…")
}

fn non_empty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn context_hotspot_scores(
    documents: &[ContextDocument],
    edges: &[ContextGraphEdge],
    selected_ref_ids: &HashSet<String>,
) -> HashMap<String, usize> {
    let mut degree_by_ref_id = HashMap::<String, usize>::new();
    for edge in edges {
        *degree_by_ref_id
            .entry(edge.from_ref_id.clone())
            .or_default() += 1;
        *degree_by_ref_id.entry(edge.to_ref_id.clone()).or_default() += 1;
    }

    documents
        .iter()
        .map(|document| {
            let degree = degree_by_ref_id
                .get(&document.ref_id)
                .copied()
                .unwrap_or_default();
            let mut score = degree.saturating_mul(3);
            score += match document.kind {
                ContextKind::ThreadFile => 8,
                ContextKind::ThreadInsight => 7,
                ContextKind::RepoContextFile => 6,
                ContextKind::ThreadSearch | ContextKind::ThreadTool => 4,
                ContextKind::SharedThread => 1,
            };
            if selected_ref_ids.contains(&document.ref_id) {
                score += 5;
            }
            if !document.graph.source_files.is_empty() {
                score += 3;
            }
            if !document.graph.source_refs.is_empty() {
                score += 2;
            }
            let haystack = format!(
                "{} {} {}",
                document.title,
                document.summary.clone().unwrap_or_default(),
                document.body.clone().unwrap_or_default()
            )
            .to_ascii_lowercase();
            if haystack.contains("decision") || haystack.contains("tradeoff") {
                score += 3;
            }
            if haystack.contains("hotspot")
                || haystack.contains("failure")
                || haystack.contains("sharp edge")
            {
                score += 3;
            }
            if haystack.contains("playbook")
                || haystack.contains("workflow")
                || haystack.contains("debug")
            {
                score += 2;
            }
            (document.ref_id.clone(), score)
        })
        .collect()
}

fn context_hotspot_reason(
    document: &ContextDocument,
    edges: &[ContextGraphEdge],
    selected_ref_ids: &HashSet<String>,
) -> String {
    let linked_count = edges
        .iter()
        .filter(|edge| edge.from_ref_id == document.ref_id || edge.to_ref_id == document.ref_id)
        .count();
    if !document.graph.source_files.is_empty() {
        format!("grounded in {} file(s)", document.graph.source_files.len())
    } else if !document.graph.source_refs.is_empty() {
        format!(
            "derived from {} graph ref(s)",
            document.graph.source_refs.len()
        )
    } else if selected_ref_ids.contains(&document.ref_id) {
        "explicitly selected for this turn".to_string()
    } else if linked_count > 0 {
        format!("linked to {linked_count} nearby node(s)")
    } else {
        "recent context evidence".to_string()
    }
}
use serde::Serialize;

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::GitInfo;
    use codex_app_server_protocol::ThreadStatus;
    use codex_app_server_protocol::Turn;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    fn sample_thread(items: Vec<ThreadItem>) -> Thread {
        Thread {
            id: "thread-1".to_string(),
            preview: "preview".to_string(),
            ephemeral: false,
            model_provider: "openai".to_string(),
            created_at: 1,
            updated_at: 2,
            status: ThreadStatus::NotLoaded,
            path: None,
            cwd: PathBuf::from("/repo"),
            cli_version: "0.0.0".to_string(),
            source: codex_app_server_protocol::SessionSource::Exec,
            agent_nickname: None,
            agent_role: None,
            git_info: Some(GitInfo {
                branch: Some("main".to_string()),
                sha: Some("abcdef12".to_string()),
                origin_url: None,
            }),
            name: None,
            turns: vec![Turn {
                id: "turn-1".to_string(),
                items,
                status: codex_app_server_protocol::TurnStatus::Completed,
                error: None,
            }],
        }
    }

    #[test]
    fn thread_context_documents_capture_context_graph_queries() {
        let documents = thread_context_documents(
            &sample_thread(vec![ThreadItem::ContextGraphQuery {
                id: "query-1".to_string(),
                operation: codex_app_server_protocol::ContextGraphQueryOperation::Search,
                scope: codex_app_server_protocol::ContextGraphQueryScope::Local,
                query: Some("context graph".to_string()),
                ref_ids: vec!["ctx:file:.codex/context/a.md".to_string()],
                result_ref_ids: vec!["ctx:file:.codex/context/a.md".to_string()],
                summary: Some("1 local context match for context graph".to_string()),
                success: true,
            }]),
            true,
            Some(Path::new("/repo")),
        );

        assert!(documents.iter().any(|document| {
            document.kind == ContextKind::ThreadSearch
                && document.title == "Context graph: context graph"
                && document.graph.source_refs == vec!["ctx:file:.codex/context/a.md".to_string()]
        }));
    }

    #[test]
    fn thread_context_documents_synthesize_recent_message_insight_without_artifacts() {
        let documents = thread_context_documents(
            &sample_thread(vec![
                ThreadItem::UserMessage {
                    id: "user-1".to_string(),
                    content: vec![codex_app_server_protocol::UserInput::Text {
                        text: "Inspect /context behavior".to_string(),
                        text_elements: Vec::new(),
                    }],
                },
                ThreadItem::AgentMessage {
                    id: "assistant-1".to_string(),
                    text: "Explained why /context looked empty after a prose-only turn."
                        .to_string(),
                    phase: None,
                },
            ]),
            true,
            Some(Path::new("/repo")),
        );

        assert!(
            documents
                .iter()
                .any(|document| document.kind == ContextKind::SharedThread)
        );
        assert_eq!(
            documents
                .into_iter()
                .find(|document| document.kind == ContextKind::ThreadInsight),
            Some(ContextDocument {
                ref_id: "ctx:thread-insight:thread-1:assistant-1".to_string(),
                kind: ContextKind::ThreadInsight,
                title: "Explained why /context looked empty after a prose-only turn.".to_string(),
                summary: Some("thread insight · recent assistant output".to_string()),
                location: Some("message/assistant-1".to_string()),
                body: Some("Explained why /context looked empty after a prose-only turn.".to_string()),
                search_text: "explained why /context looked empty after a prose-only turn.\nthread insight · recent assistant output\nmessage/assistant-1\nexplained why /context looked empty after a prose-only turn.".to_string(),
                graph: ContextDocumentGraphMetadata {
                    branches: vec!["main".to_string()],
                    source_threads: vec!["thread-1".to_string()],
                    source_files: Vec::new(),
                    source_refs: Vec::new(),
                },
            })
        );
    }

    #[test]
    fn repo_context_documents_parse_frontmatter() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join(".codex/context/concepts/plan.md");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdirs");
        std::fs::write(
            &path,
            "---\ntitle: Planning Notes\nkind: concept\nsource_refs:\n  - \"ctx:thread-insight:thread-1:plan-1\"\n---\n# Planning Notes\n\nShip it.\n",
        )
        .expect("write");

        let documents = repo_context_documents(temp.path());
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].title, "Planning Notes");
        assert_eq!(
            documents[0].graph.source_refs,
            vec!["ctx:thread-insight:thread-1:plan-1".to_string()]
        );
    }

    #[test]
    fn local_scope_documents_only_return_thread_artifacts() {
        let documents = vec![
            ContextDocument {
                ref_id: "ctx:thread:thread-1".to_string(),
                kind: ContextKind::SharedThread,
                title: "Current Thread".to_string(),
                summary: Some("thread".to_string()),
                location: Some("thread/thread-1".to_string()),
                body: None,
                search_text: "current thread".to_string(),
                graph: ContextDocumentGraphMetadata {
                    source_threads: vec!["thread-1".to_string()],
                    ..ContextDocumentGraphMetadata::default()
                },
            },
            ContextDocument {
                ref_id: "ctx:file:.codex/context/overview.md".to_string(),
                kind: ContextKind::RepoContextFile,
                title: "Planning Overview".to_string(),
                summary: Some("repo note".to_string()),
                location: Some(".codex/context/overview.md".to_string()),
                body: None,
                search_text: "planning overview".to_string(),
                graph: ContextDocumentGraphMetadata {
                    source_threads: vec!["thread-1".to_string()],
                    ..ContextDocumentGraphMetadata::default()
                },
            },
            ContextDocument {
                ref_id: "ctx:thread-search:thread-1:search-1".to_string(),
                kind: ContextKind::ThreadSearch,
                title: "robot dog search".to_string(),
                summary: Some("search".to_string()),
                location: Some("web/search".to_string()),
                body: None,
                search_text: "robot dog search".to_string(),
                graph: ContextDocumentGraphMetadata {
                    source_threads: vec!["thread-1".to_string()],
                    ..ContextDocumentGraphMetadata::default()
                },
            },
        ];

        assert_eq!(
            local_scope_documents(&documents, Some("thread-1")),
            vec![documents[2].clone()]
        );
    }

    #[test]
    fn local_scope_documents_are_empty_without_thread_artifacts() {
        let documents = vec![
            ContextDocument {
                ref_id: "ctx:thread:thread-1".to_string(),
                kind: ContextKind::SharedThread,
                title: "Current Thread".to_string(),
                summary: Some("thread".to_string()),
                location: Some("thread/thread-1".to_string()),
                body: None,
                search_text: "current thread".to_string(),
                graph: ContextDocumentGraphMetadata {
                    source_threads: vec!["thread-1".to_string()],
                    ..ContextDocumentGraphMetadata::default()
                },
            },
            ContextDocument {
                ref_id: "ctx:file:.codex/context/overview.md".to_string(),
                kind: ContextKind::RepoContextFile,
                title: "Planning Overview".to_string(),
                summary: Some("repo note".to_string()),
                location: Some(".codex/context/overview.md".to_string()),
                body: None,
                search_text: "planning overview".to_string(),
                graph: ContextDocumentGraphMetadata {
                    source_threads: vec!["thread-1".to_string()],
                    ..ContextDocumentGraphMetadata::default()
                },
            },
        ];

        assert_eq!(
            local_scope_documents(&documents, Some("thread-1")),
            Vec::new()
        );
        assert_eq!(local_scope_documents(&documents, None), Vec::new());
    }

    #[test]
    fn build_context_query_returns_rooted_projection_with_mount_metadata() {
        let seed_ref_id = "ctx:thread-insight:thread-1:plan-1".to_string();
        let local_file_ref_id = "ctx:thread-file:thread-2:tui-src-chatwidget-rs".to_string();
        let repo_ref_id = "ctx:file:.codex/context/playbooks/handoff-selection-flow.md".to_string();
        let response = build_context_query(
            vec![
                ContextDocument {
                    ref_id: seed_ref_id.clone(),
                    kind: ContextKind::ThreadInsight,
                    title: "Simplify /context selection flow".to_string(),
                    summary: Some("thread insight · retained plan output".to_string()),
                    location: Some("insight/plan-1".to_string()),
                    body: Some("Only show one-line nodes and let Enter toggle selection.".into()),
                    search_text: "simplify context selection".to_string(),
                    graph: ContextDocumentGraphMetadata {
                        source_threads: vec!["thread-1".to_string()],
                        source_files: vec!["tui/src/chatwidget.rs".to_string()],
                        ..ContextDocumentGraphMetadata::default()
                    },
                },
                ContextDocument {
                    ref_id: local_file_ref_id.clone(),
                    kind: ContextKind::ThreadFile,
                    title: "tui/src/chatwidget.rs".to_string(),
                    summary: Some("linked file · updated in thread".to_string()),
                    location: Some("tui/src/chatwidget.rs".to_string()),
                    body: Some("Adjusted the /context selection view.".to_string()),
                    search_text: "chatwidget updated".to_string(),
                    graph: ContextDocumentGraphMetadata {
                        source_threads: vec!["thread-2".to_string()],
                        source_files: vec!["tui/src/chatwidget.rs".to_string()],
                        ..ContextDocumentGraphMetadata::default()
                    },
                },
                ContextDocument {
                    ref_id: repo_ref_id.clone(),
                    kind: ContextKind::RepoContextFile,
                    title: "Handoff selection flow".to_string(),
                    summary: Some("playbook · one-line context selection".to_string()),
                    location: Some(
                        ".codex/context/playbooks/handoff-selection-flow.md".to_string(),
                    ),
                    body: Some("Document the simplified handoff selection flow.".to_string()),
                    search_text: "handoff selection playbook".to_string(),
                    graph: ContextDocumentGraphMetadata {
                        source_threads: vec!["thread-1".to_string()],
                        source_files: vec!["tui/src/chatwidget.rs".to_string()],
                        source_refs: vec![seed_ref_id.clone()],
                        ..ContextDocumentGraphMetadata::default()
                    },
                },
            ],
            &ContextQueryParams {
                current_thread_id: Some("thread-2".to_string()),
                precursor_thread_id: Some("thread-1".to_string()),
                precursor_kind: Some(ContextPrecursorKind::Handoff),
                actor_id: Some("reviewer@local".to_string()),
                repo_root: Some("/repo".to_string()),
                git_branch: Some("rewrite-codex-2gether-v2".to_string()),
                goal: Some("Verify the simplified /context and /handoff flow.".to_string()),
                query: None,
                seed_ref_ids: vec![seed_ref_id.clone()],
                limit: Some(10),
            },
        );

        assert_eq!(
            response.anchor,
            ContextQueryAnchor {
                anchor_id: "anchor:thread-2".to_string(),
                current_thread_id: Some("thread-2".to_string()),
                precursor_thread_id: Some("thread-1".to_string()),
                precursor_kind: Some(ContextPrecursorKind::Handoff),
                actor_id: Some("reviewer@local".to_string()),
                repo_root: Some("/repo".to_string()),
                git_branch: Some("rewrite-codex-2gether-v2".to_string()),
                goal: Some("Verify the simplified /context and /handoff flow.".to_string()),
            }
        );
        assert_eq!(
            response.nodes,
            vec![
                ContextQueryNode::Thread(ContextThreadNode {
                    node_id: local_file_ref_id.clone(),
                    artifact_kind: ThreadArtifactKind::FileChange,
                    title: "tui/src/chatwidget.rs".to_string(),
                    summary: Some("linked file · updated in thread".to_string()),
                    location: Some("tui/src/chatwidget.rs".to_string()),
                    body: Some("Adjusted the /context selection view.".to_string()),
                    origin_thread_id: "thread-2".to_string(),
                    source_files: vec!["tui/src/chatwidget.rs".to_string()],
                    source_refs: Vec::new(),
                    created_at: None,
                }),
                ContextQueryNode::Thread(ContextThreadNode {
                    node_id: seed_ref_id.clone(),
                    artifact_kind: ThreadArtifactKind::Plan,
                    title: "Simplify /context selection flow".to_string(),
                    summary: Some("thread insight · retained plan output".to_string()),
                    location: Some("insight/plan-1".to_string()),
                    body: Some(
                        "Only show one-line nodes and let Enter toggle selection.".to_string()
                    ),
                    origin_thread_id: "thread-1".to_string(),
                    source_files: vec!["tui/src/chatwidget.rs".to_string()],
                    source_refs: Vec::new(),
                    created_at: None,
                }),
                ContextQueryNode::Repo(ContextRepoNode {
                    node_id: repo_ref_id.clone(),
                    repo_kind: RepoMemoryKind::Playbook,
                    title: "Handoff selection flow".to_string(),
                    summary: Some("playbook · one-line context selection".to_string()),
                    path: ".codex/context/playbooks/handoff-selection-flow.md".to_string(),
                    source_threads: vec!["thread-1".to_string()],
                    source_refs: vec![seed_ref_id.clone()],
                    source_files: vec!["tui/src/chatwidget.rs".to_string()],
                    last_validated_at: None,
                }),
            ]
        );
        assert_eq!(
            response.edges,
            vec![
                ContextQueryEdge {
                    from_node_id: "anchor:thread-2".to_string(),
                    to_node_id: local_file_ref_id.clone(),
                    edge_type: ContextEdgeType::Mounted,
                    mount_reason: Some(ContextMountReason::Local),
                    reason: None,
                },
                ContextQueryEdge {
                    from_node_id: "anchor:thread-2".to_string(),
                    to_node_id: seed_ref_id.clone(),
                    edge_type: ContextEdgeType::Mounted,
                    mount_reason: Some(ContextMountReason::HandoffSeed),
                    reason: None,
                },
                ContextQueryEdge {
                    from_node_id: repo_ref_id,
                    to_node_id: local_file_ref_id.clone(),
                    edge_type: ContextEdgeType::Related,
                    mount_reason: None,
                    reason: Some("same_file".to_string()),
                },
                ContextQueryEdge {
                    from_node_id: local_file_ref_id.clone(),
                    to_node_id: "ctx:file:.codex/context/playbooks/handoff-selection-flow.md"
                        .to_string(),
                    edge_type: ContextEdgeType::CoveredBy,
                    mount_reason: None,
                    reason: Some("source_file".to_string()),
                },
                ContextQueryEdge {
                    from_node_id: seed_ref_id.clone(),
                    to_node_id: "ctx:file:.codex/context/playbooks/handoff-selection-flow.md"
                        .to_string(),
                    edge_type: ContextEdgeType::PromotedTo,
                    mount_reason: None,
                    reason: Some("source_ref".to_string()),
                },
                ContextQueryEdge {
                    from_node_id: seed_ref_id,
                    to_node_id: local_file_ref_id,
                    edge_type: ContextEdgeType::Related,
                    mount_reason: None,
                    reason: Some("same_file".to_string()),
                },
            ]
        );
    }

    #[test]
    fn build_context_query_links_graph_queries_to_discovered_nodes() {
        let graph_query_ref_id = "ctx:thread-search:thread-1:query-1".to_string();
        let repo_ref_id = "ctx:file:.codex/context/concepts/context-graph.md".to_string();

        let response = build_context_query(
            vec![
                ContextDocument {
                    ref_id: graph_query_ref_id.clone(),
                    kind: ContextKind::ThreadSearch,
                    title: "Context graph: retained plan output".to_string(),
                    summary: Some("1 global context match for retained plan output".to_string()),
                    location: Some("context/graph".to_string()),
                    body: Some("Results: ctx:file:.codex/context/concepts/context-graph.md".into()),
                    search_text: "context graph retained plan output".to_string(),
                    graph: ContextDocumentGraphMetadata {
                        source_threads: vec!["thread-1".to_string()],
                        source_refs: vec![repo_ref_id.clone()],
                        ..ContextDocumentGraphMetadata::default()
                    },
                },
                ContextDocument {
                    ref_id: repo_ref_id.clone(),
                    kind: ContextKind::RepoContextFile,
                    title: "Context graph".to_string(),
                    summary: Some("concept · Rooted thread and repo knowledge.".to_string()),
                    location: Some(".codex/context/concepts/context-graph.md".to_string()),
                    body: None,
                    search_text: "context graph rooted thread repo knowledge".to_string(),
                    graph: ContextDocumentGraphMetadata::default(),
                },
            ],
            &ContextQueryParams {
                current_thread_id: Some("thread-1".to_string()),
                precursor_thread_id: None,
                precursor_kind: None,
                actor_id: None,
                repo_root: Some("/repo".to_string()),
                git_branch: None,
                goal: None,
                query: None,
                seed_ref_ids: Vec::new(),
                limit: Some(10),
            },
        );

        assert_eq!(
            response
                .nodes
                .into_iter()
                .map(|node| match node {
                    ContextQueryNode::Thread(node) => node.node_id,
                    ContextQueryNode::Repo(node) => node.node_id,
                })
                .collect::<Vec<_>>(),
            vec![graph_query_ref_id.clone(), repo_ref_id.clone()]
        );
        assert!(response.edges.iter().any(|edge| {
            edge.from_node_id == graph_query_ref_id
                && edge.to_node_id == repo_ref_id
                && edge.edge_type == ContextEdgeType::Related
                && edge.reason.as_deref() == Some("query_result")
        }));
    }
}
