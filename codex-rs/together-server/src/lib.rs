use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use anyhow::Result;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::extract::WebSocketUpgrade;
use axum::extract::ws::Message;
use axum::extract::ws::WebSocket;
use axum::response::IntoResponse;
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use codex_app_server_protocol::ClientInfo;
use codex_app_server_protocol::InitializeCapabilities;
use codex_app_server_protocol::InitializeParams;
use codex_app_server_protocol::InitializeResponse;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification as AppJsonRpcNotification;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::SandboxMode as AppServerSandboxMode;
use codex_app_server_protocol::Thread as AppThread;
use codex_app_server_protocol::ThreadListParams as AppThreadListParams;
use codex_app_server_protocol::ThreadListResponse as AppThreadListResponse;
use codex_app_server_protocol::ThreadReadParams as AppThreadReadParams;
use codex_app_server_protocol::ThreadReadResponse as AppThreadReadResponse;
use codex_app_server_protocol::ThreadSortKey as AppThreadSortKey;
use codex_app_server_protocol::ThreadStartParams as AppThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse as AppThreadStartResponse;
use codex_app_server_protocol::ThreadStatus;
use codex_app_server_protocol::build_turns_from_rollout_items;
use codex_context_graph::ContextDocument;
use codex_context_graph::build_context_graph as shared_build_context_graph;
use codex_context_graph::build_context_query as shared_build_context_query;
use codex_context_graph::context_thread_id_from_ref_id as shared_context_thread_id_from_ref_id;
use codex_context_graph::repo_context_documents as shared_repo_context_documents;
use codex_context_graph::search_context_documents as shared_search_context_documents;
use codex_context_graph::thread_context_document as shared_thread_context_document;
use codex_context_graph::thread_context_documents as shared_thread_context_documents;
use codex_core::RolloutRecorder;
use codex_core::config::find_codex_home;
use codex_core::find_thread_path_by_id_str;
use codex_core::git_info::current_branch_name;
use codex_core::git_info::get_git_repo_root;
use codex_core::git_info::get_head_commit_hash;
use codex_protocol::protocol::InitialHistory;
use codex_state::StateRuntime;
use codex_state::TogetherClientMode as StateTogetherClientMode;
use codex_state::TogetherClientSession as StateTogetherClientSession;
use codex_state::TogetherServerRecord;
use codex_together_protocol::ConnectedMember;
use codex_together_protocol::ContextGraphEdge;
use codex_together_protocol::ContextGraphParams;
use codex_together_protocol::ContextGraphResponse;
use codex_together_protocol::ContextKind;
use codex_together_protocol::ContextPreviewParams;
use codex_together_protocol::ContextPreviewResponse;
use codex_together_protocol::ContextQueryParams;
use codex_together_protocol::ContextRef;
use codex_together_protocol::ContextResolveBundleParams;
use codex_together_protocol::ContextResolveBundleResponse;
use codex_together_protocol::ContextSearchParams;
use codex_together_protocol::ContextSearchResponse;
use codex_together_protocol::ContextSearchResult;
use codex_together_protocol::ContextStaleState;
use codex_together_protocol::ContextWriteCommitParams;
use codex_together_protocol::ContextWriteCommitResponse;
use codex_together_protocol::ContextWriteFilePlan;
use codex_together_protocol::ContextWritePlanParams;
use codex_together_protocol::ContextWritePlanResponse;
use codex_together_protocol::HandoffCommitParams;
use codex_together_protocol::HandoffCommitResponse;
use codex_together_protocol::HandoffPlanParams;
use codex_together_protocol::HandoffPlanResponse;
use codex_together_protocol::HostStopResponse;
use codex_together_protocol::JsonRpcNotification;
use codex_together_protocol::JsonRpcRequest;
use codex_together_protocol::JsonRpcResponse;
use codex_together_protocol::METHOD_CONTEXT_GRAPH;
use codex_together_protocol::METHOD_CONTEXT_PREVIEW;
use codex_together_protocol::METHOD_CONTEXT_QUERY;
use codex_together_protocol::METHOD_CONTEXT_RESOLVE_BUNDLE;
use codex_together_protocol::METHOD_CONTEXT_SEARCH;
use codex_together_protocol::METHOD_CONTEXT_WRITE_COMMIT;
use codex_together_protocol::METHOD_CONTEXT_WRITE_PLAN;
use codex_together_protocol::METHOD_HANDOFF_COMMIT;
use codex_together_protocol::METHOD_HANDOFF_PLAN;
use codex_together_protocol::METHOD_HOST_START;
use codex_together_protocol::METHOD_HOST_STATUS;
use codex_together_protocol::METHOD_HOST_STOP;
use codex_together_protocol::METHOD_INITIALIZE;
use codex_together_protocol::METHOD_INITIALIZED;
use codex_together_protocol::METHOD_MEMORY_PROMOTE;
use codex_together_protocol::METHOD_SESSION_JOIN;
use codex_together_protocol::METHOD_SESSION_LEAVE;
use codex_together_protocol::METHOD_THREAD_LIST;
use codex_together_protocol::METHOD_THREAD_READ;
use codex_together_protocol::METHOD_TOGETHER_AUTH;
use codex_together_protocol::MemoryPromoteParams;
use codex_together_protocol::MemoryPromoteResponse;
use codex_together_protocol::NOTIFY_HOST_STOPPED;
use codex_together_protocol::ThreadListParams;
use codex_together_protocol::ThreadListResponse;
use codex_together_protocol::ThreadReadParams;
use codex_together_protocol::ThreadReadResponse;
use codex_together_protocol::ThreadSummary;
use codex_together_protocol::TogetherAuthRequest;
use codex_together_protocol::TogetherAuthResponse;
use codex_together_protocol::TogetherJoinRequest;
use codex_together_protocol::TogetherJoinResponse;
use codex_together_protocol::TogetherLeaveResponse;
use codex_together_protocol::TogetherRole;
use codex_together_protocol::TogetherServerCreateRequest;
use codex_together_protocol::TogetherServerCreateResponse;
use codex_together_protocol::TogetherServerInfoResponse;
use futures::SinkExt;
use futures::StreamExt;
use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::process::Child;
use tokio::process::ChildStdin;
use tokio::process::ChildStdout;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tracing::error;
use tracing::warn;
use url::Url;
use uuid::Uuid;

const RPC_ERR_NOT_CONNECTED: i64 = -39000;
const RPC_ERR_FORBIDDEN: i64 = -39001;
const RPC_ERR_SERVER_CLOSED: i64 = -39003;
const RPC_ERR_SINGLETON_CONFLICT: i64 = -39005;
const RPC_ERR_MEMBER_NOT_ALLOWED: i64 = -39006;
const RPC_ERR_OVERLOADED: i64 = -39007;

const APP_SERVER_MAX_OVERLOAD_RETRIES: usize = 3;
const APP_SERVER_OVERLOAD_BACKOFF_MS: [u64; APP_SERVER_MAX_OVERLOAD_RETRIES] = [100, 300, 900];
const CONTEXT_DEFAULT_LIMIT: u32 = 40;

#[derive(Clone)]
struct AppState {
    inner: Arc<Mutex<ServerState>>,
    app_server: Arc<Mutex<AppServerBridge>>,
    state_db: Arc<StateRuntime>,
    endpoint_url: String,
    _singleton_lock: Arc<SingletonLock>,
}

struct ServerState {
    hosted: Option<HostedServer>,
    handoff_plans: HashMap<String, PendingHandoffPlan>,
    context_write_plans: HashMap<String, PendingContextWritePlan>,
    connections: HashMap<Uuid, ConnectionEntry>,
}

struct ConnectionEntry {
    tx: mpsc::UnboundedSender<String>,
    email: Option<String>,
}

#[derive(Debug, Clone)]
struct PendingHandoffPlan {
    source_thread_id: String,
    promotion_files: Vec<PendingContextWriteFile>,
}

#[derive(Debug, Clone)]
struct PendingContextWritePlan {
    files: Vec<PendingContextWriteFile>,
}

#[derive(Debug, Clone)]
struct PendingContextWriteFile {
    relative_path: String,
    content: String,
}

#[derive(Debug, Clone)]
struct HostedServer {
    server_id: String,
    owner_email: String,
    public_base_url: String,
    members: HashSet<String>,
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

#[derive(Debug, Default)]
struct PersistedContextCoverage {
    source_thread_ids: HashSet<String>,
    source_file_paths: HashSet<String>,
    source_ref_ids: HashSet<String>,
}

#[derive(Debug, Default)]
struct ConnectionContext {
    initialized: bool,
    email: Option<String>,
}

#[derive(Debug, Serialize)]
struct Healthz {
    ok: bool,
    version: &'static str,
    commit: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct InvitePayload {
    endpoint: String,
    server_id: String,
    owner_email: String,
    exp: i64,
}

pub async fn run_main(listen: &str) -> Result<()> {
    let (socket_addr, endpoint_url) = parse_ws_listen_url(listen)?;
    let codex_home = find_codex_home().context("failed to resolve CODEX_HOME")?;

    let singleton_lock = Arc::new(SingletonLock::acquire(codex_home.as_path())?);

    let state_db = StateRuntime::init(codex_home, "together".to_string(), None)
        .await
        .context("failed to initialize state db for together-server")?;

    let app_server = AppServerBridge::spawn_current_binary()
        .await
        .map_err(|err| anyhow::anyhow!("failed to start codex app-server bridge: {err:?}"))?;

    let state = AppState {
        inner: Arc::new(Mutex::new(ServerState {
            hosted: None,
            handoff_plans: HashMap::new(),
            context_write_plans: HashMap::new(),
            connections: HashMap::new(),
        })),
        app_server: Arc::new(Mutex::new(app_server)),
        state_db,
        endpoint_url: endpoint_url.clone(),
        _singleton_lock: singleton_lock,
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/ws", get(ws_handler))
        .with_state(state);

    let listener = TcpListener::bind(socket_addr)
        .await
        .with_context(|| format!("failed to bind together-server at {socket_addr}"))?;

    tracing::info!("codex-together server listening on {endpoint_url}");
    axum::serve(listener, app)
        .await
        .context("axum server failed")?;
    Ok(())
}

async fn healthz() -> Json<Healthz> {
    Json(Healthz {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
        commit: together_build_commit().await,
    })
}

async fn together_build_commit() -> Option<String> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = get_git_repo_root(manifest_dir)?;
    get_head_commit_hash(repo_root.as_path()).await
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let connection_id = Uuid::new_v4();
    let actor_id = default_actor_id(connection_id);
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    {
        let mut guard = state.inner.lock().await;
        guard.connections.insert(
            connection_id,
            ConnectionEntry {
                tx,
                email: Some(actor_id.clone()),
            },
        );
    }

    let send_task = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sender.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let mut ctx = ConnectionContext {
        initialized: false,
        email: Some(actor_id),
    };

    while let Some(Ok(msg)) = receiver.next().await {
        let Message::Text(text) = msg else {
            continue;
        };

        let parsed: Value = match serde_json::from_str(text.as_str()) {
            Ok(v) => v,
            Err(err) => {
                warn!(error = %err, "invalid JSON from together client");
                continue;
            }
        };

        if parsed.get("id").is_some() {
            let req: JsonRpcRequest = match serde_json::from_value(parsed) {
                Ok(req) => req,
                Err(err) => {
                    warn!(error = %err, "invalid JSON-RPC request");
                    continue;
                }
            };

            let response = handle_request(&state, connection_id, &mut ctx, req).await;
            if let Ok(text) = serde_json::to_string(&response) {
                let guard = state.inner.lock().await;
                if let Some(entry) = guard.connections.get(&connection_id) {
                    let _ = entry.tx.send(text);
                }
            }
            continue;
        }

        let note: JsonRpcNotification = match serde_json::from_value(parsed) {
            Ok(note) => note,
            Err(err) => {
                warn!(error = %err, "invalid JSON-RPC notification");
                continue;
            }
        };

        if note.method == METHOD_INITIALIZED {
            ctx.initialized = true;
        }
    }

    {
        let mut guard = state.inner.lock().await;
        guard.connections.remove(&connection_id);
    }

    send_task.abort();
}

async fn handle_request(
    state: &AppState,
    connection_id: Uuid,
    ctx: &mut ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    if req.method != METHOD_INITIALIZE && !ctx.initialized {
        return rpc_error(req.id, -32002, "Not initialized");
    }

    match req.method.as_str() {
        METHOD_INITIALIZE => JsonRpcResponse::ok(
            req.id,
            serde_json::json!({
                "serverInfo": { "name": "codex-together", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": { "experimentalApi": true }
            }),
        )
        .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed")),
        METHOD_TOGETHER_AUTH => together_auth(state, connection_id, ctx, req).await,
        METHOD_HOST_START => together_server_create(state, ctx, req).await,
        METHOD_HOST_STATUS => together_server_info(state, ctx, req).await,
        METHOD_HOST_STOP => host_stop(state, ctx, req).await,
        METHOD_SESSION_JOIN => together_join(state, ctx, req).await,
        METHOD_SESSION_LEAVE => together_leave(state, connection_id, ctx, req).await,
        METHOD_CONTEXT_SEARCH => context_search(state, ctx, req).await,
        METHOD_CONTEXT_GRAPH => context_graph(state, ctx, req).await,
        METHOD_CONTEXT_QUERY => context_query(state, ctx, req).await,
        METHOD_CONTEXT_PREVIEW => context_preview(state, ctx, req).await,
        METHOD_CONTEXT_RESOLVE_BUNDLE => context_resolve_bundle(state, ctx, req).await,
        METHOD_MEMORY_PROMOTE => memory_promote(state, ctx, req).await,
        METHOD_THREAD_READ => thread_read(state, ctx, req).await,
        METHOD_THREAD_LIST => thread_list(state, ctx, req).await,
        METHOD_HANDOFF_PLAN => handoff_plan(state, ctx, req).await,
        METHOD_HANDOFF_COMMIT => handoff_commit(state, req).await,
        METHOD_CONTEXT_WRITE_PLAN => context_write_plan(state, ctx, req).await,
        METHOD_CONTEXT_WRITE_COMMIT => context_write_commit(state, req).await,
        _ => rpc_error(req.id, -32601, "method not found"),
    }
}

async fn together_auth(
    state: &AppState,
    connection_id: Uuid,
    ctx: &mut ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: TogetherAuthRequest = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    let canonical_email = non_empty_string(payload.email)
        .or_else(|| ctx.email.clone())
        .unwrap_or_else(|| default_actor_id(connection_id));

    ctx.email = Some(canonical_email.clone());
    set_connection_email(state, connection_id, Some(canonical_email.clone())).await;

    let guard = state.inner.lock().await;
    let (server_id, owner_email, role) = if let Some(hosted) = &guard.hosted {
        let role = if canonical_email == hosted.owner_email {
            TogetherRole::Owner
        } else {
            TogetherRole::Member
        };
        (hosted.server_id.clone(), hosted.owner_email.clone(), role)
    } else {
        (String::new(), canonical_email.clone(), TogetherRole::Owner)
    };

    JsonRpcResponse::ok(
        req.id,
        TogetherAuthResponse {
            role,
            server_id,
            owner_email,
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn together_server_create(
    state: &AppState,
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: TogetherServerCreateRequest = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };
    let owner_email = ctx
        .email
        .clone()
        .unwrap_or_else(|| "host@local".to_string());

    let now = Utc::now();
    let created_at_epoch = now.timestamp();
    let public_base_url = payload.public_base_url.trim_end_matches('/').to_string();
    let server_id = format!("srv_{}", Uuid::new_v4().simple());
    let invite_token = encode_invite(InvitePayload {
        endpoint: public_base_url.clone(),
        server_id: server_id.clone(),
        owner_email: owner_email.clone(),
        exp: now.timestamp() + 60 * 60 * 24 * 30,
    });
    let invite_link = format!("codex://together/{invite_token}");

    {
        let mut guard = state.inner.lock().await;
        if guard.hosted.is_some() {
            return rpc_error(
                req.id,
                RPC_ERR_SINGLETON_CONFLICT,
                "TOGETHER_SINGLETON_CONFLICT",
            );
        }

        let hosted = HostedServer {
            server_id: server_id.clone(),
            owner_email: owner_email.clone(),
            public_base_url: public_base_url.clone(),
            members: HashSet::from([owner_email.clone()]),
        };
        guard.hosted = Some(hosted);
    }

    if let Err(err) = state
        .state_db
        .upsert_together_server(&TogetherServerRecord {
            server_id: server_id.clone(),
            owner_email: owner_email.clone(),
            public_base_url: public_base_url.clone(),
            invite_token: invite_token.clone(),
            created_at: created_at_epoch,
            closed_at: None,
        })
        .await
    {
        error!(error = %err, "failed to persist together server");
        return rpc_error(req.id, -32603, "failed to persist together server");
    }

    if let Err(err) = state
        .state_db
        .upsert_together_client_session(&StateTogetherClientSession {
            mode: StateTogetherClientMode::Host,
            server_id: Some(server_id.clone()),
            owner_email: Some(owner_email.clone()),
            endpoint: Some(public_base_url.clone()),
            created_at: created_at_epoch,
            updated_at: created_at_epoch,
        })
        .await
    {
        error!(error = %err, "failed to persist together client host session");
        return rpc_error(req.id, -32603, "failed to persist together server");
    }

    JsonRpcResponse::ok(
        req.id,
        TogetherServerCreateResponse {
            server_id,
            owner_email,
            invite_token,
            invite_link,
            local_ws_url: state.endpoint_url.clone(),
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn host_stop(
    state: &AppState,
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let email = ctx
        .email
        .clone()
        .unwrap_or_else(|| "guest@local".to_string());

    let hosted = {
        let mut guard = state.inner.lock().await;
        let Some(hosted) = guard.hosted.clone() else {
            return rpc_error(req.id, RPC_ERR_NOT_CONNECTED, "TOGETHER_NOT_CONNECTED");
        };
        if hosted.owner_email != email {
            return rpc_error(req.id, RPC_ERR_FORBIDDEN, "TOGETHER_FORBIDDEN");
        }

        guard.hosted = None;
        broadcast_notification(
            &guard,
            NOTIFY_HOST_STOPPED,
            serde_json::json!({
                "serverId": hosted.server_id,
                "ownerEmail": hosted.owner_email,
            }),
        );

        hosted
    };

    let now = Utc::now().timestamp();
    if let Err(err) = state
        .state_db
        .close_together_server(&hosted.server_id, now)
        .await
    {
        error!(error = %err, "failed to mark together server closed");
        return rpc_error(req.id, -32603, "failed to persist together server close");
    }

    if let Err(err) = state
        .state_db
        .upsert_together_client_session(&StateTogetherClientSession {
            mode: StateTogetherClientMode::Disconnected,
            server_id: None,
            owner_email: None,
            endpoint: None,
            created_at: now,
            updated_at: now,
        })
        .await
    {
        error!(error = %err, "failed to persist together disconnected session");
        return rpc_error(req.id, -32603, "failed to persist together server close");
    }

    JsonRpcResponse::ok(req.id, HostStopResponse { stopped: true })
        .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn together_server_info(
    state: &AppState,
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let email = ctx
        .email
        .clone()
        .unwrap_or_else(|| "guest@local".to_string());

    let guard = state.inner.lock().await;
    let Some(hosted) = guard.hosted.as_ref() else {
        return rpc_error(req.id, RPC_ERR_NOT_CONNECTED, "TOGETHER_NOT_CONNECTED");
    };

    let role = match member_role(hosted, &email) {
        Some(role) => role,
        None => {
            return rpc_error(
                req.id,
                RPC_ERR_MEMBER_NOT_ALLOWED,
                "TOGETHER_MEMBER_NOT_ALLOWED",
            );
        }
    };

    let mut connected_members = Vec::with_capacity(hosted.members.len());
    for member in &hosted.members {
        connected_members.push(ConnectedMember {
            email: member.clone(),
            role: if member == &hosted.owner_email {
                TogetherRole::Owner
            } else {
                TogetherRole::Member
            },
        });
    }
    connected_members.sort_by(|a, b| a.email.cmp(&b.email));
    let commit = together_build_commit().await;

    JsonRpcResponse::ok(
        req.id,
        TogetherServerInfoResponse {
            server_id: hosted.server_id.clone(),
            owner_email: hosted.owner_email.clone(),
            public_base_url: hosted.public_base_url.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit,
            role,
            connected_members,
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn context_search(
    state: &AppState,
    _ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: ContextSearchParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    JsonRpcResponse::ok(
        req.id,
        ContextSearchResponse {
            data: context_search_results(
                state,
                payload.query.as_deref(),
                payload.limit.unwrap_or(CONTEXT_DEFAULT_LIMIT),
                payload.current_thread_id.as_deref(),
            )
            .await,
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn context_graph(
    state: &AppState,
    _ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: ContextGraphParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    JsonRpcResponse::ok(
        req.id,
        build_context_graph(
            context_documents(
                state,
                context_focus_thread_ids(payload.current_thread_id.as_deref()),
            )
            .await,
            payload.query.as_deref(),
            payload.limit.unwrap_or(CONTEXT_DEFAULT_LIMIT),
            payload.current_thread_id.as_deref(),
        ),
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn context_query(
    state: &AppState,
    _ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: ContextQueryParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    let documents = context_documents(state, context_focus_thread_ids_for_query(&payload)).await;
    JsonRpcResponse::ok(req.id, shared_build_context_query(documents, &payload))
        .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn context_preview(
    state: &AppState,
    _ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: ContextPreviewParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    let item = context_documents(state, context_focus_thread_ids_for_ref_id(&payload.ref_id))
        .await
        .into_iter()
        .find(|document| document.ref_id == payload.ref_id)
        .map(ContextDocument::into_search_result);

    JsonRpcResponse::ok(req.id, ContextPreviewResponse { item })
        .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn context_resolve_bundle(
    state: &AppState,
    _ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: ContextResolveBundleParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    let response = build_context_bundle(state, payload.thread_id, payload.context_refs).await;
    JsonRpcResponse::ok(req.id, response)
        .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn memory_promote(
    state: &AppState,
    _ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: MemoryPromoteParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };
    if payload.selected_node_ids.is_empty() {
        return rpc_error(req.id, -32602, "selectedNodeIds is required");
    }

    let repo_root = match resolve_context_root() {
        Ok(path) => path,
        Err(err) => {
            return rpc_error(
                req.id,
                -32603,
                format!("failed to resolve collaboration context root: {err}"),
            );
        }
    };

    let documents = context_documents(
        state,
        context_focus_thread_ids_for_memory_promote(
            payload.current_thread_id.as_deref(),
            &payload.selected_node_ids,
        ),
    )
    .await;
    let plan = plan_memory_promote(&documents, &payload.selected_node_ids);
    if plan.promotable_node_ids.is_empty() && plan.already_covered_node_ids.is_empty() {
        return rpc_error(req.id, -32602, "no thread nodes matched selectedNodeIds");
    }

    let branch = current_branch_name(repo_root.as_path())
        .await
        .unwrap_or_default();
    let files = plan_context_write_files(
        repo_root.as_path(),
        documents,
        &plan.promotable_node_ids,
        non_empty_string(branch),
    )
    .into_iter()
    .map(|file| PendingContextWriteFile {
        relative_path: file.relative_path,
        content: file.content,
    })
    .collect::<Vec<_>>();
    let created = match write_pending_context_files(repo_root.as_path(), files) {
        Ok(written_files) => written_files
            .into_iter()
            .map(|path| format!("ctx:file:{path}"))
            .collect(),
        Err(err) => return rpc_error(req.id, -32603, err.to_string()),
    };

    JsonRpcResponse::ok(
        req.id,
        MemoryPromoteResponse {
            created,
            already_covered: plan.already_covered_node_ids,
            proposal_required: Vec::new(),
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn thread_read(
    state: &AppState,
    _ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: ThreadReadParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    let thread = {
        let mut bridge = state.app_server.lock().await;
        match bridge.thread_read(payload.thread_id, false).await {
            Ok(response) => response.thread,
            Err(err) => return app_server_error_response(req.id, err),
        }
    };

    JsonRpcResponse::ok(
        req.id,
        ThreadReadResponse {
            thread: thread_summary_from_app_thread(thread),
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn thread_list(
    state: &AppState,
    _ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: ThreadListParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    let response = {
        let mut bridge = state.app_server.lock().await;
        match bridge
            .thread_list(payload.query.clone(), payload.cursor.clone(), payload.limit)
            .await
        {
            Ok(response) => response,
            Err(err) => return app_server_error_response(req.id, err),
        }
    };

    let repo_root_filter = payload.repo_root.as_deref().map(Path::new);
    let data = response
        .data
        .into_iter()
        .filter_map(|thread| {
            let summary = thread_summary_from_app_thread(thread);
            if repo_root_filter.is_some_and(|expected_repo_root| {
                summary
                    .repo_root
                    .as_deref()
                    .map(Path::new)
                    .filter(|actual_repo_root| *actual_repo_root == expected_repo_root)
                    .is_none()
            }) {
                return None;
            }
            Some(summary)
        })
        .collect();

    JsonRpcResponse::ok(
        req.id,
        ThreadListResponse {
            data,
            next_cursor: response.next_cursor,
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn context_write_plan(
    state: &AppState,
    _ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: ContextWritePlanParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };
    if payload.selected_ref_ids.is_empty() {
        return rpc_error(req.id, -32602, "selectedRefIds is required");
    }

    let repo_root = match resolve_context_root() {
        Ok(path) => path,
        Err(err) => {
            return rpc_error(
                req.id,
                -32603,
                format!("failed to resolve collaboration context root: {err}"),
            );
        }
    };
    let branch = match payload.branch {
        Some(branch) => non_empty_string(branch).unwrap_or_default(),
        None => current_branch_name(repo_root.as_path())
            .await
            .unwrap_or_default(),
    };

    let documents = context_documents(
        state,
        context_focus_thread_ids_for_ref_ids(&payload.selected_ref_ids),
    )
    .await;
    let planned_files = plan_context_write_files(
        repo_root.as_path(),
        documents,
        &payload.selected_ref_ids,
        non_empty_string(branch),
    );
    if planned_files.is_empty() {
        return rpc_error(req.id, -32602, "no context refs matched selectedRefIds");
    }

    let plan_id = Uuid::new_v4().to_string();
    let files = planned_files
        .iter()
        .map(|file| ContextWriteFilePlan {
            path: file.relative_path.clone(),
            title: file.title.clone(),
            kind: file.kind.clone(),
            exists: file.exists,
            content: file.content.clone(),
        })
        .collect::<Vec<_>>();

    {
        let mut guard = state.inner.lock().await;
        guard.context_write_plans.insert(
            plan_id.clone(),
            PendingContextWritePlan {
                files: planned_files
                    .into_iter()
                    .map(|file| PendingContextWriteFile {
                        relative_path: file.relative_path,
                        content: file.content,
                    })
                    .collect(),
            },
        );
    }

    JsonRpcResponse::ok(req.id, ContextWritePlanResponse { plan_id, files })
        .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn context_write_commit(state: &AppState, req: JsonRpcRequest) -> JsonRpcResponse {
    let payload: ContextWriteCommitParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    let pending = {
        let mut guard = state.inner.lock().await;
        match guard.context_write_plans.remove(&payload.plan_id) {
            Some(plan) => plan,
            None => return rpc_error(req.id, -32602, "unknown context write plan"),
        }
    };

    let repo_root = match resolve_context_root() {
        Ok(path) => path,
        Err(err) => {
            return rpc_error(
                req.id,
                -32603,
                format!("failed to resolve collaboration context root: {err}"),
            );
        }
    };

    let written_files = match write_pending_context_files(repo_root.as_path(), pending.files) {
        Ok(files) => files,
        Err(err) => return rpc_error(req.id, -32603, err.to_string()),
    };

    JsonRpcResponse::ok(req.id, ContextWriteCommitResponse { written_files })
        .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

fn write_pending_context_files(
    repo_root: &Path,
    files: Vec<PendingContextWriteFile>,
) -> Result<Vec<String>> {
    let mut written_files = Vec::with_capacity(files.len());
    for file in files {
        let path = repo_root.join(&file.relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        std::fs::write(&path, file.content)
            .with_context(|| format!("failed to write {}", path.display()))?;
        written_files.push(file.relative_path);
    }
    Ok(written_files)
}

async fn handoff_plan(
    state: &AppState,
    _ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: HandoffPlanParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    let Some(source_thread_id) = payload.source_thread_id.clone() else {
        return rpc_error(req.id, -32602, "sourceThreadId is required");
    };

    let documents = context_documents(
        state,
        context_focus_thread_ids_for_handoff(
            payload.source_thread_id.as_deref(),
            &payload.selected_ref_ids,
        ),
    )
    .await;
    let source_entry = {
        let mut bridge = state.app_server.lock().await;
        match source_thread_context_entry(&mut bridge, &source_thread_id).await {
            Ok(entry) => entry,
            Err(err) if app_server_thread_not_loaded(&err) => {
                let expected_ref_id = format!("ctx:thread:{source_thread_id}");
                match documents
                    .iter()
                    .find(|document| document.ref_id == expected_ref_id)
                {
                    Some(document) => resolved_entry_from_document(document.clone()),
                    None => return app_server_error_response(req.id, err),
                }
            }
            Err(err) => return app_server_error_response(req.id, err),
        }
    };

    let selected_ref_ids =
        recommended_handoff_ref_ids(&documents, &source_thread_id, &payload.selected_ref_ids);
    let mut kept_entries = vec![source_entry];
    kept_entries.extend(selected_context_entries(&documents, &selected_ref_ids));
    dedupe_context_entries(&mut kept_entries);

    let kept_refs = kept_entries
        .iter()
        .map(|entry| entry.context_ref.clone())
        .collect::<Vec<_>>();
    let token_estimate = estimate_context_bundle_tokens(&documents, &kept_entries);
    let plan_id = if !payload.preview_only {
        Uuid::new_v4().to_string()
    } else {
        Default::default()
    };
    let goal = payload.goal.filter(|value| !value.trim().is_empty());

    if !payload.preview_only {
        let promotion_files = plan_handoff_promotion_files(&documents, &selected_ref_ids).await;
        let mut guard = state.inner.lock().await;
        guard.handoff_plans.insert(
            plan_id.clone(),
            PendingHandoffPlan {
                source_thread_id: source_thread_id.clone(),
                promotion_files,
            },
        );
    }

    JsonRpcResponse::ok(
        req.id,
        HandoffPlanResponse {
            plan_id,
            source_thread_id,
            goal,
            selected_node_ids: selected_ref_ids,
            kept_refs,
            dropped_refs: Vec::new(),
            token_estimate,
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn handoff_commit(state: &AppState, req: JsonRpcRequest) -> JsonRpcResponse {
    let payload: HandoffCommitParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    let pending = {
        let guard = state.inner.lock().await;
        match guard.handoff_plans.get(&payload.plan_id) {
            Some(plan) => plan.clone(),
            None => return rpc_error(req.id, -32602, "unknown handoff plan"),
        }
    };

    let mut bridge = state.app_server.lock().await;
    let cwd = match payload.cwd {
        Some(cwd) => cwd,
        None => match source_thread_cwd(&mut bridge, &pending.source_thread_id).await {
            Ok(cwd) => cwd,
            Err(err) => return app_server_error_response(req.id, err),
        },
    };
    let repo_root = get_git_repo_root(Path::new(&cwd)).unwrap_or_else(|| PathBuf::from(&cwd));
    if let Err(err) =
        write_pending_context_files(repo_root.as_path(), pending.promotion_files.clone())
        && !pending.promotion_files.is_empty()
    {
        warn!(
            error = %err,
            source_thread_id = %pending.source_thread_id,
            "failed to write handoff promotion files"
        );
    }

    let started = match bridge
        .thread_start(
            Some(cwd.clone()),
            payload.model,
            payload.approval_policy,
            payload.sandbox,
        )
        .await
    {
        Ok(response) => response,
        Err(err) => return app_server_error_response(req.id, err),
    };

    {
        let mut guard = state.inner.lock().await;
        guard.handoff_plans.remove(&payload.plan_id);
    }

    JsonRpcResponse::ok(
        req.id,
        HandoffCommitResponse {
            thread_id: started.thread.id.clone(),
            source_thread_id: pending.source_thread_id,
            rollout_path: started
                .thread
                .path
                .as_ref()
                .map(|path| path.display().to_string()),
            cwd: started.cwd.display().to_string(),
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn together_join(
    state: &AppState,
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: TogetherJoinRequest = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    let email = ctx
        .email
        .clone()
        .unwrap_or_else(|| "guest@local".to_string());
    let server_hint = invite_server_hint(&payload.invite);
    let (server_id, owner_email, endpoint, role) = {
        let mut guard = state.inner.lock().await;
        let Some(hosted) = guard.hosted.as_mut() else {
            return rpc_error(req.id, RPC_ERR_NOT_CONNECTED, "TOGETHER_NOT_CONNECTED");
        };

        if let Some(hint) = server_hint.as_deref()
            && hosted.server_id != hint
            && !hosted.server_id.starts_with(hint)
        {
            return rpc_error(req.id, RPC_ERR_SERVER_CLOSED, "TOGETHER_SERVER_CLOSED");
        }

        let role = if email == hosted.owner_email {
            TogetherRole::Owner
        } else {
            TogetherRole::Member
        };
        if matches!(role, TogetherRole::Member) {
            hosted.members.insert(email.clone());
        }

        (
            hosted.server_id.clone(),
            hosted.owner_email.clone(),
            hosted.public_base_url.clone(),
            role,
        )
    };

    JsonRpcResponse::ok(
        req.id,
        TogetherJoinResponse {
            server_id,
            owner_email,
            endpoint,
            role,
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn together_leave(
    state: &AppState,
    connection_id: Uuid,
    ctx: &mut ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let Some(leaving_email) = ctx.email.clone() else {
        return rpc_error(req.id, RPC_ERR_NOT_CONNECTED, "TOGETHER_NOT_CONNECTED");
    };

    {
        let mut guard = state.inner.lock().await;
        let Some(hosted) = guard.hosted.as_mut() else {
            return rpc_error(req.id, RPC_ERR_NOT_CONNECTED, "TOGETHER_NOT_CONNECTED");
        };

        if leaving_email == hosted.owner_email {
            return rpc_error(req.id, RPC_ERR_FORBIDDEN, "TOGETHER_FORBIDDEN");
        }
        if !hosted.members.remove(&leaving_email) {
            return rpc_error(
                req.id,
                RPC_ERR_MEMBER_NOT_ALLOWED,
                "TOGETHER_MEMBER_NOT_ALLOWED",
            );
        }
    };

    ctx.email = None;
    set_connection_email(state, connection_id, None).await;

    JsonRpcResponse::ok(req.id, TogetherLeaveResponse { left: true })
        .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

fn member_role(hosted: &HostedServer, email: &str) -> Option<TogetherRole> {
    if email == hosted.owner_email {
        return Some(TogetherRole::Owner);
    }
    if hosted.members.contains(email) {
        return Some(TogetherRole::Member);
    }
    None
}

fn default_actor_id(connection_id: Uuid) -> String {
    let short = connection_id.simple().to_string();
    format!("anon+{}@local", &short[..12])
}

async fn set_connection_email(state: &AppState, connection_id: Uuid, email: Option<String>) {
    let mut guard = state.inner.lock().await;
    if let Some(entry) = guard.connections.get_mut(&connection_id) {
        entry.email = email;
    }
}

fn parse_ws_listen_url(listen: &str) -> Result<(SocketAddr, String)> {
    let parsed = Url::parse(listen).with_context(|| format!("invalid listen URL: {listen}"))?;
    if parsed.scheme() != "ws" {
        anyhow::bail!("together-server requires ws:// listen URL");
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("listen URL missing host"))?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| anyhow::anyhow!("listen URL missing port"))?;
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .with_context(|| format!("invalid socket address from URL: {listen}"))?;
    Ok((addr, format!("ws://{host}:{port}/ws")))
}

fn encode_invite(invite: InvitePayload) -> String {
    let bytes = serde_json::to_vec(&invite).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(bytes)
}

fn decode_invite(token: &str) -> Result<InvitePayload> {
    let bytes = URL_SAFE_NO_PAD
        .decode(token.as_bytes())
        .context("failed to decode invite token")?;
    serde_json::from_slice(&bytes).context("failed to parse invite payload")
}

fn extract_token(invite: &str) -> String {
    if let Some(stripped) = invite.strip_prefix("codex://together/") {
        return stripped.to_string();
    }
    if let Some(index) = invite.find("/together/invite/") {
        return invite[(index + "/together/invite/".len())..].to_string();
    }
    invite.to_string()
}

fn invite_server_hint(invite: &str) -> Option<String> {
    let token = extract_token(invite);
    if let Ok(payload) = decode_invite(&token) {
        return Some(payload.server_id);
    }

    let trimmed = invite.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("srv_")
        || (trimmed.len() <= 16
            && trimmed
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
    {
        return Some(trimmed.to_string());
    }

    None
}

fn broadcast_notification(state: &ServerState, method: &str, params: Value) {
    let note = JsonRpcNotification {
        jsonrpc: "2.0".to_string(),
        method: method.to_string(),
        params,
    };

    if let Ok(text) = serde_json::to_string(&note) {
        for entry in state.connections.values() {
            let _ = entry.tx.send(text.clone());
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedContextEntry {
    context_ref: ContextRef,
    document: ContextDocument,
}

#[derive(Debug, Clone)]
struct PlannedContextWriteFile {
    relative_path: String,
    title: String,
    kind: String,
    exists: bool,
    content: String,
}

async fn context_search_results(
    state: &AppState,
    query: Option<&str>,
    limit: u32,
    current_thread_id: Option<&str>,
) -> Vec<ContextSearchResult> {
    let documents = context_documents(state, context_focus_thread_ids(current_thread_id)).await;
    shared_search_context_documents(documents, query, limit, current_thread_id)
}

fn build_context_graph(
    documents: Vec<ContextDocument>,
    query: Option<&str>,
    limit: u32,
    current_thread_id: Option<&str>,
) -> ContextGraphResponse {
    shared_build_context_graph(documents, query, limit, current_thread_id)
}

async fn context_documents(
    state: &AppState,
    focus_thread_ids: Vec<String>,
) -> Vec<ContextDocument> {
    let repo_root = match resolve_context_root() {
        Ok(path) => path,
        Err(err) => {
            warn!(error = %err, "failed to resolve collaboration context root");
            return current_thread_context_documents(state, focus_thread_ids, None).await;
        }
    };

    let mut documents = shared_repo_context_documents(repo_root.as_path());
    documents.extend(
        current_thread_context_documents(state, focus_thread_ids, Some(repo_root.as_path())).await,
    );
    documents.sort_by_key(context_default_sort_key);
    documents
}

fn resolve_context_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("failed to resolve current working directory")?;
    Ok(get_git_repo_root(&cwd).unwrap_or(cwd))
}

fn context_focus_thread_ids(current_thread_id: Option<&str>) -> Vec<String> {
    current_thread_id
        .map(str::trim)
        .filter(|thread_id| !thread_id.is_empty())
        .map(str::to_string)
        .into_iter()
        .collect()
}

fn context_focus_thread_ids_for_ref_id(ref_id: &str) -> Vec<String> {
    shared_context_thread_id_from_ref_id(ref_id)
        .into_iter()
        .collect()
}

fn context_focus_thread_ids_for_ref_ids(ref_ids: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    ref_ids
        .iter()
        .filter_map(|ref_id| shared_context_thread_id_from_ref_id(ref_id))
        .filter(|thread_id| seen.insert(thread_id.clone()))
        .collect()
}

fn context_focus_thread_ids_for_handoff(
    source_thread_id: Option<&str>,
    selected_ref_ids: &[String],
) -> Vec<String> {
    let mut thread_ids = context_focus_thread_ids_for_ref_ids(selected_ref_ids);
    if let Some(source_thread_id) = source_thread_id
        && !thread_ids
            .iter()
            .any(|thread_id| thread_id == source_thread_id)
    {
        thread_ids.insert(0, source_thread_id.to_string());
    }
    thread_ids
}

fn context_focus_thread_ids_for_query(params: &ContextQueryParams) -> Vec<String> {
    let mut thread_ids = context_focus_thread_ids(params.current_thread_id.as_deref());
    if let Some(precursor_thread_id) = params
        .precursor_thread_id
        .as_deref()
        .filter(|thread_id| !thread_id.is_empty())
        && !thread_ids
            .iter()
            .any(|thread_id| thread_id == precursor_thread_id)
    {
        thread_ids.push(precursor_thread_id.to_string());
    }
    for thread_id in context_focus_thread_ids_for_ref_ids(&params.seed_ref_ids) {
        if !thread_ids.iter().any(|existing| existing == &thread_id) {
            thread_ids.push(thread_id);
        }
    }
    thread_ids
}

fn context_focus_thread_ids_for_memory_promote(
    current_thread_id: Option<&str>,
    selected_node_ids: &[String],
) -> Vec<String> {
    let mut thread_ids = context_focus_thread_ids(current_thread_id);
    for thread_id in context_focus_thread_ids_for_ref_ids(selected_node_ids) {
        if !thread_ids.iter().any(|existing| existing == &thread_id) {
            thread_ids.push(thread_id);
        }
    }
    thread_ids
}

fn context_thread_id_from_ref_id(ref_id: &str) -> Option<String> {
    shared_context_thread_id_from_ref_id(ref_id)
}

#[cfg(test)]
fn search_context_documents(
    documents: Vec<ContextDocument>,
    query: Option<&str>,
    limit: u32,
    current_thread_id: Option<&str>,
) -> Vec<ContextSearchResult> {
    shared_search_context_documents(documents, query, limit, current_thread_id)
}

fn context_neighbor_ref_ids(
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

fn context_graph_edges(documents: &[ContextDocument]) -> Vec<ContextGraphEdge> {
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

#[cfg(test)]
fn repo_context_documents(repo_root: &Path) -> Vec<ContextDocument> {
    shared_repo_context_documents(repo_root)
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

async fn current_thread_context_documents(
    state: &AppState,
    focus_thread_ids: Vec<String>,
    repo_root: Option<&Path>,
) -> Vec<ContextDocument> {
    if focus_thread_ids.is_empty() {
        return Vec::new();
    }

    let current_thread_id = focus_thread_ids.first().cloned();
    let mut documents = Vec::new();
    let mut seen = HashSet::new();
    let mut bridge = state.app_server.lock().await;
    for thread_id in focus_thread_ids {
        if !seen.insert(thread_id.clone()) {
            continue;
        }

        let prefer_rollout = current_thread_id.as_deref() == Some(thread_id.as_str());
        let thread = if prefer_rollout {
            match load_thread_from_rollout(&thread_id, repo_root).await {
                Ok(Some(thread)) => thread,
                Ok(None) => match thread_read_with_turn_fallback(&mut bridge, &thread_id).await {
                    Ok(response) => response.thread,
                    Err(err) => {
                        warn!(
                            error = ?err,
                            thread_id,
                            "failed to read focused thread for context"
                        );
                        continue;
                    }
                },
                Err(err) => {
                    warn!(
                        error = ?err,
                        thread_id,
                        "failed to load focused thread rollout for context; falling back to app-server bridge"
                    );
                    match thread_read_with_turn_fallback(&mut bridge, &thread_id).await {
                        Ok(response) => response.thread,
                        Err(bridge_err) => {
                            warn!(
                                error = ?bridge_err,
                                thread_id,
                                "failed to read focused thread for context after rollout fallback"
                            );
                            continue;
                        }
                    }
                }
            }
        } else {
            match thread_read_with_turn_fallback(&mut bridge, &thread_id).await {
                Ok(response) => response.thread,
                Err(err) => match load_thread_from_rollout(&thread_id, repo_root).await {
                    Ok(Some(thread)) => {
                        warn!(
                            error = ?err,
                            thread_id,
                            "app-server bridge missed focused thread; using rollout fallback"
                        );
                        thread
                    }
                    Ok(None) => {
                        warn!(error = ?err, thread_id, "failed to read focused thread for context");
                        continue;
                    }
                    Err(fallback_err) => {
                        warn!(
                            error = ?err,
                            fallback_error = ?fallback_err,
                            thread_id,
                            "failed to read focused thread for context"
                        );
                        continue;
                    }
                },
            }
        };
        if thread.ephemeral || repo_root.is_some_and(|root| !thread.cwd.starts_with(root)) {
            continue;
        }

        let is_current_thread = current_thread_id.as_deref() == Some(thread.id.as_str());
        documents.extend(shared_thread_context_documents(
            &thread,
            is_current_thread,
            repo_root,
        ));
    }

    documents.sort_by_key(context_default_sort_key);
    documents
}

async fn load_thread_from_rollout(
    thread_id: &str,
    repo_root: Option<&Path>,
) -> Result<Option<AppThread>> {
    let codex_home = find_codex_home().context("failed to resolve CODEX_HOME")?;
    load_thread_from_rollout_at(codex_home.as_path(), thread_id, repo_root).await
}

async fn load_thread_from_rollout_at(
    codex_home: &Path,
    thread_id: &str,
    repo_root: Option<&Path>,
) -> Result<Option<AppThread>> {
    let Some(rollout_path) = find_thread_path_by_id_str(codex_home, thread_id).await? else {
        return Ok(None);
    };
    let items = match RolloutRecorder::get_rollout_history(rollout_path.as_path())
        .await
        .with_context(|| format!("failed to load rollout `{}`", rollout_path.display()))?
    {
        InitialHistory::New => Vec::new(),
        InitialHistory::Resumed(history) => history.history,
        InitialHistory::Forked(history) => history,
    };
    let turns = build_turns_from_rollout_items(&items);
    let updated_at = std::fs::metadata(&rollout_path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_else(|| Utc::now().timestamp());
    let cwd = repo_root
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let git_info =
        current_branch_name(cwd.as_path())
            .await
            .map(|branch| codex_app_server_protocol::GitInfo {
                sha: None,
                branch: Some(branch),
                origin_url: None,
            });

    Ok(Some(AppThread {
        id: thread_id.to_string(),
        preview: String::new(),
        ephemeral: false,
        model_provider: "openai".to_string(),
        created_at: updated_at,
        updated_at,
        status: ThreadStatus::Idle,
        path: Some(rollout_path),
        cwd,
        cli_version: env!("CARGO_PKG_VERSION").to_string(),
        source: codex_app_server_protocol::SessionSource::Cli,
        agent_nickname: None,
        agent_role: None,
        git_info,
        name: None,
        turns,
    }))
}

async fn build_context_bundle(
    state: &AppState,
    thread_id: Option<String>,
    context_refs: Vec<ContextRef>,
) -> ContextResolveBundleResponse {
    if context_refs.is_empty() {
        return ContextResolveBundleResponse {
            bundle_text: String::new(),
            kept_refs: Vec::new(),
            dropped_refs: Vec::new(),
        };
    }

    let mut focus_thread_ids = context_refs
        .iter()
        .filter_map(|context_ref| {
            context_ref
                .source_thread_id
                .clone()
                .or_else(|| context_thread_id_from_ref_id(&context_ref.ref_id))
        })
        .collect::<Vec<_>>();
    if let Some(thread_id) = thread_id
        && !focus_thread_ids
            .iter()
            .any(|existing| existing == &thread_id)
    {
        focus_thread_ids.insert(0, thread_id);
    }

    let documents = context_documents(state, focus_thread_ids).await;
    let mut kept_entries = Vec::new();
    let mut dropped_refs = Vec::new();
    for context_ref in context_refs {
        if let Some(entry) = resolved_entry_for_ref(&documents, &context_ref) {
            kept_entries.push(entry);
        } else {
            dropped_refs.push(ContextRef {
                stale_state: Some(ContextStaleState::Unavailable),
                ..context_ref
            });
        }
    }
    dedupe_context_entries(&mut kept_entries);
    let kept_refs = kept_entries
        .iter()
        .map(|entry| entry.context_ref.clone())
        .collect::<Vec<_>>();

    let artifact_paths = resolve_context_root().ok().and_then(|repo_root| {
        match sync_context_graph_artifacts(repo_root.as_path(), &documents, &kept_entries) {
            Ok(paths) => Some(paths),
            Err(err) => {
                warn!(error = %err, "failed to sync context graph artifacts");
                None
            }
        }
    });

    ContextResolveBundleResponse {
        bundle_text: render_context_bundle(&documents, &kept_entries, artifact_paths.as_ref()),
        kept_refs,
        dropped_refs,
    }
}

fn selected_context_entries(
    documents: &[ContextDocument],
    selected_ref_ids: &[String],
) -> Vec<ResolvedContextEntry> {
    if selected_ref_ids.is_empty() {
        return Vec::new();
    }

    let selected = selected_ref_ids.iter().cloned().collect::<HashSet<_>>();
    documents
        .iter()
        .filter(|document| selected.contains(&document.ref_id))
        .cloned()
        .map(resolved_entry_from_document)
        .collect()
}

fn resolved_entry_for_ref(
    documents: &[ContextDocument],
    context_ref: &ContextRef,
) -> Option<ResolvedContextEntry> {
    documents
        .iter()
        .find(|document| document.ref_id == context_ref.ref_id)
        .cloned()
        .map(resolved_entry_from_document)
}

fn dedupe_context_entries(entries: &mut Vec<ResolvedContextEntry>) {
    let mut seen = HashSet::new();
    entries.retain(|entry| seen.insert(entry.context_ref.ref_id.clone()));
}

#[derive(Debug, Clone)]
struct ContextGraphArtifactPaths {
    index_relative_path: String,
    node_relative_path_by_ref_id: HashMap<String, String>,
}

fn estimate_context_bundle_tokens(
    documents: &[ContextDocument],
    entries: &[ResolvedContextEntry],
) -> u32 {
    let chars = render_context_bundle(documents, entries, None)
        .chars()
        .count();
    ((chars / 4).max(1)).try_into().unwrap_or(u32::MAX)
}

fn resolved_entry_from_document(document: ContextDocument) -> ResolvedContextEntry {
    let context_ref = context_ref_from_document(&document);
    ResolvedContextEntry {
        context_ref,
        document,
    }
}

fn context_ref_from_document(document: &ContextDocument) -> ContextRef {
    let (source_thread_id, repo_context_id) = match document.kind {
        ContextKind::SharedThread => (
            document
                .ref_id
                .strip_prefix("ctx:thread:")
                .map(str::to_string),
            None,
        ),
        ContextKind::RepoContextFile => (None, document.location.clone()),
        ContextKind::ThreadInsight
        | ContextKind::ThreadFile
        | ContextKind::ThreadSearch
        | ContextKind::ThreadTool => (document.graph.source_threads.first().cloned(), None),
    };

    ContextRef {
        ref_id: document.ref_id.clone(),
        kind: document.kind,
        display_label: document.title.clone(),
        source_thread_id,
        repo_context_id,
        git_branch: document.graph.branches.first().cloned(),
        stale_state: Some(ContextStaleState::Fresh),
    }
}

fn document_bundle_text(document: &ContextDocument) -> String {
    let kind = match document.kind {
        ContextKind::SharedThread => "thread",
        ContextKind::ThreadInsight => "thread insight",
        ContextKind::ThreadFile => "thread file",
        ContextKind::ThreadSearch => "thread search result",
        ContextKind::ThreadTool => "thread tool result",
        ContextKind::RepoContextFile => "repo context",
    };
    let mut lines = vec![
        format!("[Context: {}]", document.title),
        format!("Kind: {kind}"),
    ];
    if let Some(location) = &document.location {
        lines.push(format!("Location: {location}"));
    }
    if let Some(summary) = &document.summary {
        lines.push(format!("Summary: {summary}"));
    }
    if !document.graph.source_threads.is_empty() {
        lines.push(format!(
            "Source threads: {}",
            document.graph.source_threads.join(", ")
        ));
    }
    if !document.graph.source_files.is_empty() {
        lines.push(format!(
            "Source files: {}",
            document.graph.source_files.join(", ")
        ));
    }
    if !document.graph.source_refs.is_empty() {
        lines.push(format!(
            "Source refs: {}",
            document.graph.source_refs.join(", ")
        ));
    }
    if let Some(body) = &document.body
        && !body.trim().is_empty()
    {
        lines.push(String::new());
        lines.push(body.trim().to_string());
    }
    lines.join("\n")
}

fn render_context_bundle(
    documents: &[ContextDocument],
    entries: &[ResolvedContextEntry],
    artifact_paths: Option<&ContextGraphArtifactPaths>,
) -> String {
    if entries.is_empty() {
        return String::new();
    }

    let selected_ref_ids = entries
        .iter()
        .map(|entry| entry.document.ref_id.clone())
        .collect::<HashSet<_>>();
    let all_edges = context_graph_edges(documents);
    let hotspot_scores = context_hotspot_scores(documents, &all_edges, &selected_ref_ids);
    let mut focus_ref_ids = selected_ref_ids.clone();
    focus_ref_ids.extend(context_neighbor_ref_ids(&all_edges, &selected_ref_ids));

    let mut hotspot_documents = documents
        .iter()
        .filter(|document| focus_ref_ids.contains(&document.ref_id))
        .collect::<Vec<_>>();
    hotspot_documents.sort_by(|left, right| {
        hotspot_scores
            .get(&right.ref_id)
            .copied()
            .unwrap_or_default()
            .cmp(
                &hotspot_scores
                    .get(&left.ref_id)
                    .copied()
                    .unwrap_or_default(),
            )
            .then_with(|| context_default_sort_key(left).cmp(&context_default_sort_key(right)))
    });

    let mut lines = vec![
        "Attached collaboration context is discoverable via the repo graph.".to_string(),
        "Only summaries, refs, and hotspots are attached inline. Inspect graph files on demand before relying on details.".to_string(),
    ];
    if let Some(paths) = artifact_paths {
        lines.push(format!("Graph index: {}", paths.index_relative_path));
    }
    lines.push(String::new());
    lines.push("Selected nodes:".to_string());
    for entry in entries {
        let document = &entry.document;
        let mut line = format!(
            "- {} {} · {}",
            context_short_hash(document.ref_id.as_str()),
            context_kind_label(document.kind),
            single_line_excerpt(document.title.as_str(), 80),
        );
        if let Some(summary) = &document.summary {
            line.push_str(&format!(" · {}", single_line_excerpt(summary.as_str(), 96)));
        }
        if let Some(node_path) = artifact_paths
            .and_then(|paths| paths.node_relative_path_by_ref_id.get(&document.ref_id))
        {
            line.push_str(&format!(" · {node_path}"));
        }
        lines.push(line);
    }

    let hotspots = hotspot_documents
        .into_iter()
        .filter(|document| !matches!(document.kind, ContextKind::SharedThread))
        .take(4)
        .collect::<Vec<_>>();
    if !hotspots.is_empty() {
        lines.push(String::new());
        lines.push("Hotspots to inspect or promote:".to_string());
        for document in hotspots {
            let score = hotspot_scores
                .get(&document.ref_id)
                .copied()
                .unwrap_or_default();
            let promotion_kind = inferred_context_write_kind(document);
            lines.push(format!(
                "- {} {} · score={} · promote as {} · {}",
                context_short_hash(document.ref_id.as_str()),
                context_kind_label(document.kind),
                score,
                promotion_kind,
                context_hotspot_reason(document, &all_edges, &selected_ref_ids),
            ));
        }
    }

    lines.push(String::new());
    lines.push("Traversal:".to_string());
    if artifact_paths.is_some() {
        lines.push(
            "- start with the graph index, then open node files only for the refs you need"
                .to_string(),
        );
    } else {
        lines.push(
            "- use the selected refs and summaries to decide which nodes need deeper inspection"
                .to_string(),
        );
    }
    lines.push(
        "- treat repo notes as durable memory and thread artifacts as evidence or recent working context"
            .to_string(),
    );
    lines.push(
        "- promote stable hotspots into .codex/context notes instead of relying on raw thread history"
            .to_string(),
    );
    lines.join("\n")
}

fn sync_context_graph_artifacts(
    repo_root: &Path,
    documents: &[ContextDocument],
    entries: &[ResolvedContextEntry],
) -> Result<ContextGraphArtifactPaths> {
    let artifact_paths = context_graph_artifact_paths(documents);
    let graph_root = repo_root.join(".codex").join("context").join(".graph");
    let nodes_root = graph_root.join("nodes");
    std::fs::create_dir_all(&nodes_root)
        .with_context(|| format!("failed to create {}", nodes_root.display()))?;

    let all_edges = context_graph_edges(documents);
    let selected_ref_ids = entries
        .iter()
        .map(|entry| entry.document.ref_id.clone())
        .collect::<HashSet<_>>();
    let hotspot_scores = context_hotspot_scores(documents, &all_edges, &selected_ref_ids);

    for document in documents {
        let Some(relative_path) = artifact_paths
            .node_relative_path_by_ref_id
            .get(&document.ref_id)
        else {
            continue;
        };
        let path = repo_root.join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        std::fs::write(
            &path,
            render_context_graph_node_file(
                document,
                documents,
                &all_edges,
                &artifact_paths.node_relative_path_by_ref_id,
                &hotspot_scores,
                &selected_ref_ids,
            ),
        )
        .with_context(|| format!("failed to write {}", path.display()))?;
    }

    let index_path = repo_root.join(&artifact_paths.index_relative_path);
    if let Some(parent) = index_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(
        &index_path,
        render_context_graph_index(
            documents,
            entries,
            &all_edges,
            &artifact_paths.node_relative_path_by_ref_id,
            &hotspot_scores,
        ),
    )
    .with_context(|| format!("failed to write {}", index_path.display()))?;

    Ok(artifact_paths)
}

fn context_graph_artifact_paths(documents: &[ContextDocument]) -> ContextGraphArtifactPaths {
    ContextGraphArtifactPaths {
        index_relative_path: ".codex/context/.graph/index.md".to_string(),
        node_relative_path_by_ref_id: documents
            .iter()
            .map(|document| {
                (
                    document.ref_id.clone(),
                    format!(
                        ".codex/context/.graph/nodes/{}/{}-{}.md",
                        context_graph_kind_directory(document.kind),
                        context_short_hash(document.ref_id.as_str()),
                        context_graph_title_slug(document.title.as_str()),
                    ),
                )
            })
            .collect(),
    }
}

fn render_context_graph_index(
    documents: &[ContextDocument],
    entries: &[ResolvedContextEntry],
    edges: &[ContextGraphEdge],
    node_relative_path_by_ref_id: &HashMap<String, String>,
    hotspot_scores: &HashMap<String, usize>,
) -> String {
    let selected_ref_ids = entries
        .iter()
        .map(|entry| entry.document.ref_id.clone())
        .collect::<HashSet<_>>();
    let mut lines = vec![
        "# Context Graph".to_string(),
        String::new(),
        format!("Generated: {}", Utc::now().to_rfc3339()),
        format!("Nodes: {}", documents.len()),
        format!("Edges: {}", edges.len()),
        String::new(),
        "Selected nodes".to_string(),
    ];
    for entry in entries {
        let document = &entry.document;
        let node_path = node_relative_path_by_ref_id
            .get(&document.ref_id)
            .cloned()
            .unwrap_or_default();
        lines.push(format!(
            "- {} | {} | {} | {}",
            context_short_hash(document.ref_id.as_str()),
            context_kind_label(document.kind),
            single_line_excerpt(document.title.as_str(), 80),
            node_path,
        ));
    }

    let mut hotspot_documents = documents.iter().collect::<Vec<_>>();
    hotspot_documents.sort_by(|left, right| {
        hotspot_scores
            .get(&right.ref_id)
            .copied()
            .unwrap_or_default()
            .cmp(
                &hotspot_scores
                    .get(&left.ref_id)
                    .copied()
                    .unwrap_or_default(),
            )
            .then_with(|| context_default_sort_key(left).cmp(&context_default_sort_key(right)))
    });
    lines.push(String::new());
    lines.push("Promotion hotspots".to_string());
    for document in hotspot_documents
        .into_iter()
        .filter(|document| !matches!(document.kind, ContextKind::SharedThread))
        .take(8)
    {
        let selected = if selected_ref_ids.contains(&document.ref_id) {
            "selected"
        } else {
            "linked"
        };
        lines.push(format!(
            "- {} | {} | score={} | {} | {}",
            context_short_hash(document.ref_id.as_str()),
            inferred_context_write_kind(document),
            hotspot_scores
                .get(&document.ref_id)
                .copied()
                .unwrap_or_default(),
            selected,
            single_line_excerpt(document.title.as_str(), 96),
        ));
    }

    lines.push(String::new());
    lines.push("Nodes".to_string());
    for document in documents {
        let node_path = node_relative_path_by_ref_id
            .get(&document.ref_id)
            .cloned()
            .unwrap_or_default();
        let summary = document
            .summary
            .as_deref()
            .map(|summary| single_line_excerpt(summary, 80))
            .unwrap_or_else(|| context_kind_label(document.kind).to_string());
        lines.push(format!(
            "- {} | {} | {} | {} | {}",
            context_short_hash(document.ref_id.as_str()),
            context_kind_label(document.kind),
            single_line_excerpt(document.title.as_str(), 72),
            summary,
            node_path,
        ));
    }

    lines.push(String::new());
    lines.push("Edges".to_string());
    for edge in edges {
        lines.push(format!(
            "- {} -{}-> {}",
            context_short_hash(edge.from_ref_id.as_str()),
            edge.label,
            context_short_hash(edge.to_ref_id.as_str()),
        ));
    }
    lines.join("\n")
}

fn render_context_graph_node_file(
    document: &ContextDocument,
    documents: &[ContextDocument],
    edges: &[ContextGraphEdge],
    node_relative_path_by_ref_id: &HashMap<String, String>,
    hotspot_scores: &HashMap<String, usize>,
    selected_ref_ids: &HashSet<String>,
) -> String {
    let neighbors = context_neighbor_ref_ids(edges, &HashSet::from([document.ref_id.clone()]));
    let related_lines = edges
        .iter()
        .filter_map(|edge| {
            if edge.from_ref_id == document.ref_id {
                Some((edge.label.as_str(), edge.to_ref_id.as_str()))
            } else if edge.to_ref_id == document.ref_id {
                Some((edge.label.as_str(), edge.from_ref_id.as_str()))
            } else {
                None
            }
        })
        .filter_map(|(label, ref_id)| {
            documents
                .iter()
                .find(|candidate| candidate.ref_id == ref_id)
                .map(|neighbor| {
                    let node_path = node_relative_path_by_ref_id
                        .get(ref_id)
                        .cloned()
                        .unwrap_or_default();
                    format!(
                        "- {} -> {} {} · {}",
                        label,
                        context_short_hash(ref_id),
                        single_line_excerpt(neighbor.title.as_str(), 72),
                        node_path,
                    )
                })
        })
        .collect::<Vec<_>>();

    let mut lines = vec![format!("# {}", document.title), String::new()];
    lines.push(format!("Ref: {}", document.ref_id));
    lines.push(format!("Kind: {}", context_kind_label(document.kind)));
    lines.push(format!(
        "Hotspot score: {}",
        hotspot_scores
            .get(&document.ref_id)
            .copied()
            .unwrap_or_default()
    ));
    lines.push(format!(
        "Promotion kind: {}",
        inferred_context_write_kind(document)
    ));
    lines.push(format!(
        "Selected: {}",
        if selected_ref_ids.contains(&document.ref_id) {
            "yes"
        } else {
            "no"
        }
    ));
    lines.push(format!("Linked nodes: {}", neighbors.len()));
    if let Some(location) = &document.location {
        lines.push(format!("Location: {location}"));
    }
    if let Some(summary) = &document.summary {
        lines.push(format!("Summary: {summary}"));
    }
    if !document.graph.source_threads.is_empty() {
        lines.push(format!(
            "Source threads: {}",
            document.graph.source_threads.join(", ")
        ));
    }
    if !document.graph.source_files.is_empty() {
        lines.push(format!(
            "Source files: {}",
            document.graph.source_files.join(", ")
        ));
    }
    if !document.graph.source_refs.is_empty() {
        lines.push(format!(
            "Source refs: {}",
            document.graph.source_refs.join(", ")
        ));
    }
    if !related_lines.is_empty() {
        lines.push(String::new());
        lines.push("## Related".to_string());
        lines.push(String::new());
        lines.extend(related_lines);
    }
    lines.push(String::new());
    lines.push("## Excerpt".to_string());
    lines.push(String::new());
    lines.push(document_bundle_text(document));
    lines.push(String::new());
    lines.join("\n")
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

fn persisted_context_coverage(documents: &[ContextDocument]) -> PersistedContextCoverage {
    let mut coverage = PersistedContextCoverage::default();
    for document in documents {
        if document.kind != ContextKind::RepoContextFile {
            continue;
        }
        coverage
            .source_thread_ids
            .extend(document.graph.source_threads.iter().cloned());
        coverage
            .source_file_paths
            .extend(document.graph.source_files.iter().cloned());
        coverage
            .source_ref_ids
            .extend(document.graph.source_refs.iter().cloned());
    }
    coverage
}

fn context_document_is_persisted(
    document: &ContextDocument,
    coverage: &PersistedContextCoverage,
) -> bool {
    if document.kind == ContextKind::RepoContextFile
        || coverage.source_ref_ids.contains(&document.ref_id)
    {
        return true;
    }

    if document.kind == ContextKind::SharedThread
        && context_thread_id_from_ref_id(&document.ref_id)
            .is_some_and(|thread_id| coverage.source_thread_ids.contains(&thread_id))
    {
        return true;
    }

    document
        .graph
        .source_files
        .iter()
        .any(|path| coverage.source_file_paths.contains(path))
}

#[derive(Debug, PartialEq, Eq)]
struct MemoryPromotePlan {
    promotable_node_ids: Vec<String>,
    already_covered_node_ids: Vec<String>,
}

fn plan_memory_promote(
    documents: &[ContextDocument],
    selected_node_ids: &[String],
) -> MemoryPromotePlan {
    let coverage = persisted_context_coverage(documents);
    let documents_by_ref_id = documents
        .iter()
        .map(|document| (document.ref_id.as_str(), document))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let mut promotable_node_ids = Vec::new();
    let mut already_covered_node_ids = Vec::new();

    for node_id in selected_node_ids {
        if !seen.insert(node_id.as_str()) {
            continue;
        }
        let Some(document) = documents_by_ref_id.get(node_id.as_str()).copied() else {
            continue;
        };
        if matches!(
            document.kind,
            ContextKind::ThreadInsight
                | ContextKind::ThreadFile
                | ContextKind::ThreadSearch
                | ContextKind::ThreadTool
        ) && !context_document_is_persisted(document, &coverage)
        {
            promotable_node_ids.push(node_id.clone());
        } else {
            already_covered_node_ids.push(node_id.clone());
        }
    }

    MemoryPromotePlan {
        promotable_node_ids,
        already_covered_node_ids,
    }
}

fn recommended_handoff_ref_ids(
    documents: &[ContextDocument],
    source_thread_id: &str,
    selected_ref_ids: &[String],
) -> Vec<String> {
    const HANDOFF_RECOMMENDATION_LIMIT: usize = 4;

    let all_edges = context_graph_edges(documents);
    let coverage = persisted_context_coverage(documents);
    let available_ref_ids = documents
        .iter()
        .map(|document| document.ref_id.clone())
        .collect::<HashSet<_>>();
    let mut deduped_selected_ref_ids = Vec::new();
    let mut selected_ref_ids_set = HashSet::new();
    for ref_id in selected_ref_ids {
        if available_ref_ids.contains(ref_id) && selected_ref_ids_set.insert(ref_id.clone()) {
            deduped_selected_ref_ids.push(ref_id.clone());
        }
    }

    let hotspot_scores = context_hotspot_scores(documents, &all_edges, &selected_ref_ids_set);
    let mut recommended_documents = documents
        .iter()
        .filter(|document| {
            matches!(
                document.kind,
                ContextKind::ThreadInsight
                    | ContextKind::ThreadFile
                    | ContextKind::ThreadSearch
                    | ContextKind::ThreadTool
            )
        })
        .filter(|document| {
            document
                .graph
                .source_threads
                .iter()
                .any(|thread_id| thread_id == source_thread_id)
        })
        .filter(|document| !selected_ref_ids_set.contains(&document.ref_id))
        .filter(|document| !context_document_is_persisted(document, &coverage))
        .collect::<Vec<_>>();
    recommended_documents.sort_by(|left, right| {
        hotspot_scores
            .get(&right.ref_id)
            .copied()
            .unwrap_or_default()
            .cmp(
                &hotspot_scores
                    .get(&left.ref_id)
                    .copied()
                    .unwrap_or_default(),
            )
            .then_with(|| context_default_sort_key(left).cmp(&context_default_sort_key(right)))
    });

    deduped_selected_ref_ids.extend(
        recommended_documents
            .into_iter()
            .take(HANDOFF_RECOMMENDATION_LIMIT)
            .map(|document| document.ref_id.clone()),
    );

    if deduped_selected_ref_ids.is_empty()
        && let Some(fallback) = documents
            .iter()
            .filter(|document| {
                !matches!(document.kind, ContextKind::SharedThread)
                    && document
                        .graph
                        .source_threads
                        .iter()
                        .any(|thread_id| thread_id == source_thread_id)
            })
            .max_by(|left, right| {
                hotspot_scores
                    .get(&left.ref_id)
                    .copied()
                    .unwrap_or_default()
                    .cmp(
                        &hotspot_scores
                            .get(&right.ref_id)
                            .copied()
                            .unwrap_or_default(),
                    )
                    .then_with(|| {
                        context_default_sort_key(right).cmp(&context_default_sort_key(left))
                    })
            })
    {
        deduped_selected_ref_ids.push(fallback.ref_id.clone());
    }

    deduped_selected_ref_ids
}

fn handoff_promotion_ref_ids(
    documents: &[ContextDocument],
    selected_ref_ids: &[String],
) -> Vec<String> {
    const HANDOFF_PROMOTION_LIMIT: usize = 4;

    let all_edges = context_graph_edges(documents);
    let selected_ref_ids_set = selected_ref_ids.iter().cloned().collect::<HashSet<_>>();
    let hotspot_scores = context_hotspot_scores(documents, &all_edges, &selected_ref_ids_set);
    let coverage = persisted_context_coverage(documents);
    let documents_by_ref_id = documents
        .iter()
        .map(|document| (document.ref_id.as_str(), document))
        .collect::<HashMap<_, _>>();
    let mut promotion_candidates = selected_ref_ids
        .iter()
        .filter_map(|ref_id| documents_by_ref_id.get(ref_id.as_str()).copied())
        .filter(|document| {
            !matches!(
                document.kind,
                ContextKind::SharedThread | ContextKind::RepoContextFile
            )
        })
        .filter(|document| !context_document_is_persisted(document, &coverage))
        .collect::<Vec<_>>();
    promotion_candidates.sort_by(|left, right| {
        hotspot_scores
            .get(&right.ref_id)
            .copied()
            .unwrap_or_default()
            .cmp(
                &hotspot_scores
                    .get(&left.ref_id)
                    .copied()
                    .unwrap_or_default(),
            )
            .then_with(|| context_default_sort_key(left).cmp(&context_default_sort_key(right)))
    });
    promotion_candidates.dedup_by(|left, right| left.ref_id == right.ref_id);
    promotion_candidates
        .into_iter()
        .take(HANDOFF_PROMOTION_LIMIT)
        .map(|document| document.ref_id.clone())
        .collect()
}

fn context_graph_kind_directory(kind: ContextKind) -> &'static str {
    match kind {
        ContextKind::SharedThread => "threads",
        ContextKind::ThreadInsight => "insights",
        ContextKind::ThreadFile => "files",
        ContextKind::ThreadSearch => "searches",
        ContextKind::ThreadTool => "tools",
        ContextKind::RepoContextFile => "notes",
    }
}

fn context_graph_title_slug(title: &str) -> String {
    let slug = slugify_context_value(title);
    let truncated = slug.chars().take(48).collect::<String>();
    if truncated.is_empty() {
        "context".to_string()
    } else {
        truncated
    }
}

fn context_short_hash(ref_id: &str) -> String {
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

fn context_kind_label(kind: ContextKind) -> &'static str {
    match kind {
        ContextKind::SharedThread => "thread",
        ContextKind::ThreadInsight => "insight",
        ContextKind::ThreadFile => "file",
        ContextKind::ThreadSearch => "search",
        ContextKind::ThreadTool => "tool",
        ContextKind::RepoContextFile => "note",
    }
}

#[cfg(test)]
fn thread_context_document(thread: codex_app_server_protocol::Thread) -> ContextDocument {
    shared_thread_context_document(thread)
}

#[cfg(test)]
fn thread_context_documents(
    thread: &codex_app_server_protocol::Thread,
    is_current_thread: bool,
    repo_root: Option<&Path>,
) -> Vec<ContextDocument> {
    shared_thread_context_documents(thread, is_current_thread, repo_root)
}

async fn source_thread_context_entry(
    bridge: &mut AppServerBridge,
    thread_id: &str,
) -> Result<ResolvedContextEntry, AppServerError> {
    let thread_read = thread_read_with_turn_fallback(bridge, thread_id).await?;
    Ok(resolved_entry_from_document(
        shared_thread_context_document(thread_read.thread),
    ))
}

async fn thread_read_with_turn_fallback(
    bridge: &mut AppServerBridge,
    thread_id: &str,
) -> Result<AppThreadReadResponse, AppServerError> {
    match bridge.thread_read(thread_id.to_string(), true).await {
        Ok(response) => Ok(response),
        Err(err) if app_server_thread_not_loaded(&err) => {
            bridge.thread_read(thread_id.to_string(), false).await
        }
        Err(err) => Err(err),
    }
}

async fn source_thread_cwd(
    bridge: &mut AppServerBridge,
    thread_id: &str,
) -> Result<String, AppServerError> {
    let thread_read = bridge.thread_read(thread_id.to_string(), false).await?;
    Ok(thread_read.thread.cwd.display().to_string())
}

fn thread_summary_from_app_thread(thread: AppThread) -> ThreadSummary {
    let repo_root = get_git_repo_root(&thread.cwd).unwrap_or_else(|| thread.cwd.clone());
    ThreadSummary {
        thread_id: thread.id,
        actor_id: thread.agent_nickname,
        title: thread.name.and_then(non_empty_string),
        preview: non_empty_string(thread.preview),
        repo_root: Some(repo_root.display().to_string()),
        cwd: Some(thread.cwd.display().to_string()),
        git_branch: thread.git_info.and_then(|git_info| git_info.branch),
        goal: None,
        precursor_thread_id: None,
        precursor_kind: None,
        updated_at: Some(thread.updated_at),
    }
}

fn plan_context_write_files(
    repo_root: &Path,
    documents: Vec<ContextDocument>,
    selected_ref_ids: &[String],
    branch: Option<String>,
) -> Vec<PlannedContextWriteFile> {
    let documents_by_ref = documents
        .into_iter()
        .map(|document| (document.ref_id.clone(), document))
        .collect::<HashMap<_, _>>();
    let mut used_paths = HashSet::new();

    selected_ref_ids
        .iter()
        .filter_map(|ref_id| documents_by_ref.get(ref_id))
        .filter_map(|document| {
            plan_context_write_file(repo_root, document, branch.clone(), &mut used_paths)
        })
        .collect()
}

async fn plan_handoff_promotion_files(
    documents: &[ContextDocument],
    selected_ref_ids: &[String],
) -> Vec<PendingContextWriteFile> {
    let promotion_ref_ids = handoff_promotion_ref_ids(documents, selected_ref_ids);
    if promotion_ref_ids.is_empty() {
        return Vec::new();
    }

    let repo_root = match resolve_context_root() {
        Ok(path) => path,
        Err(err) => {
            warn!(error = %err, "failed to resolve collaboration context root for handoff promotion");
            return Vec::new();
        }
    };
    let branch = current_branch_name(repo_root.as_path())
        .await
        .unwrap_or_default();
    plan_context_write_files(
        repo_root.as_path(),
        documents.to_vec(),
        &promotion_ref_ids,
        non_empty_string(branch),
    )
    .into_iter()
    .map(|file| PendingContextWriteFile {
        relative_path: file.relative_path,
        content: file.content,
    })
    .collect()
}

fn plan_context_write_file(
    repo_root: &Path,
    document: &ContextDocument,
    branch: Option<String>,
    used_paths: &mut HashSet<String>,
) -> Option<PlannedContextWriteFile> {
    let existing_relative_path = document.location.clone().filter(|location| {
        document.kind == ContextKind::RepoContextFile && location.ends_with(".md")
    });
    let existing_content = existing_relative_path
        .as_ref()
        .and_then(|relative_path| std::fs::read_to_string(repo_root.join(relative_path)).ok());
    let existing_metadata = existing_content
        .as_deref()
        .map(split_optional_frontmatter)
        .and_then(|(frontmatter, _)| frontmatter)
        .map(|frontmatter| parse_repo_context_metadata(frontmatter.as_str()))
        .unwrap_or_default();

    let kind = existing_metadata
        .kind
        .clone()
        .unwrap_or_else(|| inferred_context_write_kind(document));
    let title = existing_metadata
        .title
        .clone()
        .unwrap_or_else(|| context_write_source_title(document));
    let relative_path = context_write_relative_path(
        repo_root,
        document,
        kind.as_str(),
        existing_relative_path,
        used_paths,
    );
    let exists = repo_root.join(&relative_path).exists();
    let id = existing_metadata
        .id
        .unwrap_or_else(|| context_write_id_from_path(&relative_path));
    let source_threads = document.graph.source_threads.clone();
    let source_files = document.graph.source_files.clone();
    let source_refs = if document.kind == ContextKind::RepoContextFile {
        existing_metadata.source_refs
    } else {
        vec![document.ref_id.clone()]
    };
    let content = render_context_write_file(
        ContextWriteMetadata {
            id,
            kind: kind.clone(),
            title: title.clone(),
            branch,
            source_threads,
            source_files,
            source_refs: source_refs.clone(),
            last_validated_at: Utc::now().format("%Y-%m-%d").to_string(),
        },
        context_write_body(document, existing_content.as_deref(), &source_refs),
    );

    Some(PlannedContextWriteFile {
        relative_path,
        title,
        kind,
        exists,
        content,
    })
}

#[derive(Debug)]
struct ContextWriteMetadata {
    id: String,
    kind: String,
    title: String,
    branch: Option<String>,
    source_threads: Vec<String>,
    source_files: Vec<String>,
    source_refs: Vec<String>,
    last_validated_at: String,
}

fn render_context_write_file(metadata: ContextWriteMetadata, body: String) -> String {
    let mut lines = vec![
        "---".to_string(),
        format!("id: {}", yaml_quoted(&metadata.id)),
        format!("kind: {}", yaml_quoted(&metadata.kind)),
        format!("title: {}", yaml_quoted(&metadata.title)),
        "applies_to:".to_string(),
    ];
    if let Some(branch) = metadata.branch {
        lines.push("  branches:".to_string());
        lines.push(format!("    - {}", yaml_quoted(&branch)));
    } else {
        lines.push("  branches: []".to_string());
    }
    lines.extend(render_yaml_list("source_threads", &metadata.source_threads));
    lines.extend(render_yaml_list("source_files", &metadata.source_files));
    lines.extend(render_yaml_list("source_refs", &metadata.source_refs));
    lines.push(format!("last_validated_at: {}", metadata.last_validated_at));
    lines.push("---".to_string());
    lines.push(String::new());
    lines.push(body.trim().to_string());
    lines.push(String::new());
    lines.join("\n")
}

fn render_yaml_list(label: &str, values: &[String]) -> Vec<String> {
    if values.is_empty() {
        return vec![format!("{label}: []")];
    }

    let mut lines = vec![format!("{label}:")];
    for value in values {
        lines.push(format!("  - {}", yaml_quoted(value)));
    }
    lines
}

fn yaml_quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn context_write_relative_path(
    repo_root: &Path,
    document: &ContextDocument,
    kind: &str,
    existing_relative_path: Option<String>,
    used_paths: &mut HashSet<String>,
) -> String {
    if let Some(relative_path) = existing_relative_path
        && used_paths.insert(relative_path.clone())
    {
        return relative_path;
    }

    let directory = context_write_directory_for_kind(kind);
    let slug = slugify_context_value(&context_write_source_title(document));
    let mut candidate = format!(".codex/context/{directory}/{slug}.md");
    let mut suffix = 2usize;
    while !used_paths.insert(candidate.clone()) || repo_root.join(&candidate).exists() {
        candidate = format!(".codex/context/{directory}/{slug}-{suffix}.md");
        suffix += 1;
    }
    candidate
}

fn context_write_directory_for_kind(kind: &str) -> &'static str {
    match kind {
        "decision" => "decisions",
        "playbook" => "playbooks",
        "hotspot" => "hotspots",
        _ => "concepts",
    }
}

fn context_write_source_title(document: &ContextDocument) -> String {
    let title = if document.kind == ContextKind::SharedThread
        && let Some(stripped) = document.title.strip_prefix("🦞 ")
    {
        stripped
            .split_once(" · ")
            .map(|(_, title)| title)
            .unwrap_or(stripped)
            .to_string()
    } else {
        document.title.clone()
    };
    if document.kind == ContextKind::SharedThread
        && let Some(thread_id) = document.ref_id.strip_prefix("ctx:thread:")
    {
        let short_thread_title =
            format!("Thread {}", thread_id.chars().take(8).collect::<String>());
        if title.eq_ignore_ascii_case("Current Thread")
            || title.eq_ignore_ascii_case(&short_thread_title)
        {
            return thread_id.to_string();
        }
    }
    title
}

fn inferred_context_write_kind(document: &ContextDocument) -> String {
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

fn context_write_id_from_path(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "context-note".to_string())
}

fn slugify_context_value(value: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "context-note".to_string()
    } else {
        slug
    }
}

fn context_write_body(
    document: &ContextDocument,
    existing_content: Option<&str>,
    source_refs: &[String],
) -> String {
    if let Some(existing_content) = existing_content {
        let (_, body) = split_optional_frontmatter(existing_content);
        if !body.trim().is_empty() {
            return body.trim().to_string();
        }
    }

    let mut lines = vec![format!("# {}", context_write_source_title(document))];
    if let Some(summary) = &document.summary {
        lines.push(String::new());
        lines.push(summary.clone());
    }
    if let Some(body) = &document.body
        && !body.trim().is_empty()
    {
        lines.push(String::new());
        lines.push("## Details".to_string());
        lines.push(String::new());
        lines.push(body.trim().to_string());
    }
    lines.push(String::new());
    lines.push("## Sources".to_string());
    lines.push(String::new());
    if document.kind == ContextKind::RepoContextFile
        && let Some(location) = &document.location
    {
        lines.push(format!("- repo context: {location}"));
    }
    for thread_id in &document.graph.source_threads {
        lines.push(format!("- thread: {thread_id}"));
    }
    for source_file in &document.graph.source_files {
        lines.push(format!("- file: {source_file}"));
    }
    for source_ref in source_refs {
        lines.push(format!("- ref: {source_ref}"));
    }
    lines.join("\n")
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

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> JsonRpcResponse {
    JsonRpcResponse::err(id, code, message)
}

fn app_server_error_response(id: Value, err: AppServerError) -> JsonRpcResponse {
    match err {
        AppServerError::Rpc { code: -32001, .. } => {
            rpc_error(id, RPC_ERR_OVERLOADED, "TOGETHER_OVERLOADED")
        }
        AppServerError::Rpc { code, message } if code == -32602 => rpc_error(id, code, message),
        AppServerError::Rpc { code, message } => {
            warn!(code, message = %message, "app-server returned RPC error");
            rpc_error(id, -32603, format!("app-server error {code}: {message}"))
        }
        AppServerError::Transport(err) => {
            warn!(error = %err, "app-server transport failure");
            rpc_error(id, -32603, "app-server unavailable")
        }
        AppServerError::Decode(err) => {
            warn!(error = %err, "app-server protocol decode failure");
            rpc_error(id, -32603, "app-server protocol error")
        }
    }
}

fn app_server_thread_not_loaded(err: &AppServerError) -> bool {
    matches!(
        err,
        AppServerError::Rpc { code, message }
            if (*code == -32600 || *code == -32602 || *code == -32603)
                && message.to_ascii_lowercase().contains("thread not loaded")
    )
}

fn thread_start_sandbox_mode_from_policy(
    policy: codex_protocol::protocol::SandboxPolicy,
) -> Option<AppServerSandboxMode> {
    match policy {
        codex_protocol::protocol::SandboxPolicy::DangerFullAccess => {
            Some(AppServerSandboxMode::DangerFullAccess)
        }
        codex_protocol::protocol::SandboxPolicy::ReadOnly { .. } => {
            Some(AppServerSandboxMode::ReadOnly)
        }
        codex_protocol::protocol::SandboxPolicy::WorkspaceWrite { .. } => {
            Some(AppServerSandboxMode::WorkspaceWrite)
        }
        codex_protocol::protocol::SandboxPolicy::ExternalSandbox { .. } => None,
    }
}

#[derive(Debug)]
enum AppServerError {
    Rpc { code: i64, message: String },
    Transport(anyhow::Error),
    Decode(anyhow::Error),
}

impl AppServerError {
    fn is_overloaded(&self) -> bool {
        matches!(self, Self::Rpc { code: -32001, .. })
    }

    fn is_transport(&self) -> bool {
        matches!(self, Self::Transport(_))
    }
}

struct AppServerBridge {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_request_id: i64,
}

impl AppServerBridge {
    async fn spawn_current_binary() -> Result<Self, AppServerError> {
        let (child, stdin, stdout) = Self::spawn_child().await?;
        let mut bridge = Self {
            child,
            stdin,
            stdout,
            next_request_id: 1,
        };
        bridge.initialize_handshake().await?;
        Ok(bridge)
    }

    async fn spawn_child() -> Result<(Child, ChildStdin, BufReader<ChildStdout>), AppServerError> {
        let exe = std::env::current_exe().map_err(|err| {
            AppServerError::Transport(anyhow::anyhow!(
                "failed to locate current executable: {err}"
            ))
        })?;

        let mut child = Command::new(&exe)
            .arg("app-server")
            .arg("--listen")
            .arg("stdio://")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|err| {
                AppServerError::Transport(anyhow::anyhow!(
                    "failed to spawn `{}` app-server: {err}",
                    exe.display()
                ))
            })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            AppServerError::Transport(anyhow::anyhow!("codex app-server stdin unavailable"))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AppServerError::Transport(anyhow::anyhow!("codex app-server stdout unavailable"))
        })?;

        Ok((child, stdin, BufReader::new(stdout)))
    }

    async fn restart(&mut self) -> Result<(), AppServerError> {
        warn!("restarting embedded codex app-server bridge");
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;

        let (child, stdin, stdout) = Self::spawn_child().await?;
        self.child = child;
        self.stdin = stdin;
        self.stdout = stdout;
        self.next_request_id = 1;
        self.initialize_handshake().await
    }

    async fn initialize_handshake(&mut self) -> Result<(), AppServerError> {
        let init_params = InitializeParams {
            client_info: ClientInfo {
                name: "codex-together-server".to_string(),
                title: Some("Codex Together Server".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            capabilities: Some(InitializeCapabilities {
                experimental_api: true,
                opt_out_notification_methods: None,
            }),
        };

        let _: InitializeResponse = self
            .call(
                "initialize",
                serde_json::to_value(init_params).map_err(|err| {
                    AppServerError::Decode(anyhow::anyhow!(
                        "failed to serialize initialize params: {err}"
                    ))
                })?,
            )
            .await?;

        let initialized = JSONRPCMessage::Notification(AppJsonRpcNotification {
            method: METHOD_INITIALIZED.to_string(),
            params: None,
        });
        self.write_message(&initialized).await?;

        Ok(())
    }

    async fn thread_read(
        &mut self,
        thread_id: String,
        include_turns: bool,
    ) -> Result<AppThreadReadResponse, AppServerError> {
        self.request_with_retry(
            "thread/read",
            serde_json::to_value(AppThreadReadParams {
                thread_id,
                include_turns,
            })
            .map_err(|err| {
                AppServerError::Decode(anyhow::anyhow!(
                    "failed to serialize thread/read params: {err}"
                ))
            })?,
        )
        .await
    }

    async fn thread_list(
        &mut self,
        query: Option<String>,
        cursor: Option<String>,
        limit: Option<u32>,
    ) -> Result<AppThreadListResponse, AppServerError> {
        self.request_with_retry(
            "thread/list",
            serde_json::to_value(AppThreadListParams {
                cursor,
                limit,
                sort_key: Some(AppThreadSortKey::UpdatedAt),
                model_providers: None,
                source_kinds: None,
                archived: Some(false),
                cwd: None,
                search_term: query,
            })
            .map_err(|err| {
                AppServerError::Decode(anyhow::anyhow!(
                    "failed to serialize thread/list params: {err}"
                ))
            })?,
        )
        .await
    }

    async fn thread_start(
        &mut self,
        cwd: Option<String>,
        model: Option<String>,
        approval_policy: Option<codex_protocol::protocol::AskForApproval>,
        sandbox: Option<codex_protocol::protocol::SandboxPolicy>,
    ) -> Result<AppThreadStartResponse, AppServerError> {
        let approval_policy = approval_policy.map(Into::into);
        let sandbox = sandbox.and_then(thread_start_sandbox_mode_from_policy);
        self.request_with_retry(
            "thread/start",
            serde_json::to_value(AppThreadStartParams {
                model,
                model_provider: None,
                cwd,
                approval_policy,
                sandbox,
                config: None,
                service_name: None,
                base_instructions: None,
                developer_instructions: None,
                personality: None,
                ephemeral: None,
                dynamic_tools: None,
                mock_experimental_field: None,
                experimental_raw_events: false,
                persist_extended_history: true,
                materialize_rollout_path: true,
            })
            .map_err(|err| {
                AppServerError::Decode(anyhow::anyhow!(
                    "failed to serialize thread/start params: {err}"
                ))
            })?,
        )
        .await
    }

    async fn request_with_retry<T>(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<T, AppServerError>
    where
        T: DeserializeOwned,
    {
        let mut restarted_after_transport_error = false;
        let mut overload_attempt = 0usize;

        loop {
            match self.call(method, params.clone()).await {
                Ok(response) => return Ok(response),
                Err(err)
                    if err.is_overloaded()
                        && overload_attempt < APP_SERVER_MAX_OVERLOAD_RETRIES =>
                {
                    let sleep_ms = APP_SERVER_OVERLOAD_BACKOFF_MS[overload_attempt];
                    overload_attempt += 1;
                    tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                }
                Err(err) if err.is_transport() && !restarted_after_transport_error => {
                    restarted_after_transport_error = true;
                    self.restart().await?;
                }
                Err(err) => return Err(err),
            }
        }
    }

    async fn call<T>(&mut self, method: &str, params: Value) -> Result<T, AppServerError>
    where
        T: DeserializeOwned,
    {
        let request_id = RequestId::Integer(self.next_request_id);
        self.next_request_id += 1;

        let request = JSONRPCMessage::Request(JSONRPCRequest {
            id: request_id.clone(),
            method: method.to_string(),
            params: Some(params),
        });
        self.write_message(&request).await?;

        loop {
            match self.read_message().await? {
                JSONRPCMessage::Response(response) if response.id == request_id => {
                    return serde_json::from_value(response.result).map_err(|err| {
                        AppServerError::Decode(anyhow::anyhow!(
                            "failed to decode `{method}` response payload: {err}"
                        ))
                    });
                }
                JSONRPCMessage::Error(err) if err.id == request_id => {
                    return Err(AppServerError::Rpc {
                        code: err.error.code,
                        message: err.error.message,
                    });
                }
                JSONRPCMessage::Request(server_request) => {
                    self.reply_unsupported_request(server_request.id).await?;
                }
                JSONRPCMessage::Notification(_) => {
                    // Best-effort ignore for now.
                }
                _ => {
                    // Another in-flight request should not exist because calls are serialized.
                }
            }
        }
    }

    async fn reply_unsupported_request(&mut self, id: RequestId) -> Result<(), AppServerError> {
        let response = JSONRPCMessage::Error(JSONRPCError {
            id,
            error: JSONRPCErrorError {
                code: -32601,
                data: None,
                message: "unsupported server request in together bridge".to_string(),
            },
        });
        self.write_message(&response).await
    }

    async fn write_message(&mut self, message: &JSONRPCMessage) -> Result<(), AppServerError> {
        let payload = serde_json::to_string(message).map_err(|err| {
            AppServerError::Decode(anyhow::anyhow!("failed to encode JSON-RPC message: {err}"))
        })?;

        self.stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|err| {
                AppServerError::Transport(anyhow::anyhow!(
                    "failed to write to codex app-server stdin: {err}"
                ))
            })?;
        self.stdin.write_all(b"\n").await.map_err(|err| {
            AppServerError::Transport(anyhow::anyhow!(
                "failed to write newline to codex app-server stdin: {err}"
            ))
        })?;
        self.stdin.flush().await.map_err(|err| {
            AppServerError::Transport(anyhow::anyhow!(
                "failed to flush codex app-server stdin: {err}"
            ))
        })
    }

    async fn read_message(&mut self) -> Result<JSONRPCMessage, AppServerError> {
        loop {
            let mut line = String::new();
            let bytes = self.stdout.read_line(&mut line).await.map_err(|err| {
                AppServerError::Transport(anyhow::anyhow!(
                    "failed to read codex app-server stdout: {err}"
                ))
            })?;

            if bytes == 0 {
                return Err(AppServerError::Transport(anyhow::anyhow!(
                    "codex app-server closed stdout"
                )));
            }

            if line.trim().is_empty() {
                continue;
            }

            let message: JSONRPCMessage = serde_json::from_str(line.trim()).map_err(|err| {
                AppServerError::Decode(anyhow::anyhow!(
                    "invalid JSON-RPC payload from codex app-server: {err}"
                ))
            })?;
            return Ok(message);
        }
    }
}

struct SingletonLock {
    path: PathBuf,
}

impl SingletonLock {
    fn acquire(codex_home: &Path) -> Result<Self> {
        let lock_dir = codex_home.join("together");
        std::fs::create_dir_all(&lock_dir)
            .with_context(|| format!("failed to create lock directory {}", lock_dir.display()))?;

        let lock_path = lock_dir.join("session.lock");
        loop {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(mut file) => {
                    let pid = std::process::id();
                    let now = Utc::now().to_rfc3339();
                    let content = format!("{pid}\n{now}\n");
                    file.write_all(content.as_bytes()).with_context(|| {
                        format!("failed to write lock file {}", lock_path.display())
                    })?;
                    return Ok(Self { path: lock_path });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if clear_stale_lock(&lock_path)? {
                        continue;
                    }
                    anyhow::bail!("TOGETHER_SINGLETON_CONFLICT");
                }
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("failed to create lock {}", lock_path.display()));
                }
            }
        }
    }
}

impl Drop for SingletonLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn clear_stale_lock(path: &Path) -> Result<bool> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read lock {}", path.display()));
        }
    };

    let pid = content
        .lines()
        .next()
        .and_then(|line| line.trim().parse::<u32>().ok())
        .unwrap_or(0);

    if pid != 0 && is_pid_running(pid) {
        return Ok(false);
    }

    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(err) => {
            Err(err).with_context(|| format!("failed to remove stale lock {}", path.display()))
        }
    }
}

#[cfg(unix)]
fn is_pid_running(pid: u32) -> bool {
    let pid_text = pid.to_string();
    if let Ok(output) = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid_text])
        .output()
        && output.status.success()
    {
        let stat = String::from_utf8_lossy(&output.stdout);
        if stat.trim_start().starts_with('Z') {
            return false;
        }
    }

    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }

    matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(code) if code == libc::EPERM
    )
}

#[cfg(not(unix))]
fn is_pid_running(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::MemoryPromotePlan;
    use super::build_context_graph;
    use super::context_focus_thread_ids_for_query;
    use super::context_graph_artifact_paths;
    use super::handoff_promotion_ref_ids;
    use super::load_thread_from_rollout_at;
    use super::plan_context_write_files;
    use super::plan_memory_promote;
    use super::recommended_handoff_ref_ids;
    use super::render_context_bundle;
    use super::repo_context_documents;
    use super::resolved_entry_from_document;
    use super::search_context_documents;
    use super::thread_context_document;
    use super::thread_context_documents;
    use super::thread_summary_from_app_thread;
    use codex_app_server_protocol::CommandAction;
    use codex_app_server_protocol::CommandExecutionStatus;
    use codex_app_server_protocol::GitInfo;
    use codex_app_server_protocol::SessionSource;
    use codex_app_server_protocol::Thread;
    use codex_app_server_protocol::ThreadItem;
    use codex_app_server_protocol::ThreadStatus;
    use codex_app_server_protocol::Turn;
    use codex_app_server_protocol::TurnStatus as AppTurnStatus;
    use codex_context_graph::build_context_query;
    use codex_protocol::ThreadId;
    use codex_together_protocol::ContextEdgeType;
    use codex_together_protocol::ContextKind;
    use codex_together_protocol::ContextMountReason;
    use codex_together_protocol::ContextPrecursorKind;
    use codex_together_protocol::ContextQueryEdge;
    use codex_together_protocol::ContextQueryNode;
    use codex_together_protocol::ContextQueryParams;
    use codex_together_protocol::ContextThreadNode;
    use codex_together_protocol::ThreadArtifactKind;
    use std::path::Path;
    use std::path::PathBuf;

    #[test]
    fn repo_context_documents_parse_frontmatter_and_body() {
        let temp_root = temp_test_dir("repo-context-docs");
        let context_dir = temp_root.join(".codex").join("context");
        std::fs::create_dir_all(&context_dir).expect("create context dir");
        std::fs::write(
            context_dir.join("overview.md"),
            "---\ntitle: Planning Notes\nkind: plan\napplies_to:\n  branches:\n    - \"rewrite-codex-2gether-v2\"\nsource_threads:\n  - \"thread-1\"\nsource_files:\n  - \".codex/context/reference.md\"\nsource_refs:\n  - \"ctx:thread-insight:thread-1:plan-1\"\nvisibility: \"repo\"\n---\n# Planning Notes\n\nowner=zanechee@local · shared_by=zanechee@local · shared_at=2026-03-15T22:41:10.744997+00:00\nShip the context browser first.\nShared by: zanechee@local\n",
        )
        .expect("write repo context");

        let documents = repo_context_documents(&temp_root);

        assert_eq!(documents.len(), 1);
        let document = &documents[0];
        assert_eq!(document.kind, ContextKind::RepoContextFile);
        assert_eq!(document.title, "Planning Notes");
        assert_eq!(
            document.location.as_deref(),
            Some(".codex/context/overview.md")
        );
        assert_eq!(
            document.summary.as_deref(),
            Some("plan · Ship the context browser first.")
        );
        assert_eq!(
            document.body.as_deref(),
            Some("# Planning Notes\n\nShip the context browser first.")
        );
        assert_eq!(
            document.graph.branches,
            vec!["rewrite-codex-2gether-v2".to_string()]
        );
        assert_eq!(document.graph.source_threads, vec!["thread-1".to_string()]);
        assert_eq!(
            document.graph.source_files,
            vec![".codex/context/reference.md".to_string()]
        );
        assert_eq!(
            document.graph.source_refs,
            vec!["ctx:thread-insight:thread-1:plan-1".to_string()]
        );

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn build_context_graph_keeps_repo_note_without_synthesizing_prompt_thread() {
        let temp_root = temp_test_dir("context-graph-repo-only");
        let context_dir = temp_root.join(".codex").join("context");
        std::fs::create_dir_all(&context_dir).expect("create context dir");
        std::fs::write(
            context_dir.join("overview.md"),
            "---\ntitle: Planning Overview\nkind: concept\napplies_to:\n  branches:\n    - \"rewrite-codex-2gether-v2\"\nsource_threads:\n  - \"thread-1\"\nsource_files: []\n---\n# Planning Overview\n\nowner=zanechee@local · shared_by=zanechee@local\n\n## Details\n\nThread: thread-1\nPreview: planning sync\nGit branch: rewrite-codex-2gether-v2\n",
        )
        .expect("write repo context");

        let documents = repo_context_documents(&temp_root);
        let graph = build_context_graph(documents, Some("planning"), 10, None);

        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(
            graph
                .nodes
                .iter()
                .map(|node| (node.title.clone(), node.summary.clone().unwrap_or_default()))
                .collect::<Vec<_>>(),
            vec![(
                "Planning Overview".to_string(),
                "concept · branch=rewrite-codex-2gether-v2 · 1 source thread".to_string(),
            ),]
        );
        assert!(graph.edges.is_empty());

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn build_context_graph_links_repo_notes_to_threads_by_source_and_branch() {
        let temp_root = temp_test_dir("context-graph-links");
        let context_dir = temp_root.join(".codex").join("context");
        std::fs::create_dir_all(&context_dir).expect("create context dir");
        std::fs::write(
            context_dir.join("overview.md"),
            "---\ntitle: Planning Overview\nkind: concept\napplies_to:\n  branches:\n    - \"rewrite-codex-2gether-v2\"\nsource_threads:\n  - \"thread-1\"\n---\n# Planning Overview\n\nShip the context browser first.\n",
        )
        .expect("write repo context");

        let mut documents = repo_context_documents(&temp_root);
        let thread = sample_thread(
            "thread-1",
            Some("planning sync"),
            Some("rewrite-codex-2gether-v2"),
        );
        documents.extend(thread_context_documents(
            &thread,
            true,
            Some(temp_root.as_path()),
        ));

        let graph = build_context_graph(documents, None, 10, Some("thread-1"));

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(
            graph
                .nodes
                .iter()
                .map(|node| node.ref_id.clone())
                .collect::<Vec<_>>(),
            vec![
                "ctx:thread:thread-1".to_string(),
                "ctx:file:.codex/context/overview.md".to_string(),
            ]
        );
        assert_eq!(
            graph
                .edges
                .iter()
                .map(|edge| {
                    (
                        edge.from_ref_id.clone(),
                        edge.to_ref_id.clone(),
                        edge.label.clone(),
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                (
                    "ctx:thread:thread-1".to_string(),
                    "ctx:file:.codex/context/overview.md".to_string(),
                    "branch".to_string(),
                ),
                (
                    "ctx:thread:thread-1".to_string(),
                    "ctx:file:.codex/context/overview.md".to_string(),
                    "source".to_string(),
                ),
            ]
        );

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn build_context_graph_roots_on_current_thread_component() {
        let temp_root = temp_test_dir("context-graph-current-component");
        let context_dir = temp_root.join(".codex").join("context");
        std::fs::create_dir_all(&context_dir).expect("create context dir");
        std::fs::write(
            context_dir.join("overview.md"),
            "---\ntitle: Planning Overview\nkind: concept\napplies_to:\n  branches:\n    - \"rewrite-codex-2gether-v2\"\nsource_threads:\n  - \"thread-1\"\n---\n# Planning Overview\n\nShip the context browser first.\n",
        )
        .expect("write planning context");
        std::fs::write(
            context_dir.join("archive.md"),
            "---\ntitle: Archive Notes\nkind: concept\n---\nOld planning scratchpad.\n",
        )
        .expect("write archive context");

        let mut documents = repo_context_documents(&temp_root);
        let thread = sample_thread(
            "thread-1",
            Some("planning sync"),
            Some("rewrite-codex-2gether-v2"),
        );
        documents.extend(thread_context_documents(
            &thread,
            true,
            Some(temp_root.as_path()),
        ));

        let graph = build_context_graph(documents, None, 10, Some("thread-1"));

        assert_eq!(
            graph
                .nodes
                .iter()
                .map(|node| node.ref_id.clone())
                .collect::<Vec<_>>(),
            vec![
                "ctx:thread:thread-1".to_string(),
                "ctx:file:.codex/context/overview.md".to_string(),
            ]
        );

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn build_context_graph_links_repo_notes_to_artifacts_by_source_ref() {
        let temp_root = temp_test_dir("context-graph-derived-links");
        let context_dir = temp_root.join(".codex").join("context");
        std::fs::create_dir_all(&context_dir).expect("create context dir");
        std::fs::write(
            context_dir.join("overview.md"),
            "---\ntitle: Branch Plan\nkind: concept\nsource_threads:\n  - \"thread-1\"\nsource_refs:\n  - \"ctx:thread-insight:thread-1:plan-1\"\n---\n# Branch Plan\n\nShip the graph rewrite.\n",
        )
        .expect("write planning context");

        let mut thread = sample_thread("thread-1", Some("planning sync"), Some("main"));
        thread.turns = vec![Turn {
            id: "turn-1".to_string(),
            items: vec![ThreadItem::Plan {
                id: "plan-1".to_string(),
                text: "Ship the graph rewrite.".to_string(),
            }],
            status: AppTurnStatus::Completed,
            error: None,
        }];
        let mut documents = repo_context_documents(&temp_root);
        documents.extend(thread_context_documents(
            &thread,
            true,
            Some(temp_root.as_path()),
        ));

        let graph = build_context_graph(documents, None, 10, Some("thread-1"));

        assert!(graph.edges.iter().any(|edge| {
            edge.from_ref_id == "ctx:thread-insight:thread-1:plan-1"
                && edge.to_ref_id == "ctx:file:.codex/context/overview.md"
                && edge.label == "derived"
        }));

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn context_write_plan_updates_existing_repo_context_file() {
        let temp_root = temp_test_dir("repo-context-write-existing");
        let context_dir = temp_root.join(".codex").join("context");
        std::fs::create_dir_all(&context_dir).expect("create context dir");
        std::fs::write(
            context_dir.join("overview.md"),
            "---\nid: planning-overview\nkind: decision\ntitle: Planning Overview\n---\n# Planning Overview\n\nKeep the body stable.\n",
        )
        .expect("write repo context");

        let documents = repo_context_documents(&temp_root);
        let planned = plan_context_write_files(
            &temp_root,
            documents,
            &["ctx:file:.codex/context/overview.md".to_string()],
            Some("rewrite-codex-2gether-v2".to_string()),
        );

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].relative_path, ".codex/context/overview.md");
        assert!(planned[0].exists);
        assert!(planned[0].content.contains("id: \"planning-overview\""));
        assert!(planned[0].content.contains("kind: \"decision\""));
        assert!(planned[0].content.contains("\"rewrite-codex-2gether-v2\""));
        assert!(
            planned[0]
                .content
                .contains("# Planning Overview\n\nKeep the body stable.")
        );

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn context_write_plan_creates_new_file_for_thread() {
        let temp_root = temp_test_dir("repo-context-write-thread");
        let thread = sample_thread("thread-1", Some("planning sync"), None);
        let documents = thread_context_documents(&thread, true, None);
        let selected_ref_ids = vec!["ctx:thread:thread-1".to_string()];

        let planned = plan_context_write_files(
            &temp_root,
            documents,
            &selected_ref_ids,
            Some("rewrite-codex-2gether-v2".to_string()),
        );

        assert_eq!(planned.len(), 1);
        assert_eq!(
            planned[0].relative_path,
            ".codex/context/concepts/thread-1.md"
        );
        assert!(!planned[0].exists);
        assert!(planned[0].content.contains("source_threads:"));
        assert!(planned[0].content.contains("- \"thread-1\""));
        assert!(planned[0].content.contains("## Sources"));
        assert!(planned[0].content.contains("- thread: thread-1"));

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn context_write_plan_creates_new_file_for_artifact_with_source_ref() {
        let temp_root = temp_test_dir("repo-context-write-artifact");
        let mut thread = sample_thread("thread-1", Some("planning sync"), None);
        thread.turns = vec![Turn {
            id: "turn-1".to_string(),
            items: vec![ThreadItem::Plan {
                id: "plan-1".to_string(),
                text: "Capture the discovery-pack behavior.".to_string(),
            }],
            status: AppTurnStatus::Completed,
            error: None,
        }];
        let documents = thread_context_documents(&thread, true, None);

        let planned = plan_context_write_files(
            &temp_root,
            documents,
            &["ctx:thread-insight:thread-1:plan-1".to_string()],
            Some("main".to_string()),
        );

        assert_eq!(planned.len(), 1);
        assert!(
            planned[0]
                .content
                .contains("source_refs:\n  - \"ctx:thread-insight:thread-1:plan-1\"")
        );
        assert!(
            planned[0]
                .content
                .contains("- ref: ctx:thread-insight:thread-1:plan-1")
        );
    }

    #[test]
    fn recommended_handoff_ref_ids_preserves_selected_and_adds_unpersisted_artifacts() {
        let temp_root = temp_test_dir("handoff-recommendations");
        let context_dir = temp_root.join(".codex").join("context");
        std::fs::create_dir_all(&context_dir).expect("create context dir");
        std::fs::write(
            context_dir.join("overview.md"),
            "---\ntitle: Planning Overview\nkind: concept\nsource_threads:\n  - \"thread-1\"\nsource_refs:\n  - \"ctx:thread-insight:thread-1:plan-1\"\n---\n# Planning Overview\n\nPersisted already.\n",
        )
        .expect("write repo context");

        let mut thread = sample_thread("thread-1", Some("planning sync"), Some("main"));
        thread.turns = vec![Turn {
            id: "turn-1".to_string(),
            items: vec![
                ThreadItem::Plan {
                    id: "plan-1".to_string(),
                    text: "Persisted already.".to_string(),
                },
                ThreadItem::Plan {
                    id: "plan-2".to_string(),
                    text: "Capture the simplified handoff selection flow.".to_string(),
                },
            ],
            status: AppTurnStatus::Completed,
            error: None,
        }];

        let mut documents = repo_context_documents(&temp_root);
        documents.extend(thread_context_documents(
            &thread,
            true,
            Some(temp_root.as_path()),
        ));

        let recommended_ref_ids = recommended_handoff_ref_ids(
            &documents,
            "thread-1",
            &["ctx:file:.codex/context/overview.md".to_string()],
        );

        assert_eq!(
            recommended_ref_ids,
            vec![
                "ctx:file:.codex/context/overview.md".to_string(),
                "ctx:thread-insight:thread-1:plan-2".to_string(),
            ]
        );

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn handoff_promotion_ref_ids_skip_repo_notes_and_persisted_artifacts() {
        let temp_root = temp_test_dir("handoff-promotion");
        let context_dir = temp_root.join(".codex").join("context");
        std::fs::create_dir_all(&context_dir).expect("create context dir");
        std::fs::write(
            context_dir.join("overview.md"),
            "---\ntitle: Planning Overview\nkind: concept\nsource_threads:\n  - \"thread-1\"\nsource_refs:\n  - \"ctx:thread-insight:thread-1:plan-1\"\n---\n# Planning Overview\n\nPersisted already.\n",
        )
        .expect("write repo context");

        let mut thread = sample_thread("thread-1", Some("planning sync"), Some("main"));
        thread.turns = vec![Turn {
            id: "turn-1".to_string(),
            items: vec![
                ThreadItem::Plan {
                    id: "plan-1".to_string(),
                    text: "Persisted already.".to_string(),
                },
                ThreadItem::Plan {
                    id: "plan-2".to_string(),
                    text: "Promote the remaining handoff guidance.".to_string(),
                },
            ],
            status: AppTurnStatus::Completed,
            error: None,
        }];

        let mut documents = repo_context_documents(&temp_root);
        documents.extend(thread_context_documents(
            &thread,
            true,
            Some(temp_root.as_path()),
        ));

        let promotion_ref_ids = handoff_promotion_ref_ids(
            &documents,
            &[
                "ctx:file:.codex/context/overview.md".to_string(),
                "ctx:thread-insight:thread-1:plan-1".to_string(),
                "ctx:thread-insight:thread-1:plan-2".to_string(),
            ],
        );

        assert_eq!(
            promotion_ref_ids,
            vec!["ctx:thread-insight:thread-1:plan-2".to_string()]
        );

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn plan_memory_promote_returns_only_unpersisted_thread_nodes() {
        let temp_root = temp_test_dir("memory-promote");
        let context_dir = temp_root.join(".codex").join("context");
        std::fs::create_dir_all(&context_dir).expect("create context dir");
        std::fs::write(
            context_dir.join("overview.md"),
            "---\ntitle: Planning Overview\nkind: concept\nsource_threads:\n  - \"thread-1\"\nsource_refs:\n  - \"ctx:thread-insight:thread-1:plan-1\"\n---\n# Planning Overview\n\nPersisted already.\n",
        )
        .expect("write repo context");

        let mut thread = sample_thread("thread-1", Some("planning sync"), Some("main"));
        thread.turns = vec![Turn {
            id: "turn-1".to_string(),
            items: vec![
                ThreadItem::Plan {
                    id: "plan-1".to_string(),
                    text: "Persisted already.".to_string(),
                },
                ThreadItem::Plan {
                    id: "plan-2".to_string(),
                    text: "Promote this remaining insight.".to_string(),
                },
            ],
            status: AppTurnStatus::Completed,
            error: None,
        }];

        let mut documents = repo_context_documents(&temp_root);
        documents.extend(thread_context_documents(
            &thread,
            true,
            Some(temp_root.as_path()),
        ));

        let plan = plan_memory_promote(
            &documents,
            &[
                "ctx:file:.codex/context/overview.md".to_string(),
                "ctx:thread-insight:thread-1:plan-1".to_string(),
                "ctx:thread-insight:thread-1:plan-2".to_string(),
            ],
        );

        assert_eq!(
            plan,
            MemoryPromotePlan {
                promotable_node_ids: vec!["ctx:thread-insight:thread-1:plan-2".to_string()],
                already_covered_node_ids: vec![
                    "ctx:file:.codex/context/overview.md".to_string(),
                    "ctx:thread-insight:thread-1:plan-1".to_string(),
                ],
            }
        );

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn context_query_focus_thread_ids_include_precursor_and_seed_threads() {
        let thread_ids = context_focus_thread_ids_for_query(&ContextQueryParams {
            current_thread_id: Some("thread-current".to_string()),
            precursor_thread_id: Some("thread-prev".to_string()),
            precursor_kind: Some(ContextPrecursorKind::Handoff),
            actor_id: None,
            repo_root: None,
            git_branch: None,
            goal: None,
            query: None,
            seed_ref_ids: vec![
                "ctx:thread-insight:thread-prev:plan-1".to_string(),
                "ctx:thread-insight:thread-other:plan-2".to_string(),
            ],
            limit: None,
        });

        assert_eq!(
            thread_ids,
            vec![
                "thread-current".to_string(),
                "thread-prev".to_string(),
                "thread-other".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn rollout_fallback_populates_local_context_query_for_current_thread() {
        let codex_home = temp_test_dir("context-rollout-home");
        let repo_root = temp_test_dir("context-rollout-repo");
        std::fs::create_dir_all(&repo_root).expect("create repo root");
        let thread_id = ThreadId::new().to_string();
        write_rollout_with_web_search(codex_home.as_path(), thread_id.as_str());

        let thread = load_thread_from_rollout_at(
            codex_home.as_path(),
            thread_id.as_str(),
            Some(repo_root.as_path()),
        )
        .await
        .expect("load rollout thread")
        .expect("thread should exist");
        let response = build_context_query(
            thread_context_documents(&thread, true, Some(repo_root.as_path())),
            &ContextQueryParams {
                current_thread_id: Some(thread_id.clone()),
                precursor_thread_id: None,
                precursor_kind: None,
                actor_id: None,
                repo_root: Some(repo_root.display().to_string()),
                git_branch: None,
                goal: None,
                query: None,
                seed_ref_ids: Vec::new(),
                limit: None,
            },
        );

        assert_eq!(
            response.nodes,
            vec![ContextQueryNode::Thread(ContextThreadNode {
                node_id: format!("ctx:thread-search:{thread_id}:web-1"),
                artifact_kind: ThreadArtifactKind::Search,
                title: "cloud ethics".to_string(),
                summary: Some("thread search result · retained web search".to_string()),
                location: Some("web/search".to_string()),
                body: Some("Query: cloud ethics".to_string()),
                origin_thread_id: thread_id.clone(),
                source_files: Vec::new(),
                source_refs: Vec::new(),
                created_at: None,
            })]
        );
        assert_eq!(
            response.edges,
            vec![ContextQueryEdge {
                from_node_id: format!("anchor:{thread_id}"),
                to_node_id: format!("ctx:thread-search:{thread_id}:web-1"),
                edge_type: ContextEdgeType::Mounted,
                mount_reason: Some(ContextMountReason::Local),
                reason: None,
            }]
        );

        let _ = std::fs::remove_dir_all(codex_home);
        let _ = std::fs::remove_dir_all(repo_root);
    }

    #[test]
    fn thread_summary_from_app_thread_uses_agent_nickname_and_preview() {
        let mut thread = sample_thread("thread-1", Some("planning sync"), Some("main"));
        thread.name = Some("Context thread".to_string());

        let summary = thread_summary_from_app_thread(thread);

        assert_eq!(summary.thread_id, "thread-1".to_string());
        assert_eq!(summary.actor_id, Some("lobster-worker".to_string()));
        assert_eq!(summary.title, Some("Context thread".to_string()));
        assert_eq!(summary.preview, Some("planning sync".to_string()));
        assert_eq!(summary.repo_root, Some("/tmp/repo".to_string()));
        assert_eq!(summary.cwd, Some("/tmp/repo".to_string()));
        assert_eq!(summary.git_branch, Some("main".to_string()));
        assert_eq!(summary.goal, None);
        assert_eq!(summary.precursor_thread_id, None);
        assert_eq!(summary.precursor_kind, None);
        assert_eq!(summary.updated_at, Some(1_741_422_760));
    }

    #[test]
    fn thread_context_documents_keep_retained_artifacts_including_web_queries() {
        let mut thread = sample_thread("thread-1", Some("planning sync"), Some("main"));
        let repo_root = PathBuf::from("/tmp/repo");
        thread.turns = vec![Turn {
            id: "turn-1".to_string(),
            items: vec![
                ThreadItem::Plan {
                    id: "plan-1".to_string(),
                    text: "Capture the current branch context.".to_string(),
                },
                ThreadItem::CommandExecution {
                    id: "cmd-1".to_string(),
                    command: "rg context src".to_string(),
                    cwd: PathBuf::from("/tmp/repo"),
                    process_id: None,
                    status: CommandExecutionStatus::Completed,
                    command_actions: vec![
                        CommandAction::Read {
                            command: "cat src/lib.rs".to_string(),
                            name: "cat".to_string(),
                            path: PathBuf::from("/tmp/repo/src/lib.rs"),
                        },
                        CommandAction::Search {
                            command: "rg context src".to_string(),
                            query: Some("context".to_string()),
                            path: Some("src".to_string()),
                        },
                    ],
                    aggregated_output: Some(
                        "src/lib.rs\nCurrent thread context\nlinked persistent knowledge\n"
                            .to_string(),
                    ),
                    exit_code: Some(0),
                    duration_ms: Some(8),
                },
                ThreadItem::WebSearch {
                    id: "web-1".to_string(),
                    query: "what is a context graph".to_string(),
                    action: None,
                },
            ],
            status: AppTurnStatus::Completed,
            error: None,
        }];

        let documents = thread_context_documents(&thread, true, Some(repo_root.as_path()));

        assert_eq!(
            documents
                .iter()
                .map(|document| (document.kind, document.title.clone()))
                .collect::<Vec<_>>(),
            vec![
                (
                    ContextKind::SharedThread,
                    "🦞 lobster-worker · Current Thread".to_string()
                ),
                (
                    ContextKind::ThreadInsight,
                    "Capture the current branch context.".to_string(),
                ),
                (ContextKind::ThreadFile, "src/lib.rs".to_string()),
                (
                    ContextKind::ThreadSearch,
                    "Search results in src".to_string()
                ),
                (
                    ContextKind::ThreadSearch,
                    "what is a context graph".to_string()
                ),
            ]
        );
        let web_search_document = documents
            .iter()
            .find(|document| document.title == "what is a context graph")
            .expect("retained web search document");
        assert_eq!(
            web_search_document.summary.as_deref(),
            Some("thread search result · retained web search")
        );
        assert_eq!(web_search_document.location.as_deref(), Some("web/search"));
        assert_eq!(
            web_search_document.body.as_deref(),
            Some("Query: what is a context graph")
        );
    }

    #[test]
    fn search_context_documents_does_not_match_thread_anchor_from_prompt_preview() {
        let temp_root = temp_test_dir("context-search-anchor-filter");
        let context_dir = temp_root.join(".codex").join("context");
        std::fs::create_dir_all(&context_dir).expect("create context dir");
        std::fs::write(
            context_dir.join("planning.md"),
            "---\ntitle: Planning Overview\nkind: note\n---\nCapture the collaboration rewrite milestones.\n",
        )
        .expect("write repo context");

        let mut documents = repo_context_documents(&temp_root);
        documents.push(thread_context_document(sample_thread(
            "thread-1",
            Some("planning sync"),
            None,
        )));

        let results = search_context_documents(documents, Some("planning"), 10, None);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, ContextKind::RepoContextFile);
        assert_eq!(results[0].title, "Planning Overview");

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn search_context_documents_matches_thread_git_metadata() {
        let documents = vec![thread_context_document(sample_thread(
            "thread-1",
            Some("planning sync"),
            Some("rewrite-codex-2gether-v2"),
        ))];

        let results =
            search_context_documents(documents, Some("rewrite-codex-2gether-v2"), 10, None);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, ContextKind::SharedThread);
        assert_eq!(results[0].location.as_deref(), Some("thread/thread-1"));
        assert!(
            results[0]
                .body
                .as_deref()
                .is_some_and(|body| body.contains("Git branch: rewrite-codex-2gether-v2"))
        );
    }

    #[test]
    fn search_context_documents_prioritizes_current_thread_component() {
        let temp_root = temp_test_dir("context-search-current-thread");
        let context_dir = temp_root.join(".codex").join("context");
        std::fs::create_dir_all(&context_dir).expect("create context dir");
        std::fs::write(
            context_dir.join("overview.md"),
            "---\ntitle: Planning Overview\nkind: concept\napplies_to:\n  branches:\n    - \"rewrite-codex-2gether-v2\"\nsource_threads:\n  - \"thread-1\"\n---\n# Planning Overview\n\nShip the context browser first.\n",
        )
        .expect("write planning context");
        std::fs::write(
            context_dir.join("archive.md"),
            "---\ntitle: Archive Notes\nkind: concept\n---\nOld planning scratchpad.\n",
        )
        .expect("write archive context");

        let mut documents = repo_context_documents(&temp_root);
        documents.push(thread_context_document(sample_thread(
            "thread-1",
            Some("planning sync"),
            Some("rewrite-codex-2gether-v2"),
        )));

        let results = search_context_documents(documents, None, 10, Some("thread-1"));

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].kind, ContextKind::SharedThread);
        assert_eq!(results[0].location.as_deref(), Some("thread/thread-1"));
        assert_eq!(results[1].kind, ContextKind::RepoContextFile);
        assert_eq!(results[1].title, "Planning Overview");
        assert_eq!(results[2].title, "Archive Notes");

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn render_context_bundle_prefers_discovery_pack_over_full_body() {
        let mut thread = sample_thread("thread-1", Some("planning sync"), Some("main"));
        thread.turns = vec![Turn {
            id: "turn-1".to_string(),
            items: vec![ThreadItem::Plan {
                id: "plan-1".to_string(),
                text: "Discovery pack rollout\nThis plan body should not be copied wholesale into the bundle.".to_string(),
            }],
            status: AppTurnStatus::Completed,
            error: None,
        }];
        let documents = thread_context_documents(&thread, true, None);
        let entries = documents
            .iter()
            .filter(|document| matches!(document.kind, ContextKind::ThreadInsight))
            .cloned()
            .map(resolved_entry_from_document)
            .collect::<Vec<_>>();
        let artifact_paths = context_graph_artifact_paths(&documents);

        let bundle = render_context_bundle(&documents, &entries, Some(&artifact_paths));

        assert!(bundle.contains("Graph index: .codex/context/.graph/index.md"));
        assert!(bundle.contains("Hotspots to inspect or promote:"));
        assert!(bundle.contains(".codex/context/.graph/nodes/insights/"));
        assert!(bundle.contains("Discovery pack rollout"));
        assert!(!bundle.contains("This plan body should not be copied wholesale into the bundle."));
    }

    fn sample_thread(id: &str, preview: Option<&str>, branch: Option<&str>) -> Thread {
        Thread {
            id: id.to_string(),
            preview: preview.unwrap_or_default().to_string(),
            ephemeral: false,
            model_provider: "openai".to_string(),
            created_at: 1_741_422_400,
            updated_at: 1_741_422_760,
            status: ThreadStatus::NotLoaded,
            path: None,
            cwd: PathBuf::from("/tmp/repo"),
            cli_version: "0.0.0-test".to_string(),
            source: SessionSource::Cli,
            agent_nickname: Some("lobster-worker".to_string()),
            agent_role: Some("research".to_string()),
            git_info: Some(GitInfo {
                sha: Some("abc123def456".to_string()),
                branch: branch.map(str::to_string),
                origin_url: Some("git@github.com:openai/codex-together.git".to_string()),
            }),
            name: None,
            turns: Vec::new(),
        }
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", ThreadId::new()));
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        dir
    }

    fn write_rollout_with_web_search(codex_home: &Path, thread_id: &str) {
        let filename_ts = "2026-03-18T12-00-00";
        let meta_ts = "2026-03-18T12:00:00Z";
        let dir = codex_home
            .join("sessions")
            .join("2026")
            .join("03")
            .join("18");
        std::fs::create_dir_all(&dir).expect("create sessions dir");

        let session_meta = codex_protocol::protocol::SessionMetaLine {
            meta: codex_protocol::protocol::SessionMeta {
                id: ThreadId::from_string(thread_id).expect("valid thread id"),
                forked_from_id: None,
                timestamp: meta_ts.to_string(),
                cwd: PathBuf::from("/tmp/repo"),
                originator: "codex".to_string(),
                cli_version: "0.0.0".to_string(),
                source: codex_protocol::protocol::SessionSource::Cli,
                agent_nickname: None,
                agent_role: None,
                model_provider: None,
                base_instructions: None,
                dynamic_tools: None,
            },
            git: None,
        };
        let lines = [
            codex_protocol::protocol::RolloutLine {
                timestamp: meta_ts.to_string(),
                item: codex_protocol::protocol::RolloutItem::SessionMeta(session_meta),
            },
            codex_protocol::protocol::RolloutLine {
                timestamp: meta_ts.to_string(),
                item: codex_protocol::protocol::RolloutItem::EventMsg(
                    codex_protocol::protocol::EventMsg::WebSearchEnd(
                        codex_protocol::protocol::WebSearchEndEvent {
                            call_id: "web-1".to_string(),
                            query: "cloud ethics".to_string(),
                            action: codex_protocol::models::WebSearchAction::Other,
                        },
                    ),
                ),
            },
        ]
        .into_iter()
        .map(|line| serde_json::to_string(&line).expect("serialize rollout line"))
        .collect::<Vec<_>>()
        .join("\n");
        let rollout_path = dir.join(format!("rollout-{filename_ts}-{thread_id}.jsonl"));
        std::fs::write(rollout_path, format!("{lines}\n")).expect("write rollout");
    }
}
