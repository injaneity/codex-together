use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

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
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadReadParams;
use codex_app_server_protocol::ThreadReadResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_core::config::find_codex_home;
use codex_core::git_info::current_branch_name;
use codex_core::git_info::get_git_repo_root;
use codex_core::git_info::get_head_commit_hash;
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
use codex_together_protocol::METHOD_SESSION_JOIN;
use codex_together_protocol::METHOD_SESSION_LEAVE;
use codex_together_protocol::METHOD_TOGETHER_AUTH;
use codex_together_protocol::NOTIFY_HOST_STOPPED;
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
const CONTEXT_MAX_LIMIT: u32 = 200;
const CONTEXT_BODY_CHAR_LIMIT: usize = 4_000;

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

#[derive(Debug, Clone)]
struct ContextDocument {
    ref_id: String,
    kind: ContextKind,
    title: String,
    summary: Option<String>,
    location: Option<String>,
    body: Option<String>,
    search_text: String,
    graph: ContextDocumentGraphMetadata,
}

#[derive(Debug, Clone, Default)]
struct ContextDocumentGraphMetadata {
    branches: Vec<String>,
    source_threads: Vec<String>,
    source_files: Vec<String>,
}

#[derive(Debug, Default)]
struct RepoContextMetadata {
    id: Option<String>,
    title: Option<String>,
    kind: Option<String>,
    branches: Vec<String>,
    source_threads: Vec<String>,
    source_files: Vec<String>,
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
        METHOD_CONTEXT_PREVIEW => context_preview(state, ctx, req).await,
        METHOD_CONTEXT_RESOLVE_BUNDLE => context_resolve_bundle(state, ctx, req).await,
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

    let mut written_files = Vec::with_capacity(pending.files.len());
    for file in pending.files {
        let path = repo_root.join(&file.relative_path);
        if let Some(parent) = path.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            return rpc_error(
                req.id,
                -32603,
                format!("failed to create {}: {err}", parent.display()),
            );
        }
        if let Err(err) = std::fs::write(&path, file.content) {
            return rpc_error(
                req.id,
                -32603,
                format!("failed to write {}: {err}", path.display()),
            );
        }
        written_files.push(file.relative_path);
    }

    JsonRpcResponse::ok(req.id, ContextWriteCommitResponse { written_files })
        .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
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

    let mut kept_entries = vec![source_entry];
    kept_entries.extend(selected_context_entries(
        documents,
        &payload.selected_ref_ids,
    ));
    dedupe_context_entries(&mut kept_entries);

    let kept_refs = kept_entries
        .iter()
        .map(|entry| entry.context_ref.clone())
        .collect::<Vec<_>>();
    let token_estimate = estimate_context_bundle_tokens(&kept_entries);
    let plan_id = Uuid::new_v4().to_string();
    let goal = payload.goal.filter(|value| !value.trim().is_empty());

    {
        let mut guard = state.inner.lock().await;
        guard.handoff_plans.insert(
            plan_id.clone(),
            PendingHandoffPlan {
                source_thread_id: source_thread_id.clone(),
            },
        );
    }

    JsonRpcResponse::ok(
        req.id,
        HandoffPlanResponse {
            plan_id,
            source_thread_id,
            goal,
            selected_node_ids: kept_refs.iter().map(|entry| entry.ref_id.clone()).collect(),
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

impl ContextDocument {
    fn into_search_result(self) -> ContextSearchResult {
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

#[derive(Debug, Clone)]
struct ResolvedContextEntry {
    context_ref: ContextRef,
    bundle_text: String,
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
    search_context_documents(documents, query, limit, current_thread_id)
}

fn build_context_graph(
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

    let mut documents = repo_context_documents(repo_root.as_path());
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
    context_thread_id_from_ref_id(ref_id).into_iter().collect()
}

fn context_focus_thread_ids_for_ref_ids(ref_ids: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    ref_ids
        .iter()
        .filter_map(|ref_id| context_thread_id_from_ref_id(ref_id))
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

fn context_thread_id_from_ref_id(ref_id: &str) -> Option<String> {
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

fn search_context_documents(
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

fn repo_context_documents(repo_root: &Path) -> Vec<ContextDocument> {
    let context_root = repo_root.join(".codex").join("context");
    let mut markdown_files = Vec::new();
    collect_markdown_files(context_root.as_path(), &mut markdown_files);
    markdown_files.sort();

    markdown_files
        .into_iter()
        .filter_map(|path| repo_context_document(repo_root, path.as_path()))
        .collect()
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
        "{}\n{}\n{}\n{}\n{}",
        title,
        summary.clone().unwrap_or_default(),
        location,
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
            || line.starts_with("- shared thread:")
    )
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

        let thread = match thread_read_with_turn_fallback(&mut bridge, &thread_id).await {
            Ok(response) => response.thread,
            Err(err) => {
                warn!(error = ?err, thread_id, "failed to read focused thread for context");
                continue;
            }
        };
        if thread.ephemeral || repo_root.is_some_and(|root| !thread.cwd.starts_with(root)) {
            continue;
        }

        let is_current_thread = current_thread_id.as_deref() == Some(thread.id.as_str());
        documents.extend(thread_context_documents(
            &thread,
            is_current_thread,
            repo_root,
        ));
    }

    documents.sort_by_key(context_default_sort_key);
    documents
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

    ContextResolveBundleResponse {
        bundle_text: render_context_bundle(&kept_entries),
        kept_refs,
        dropped_refs,
    }
}

fn selected_context_entries(
    documents: Vec<ContextDocument>,
    selected_ref_ids: &[String],
) -> Vec<ResolvedContextEntry> {
    if selected_ref_ids.is_empty() {
        return Vec::new();
    }

    let selected = selected_ref_ids.iter().cloned().collect::<HashSet<_>>();
    documents
        .into_iter()
        .filter(|document| selected.contains(&document.ref_id))
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

fn estimate_context_bundle_tokens(entries: &[ResolvedContextEntry]) -> u32 {
    let chars = entries
        .iter()
        .map(|entry| entry.bundle_text.chars().count())
        .sum::<usize>();
    ((chars / 4).max(1)).try_into().unwrap_or(u32::MAX)
}

fn resolved_entry_from_document(document: ContextDocument) -> ResolvedContextEntry {
    let context_ref = context_ref_from_document(&document);
    let bundle_text = document_bundle_text(&document);
    ResolvedContextEntry {
        context_ref,
        bundle_text,
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
    if let Some(body) = &document.body
        && !body.trim().is_empty()
    {
        lines.push(String::new());
        lines.push(body.trim().to_string());
    }
    lines.join("\n")
}

fn render_context_bundle(entries: &[ResolvedContextEntry]) -> String {
    entries
        .iter()
        .map(|entry| entry.bundle_text.clone())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn thread_context_document(thread: codex_app_server_protocol::Thread) -> ContextDocument {
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

fn thread_context_documents(
    thread: &codex_app_server_protocol::Thread,
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

fn thread_context_title(
    thread: &codex_app_server_protocol::Thread,
    is_current_thread: bool,
) -> String {
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

async fn source_thread_context_entry(
    bridge: &mut AppServerBridge,
    thread_id: &str,
) -> Result<ResolvedContextEntry, AppServerError> {
    let thread_read = thread_read_with_turn_fallback(bridge, thread_id).await?;
    Ok(resolved_entry_from_document(thread_context_document(
        thread_read.thread,
    )))
}

async fn thread_read_with_turn_fallback(
    bridge: &mut AppServerBridge,
    thread_id: &str,
) -> Result<ThreadReadResponse, AppServerError> {
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

fn thread_context_summary(thread: &codex_app_server_protocol::Thread) -> String {
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
    thread: &codex_app_server_protocol::Thread,
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
    thread: &codex_app_server_protocol::Thread,
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

fn thread_artifact_documents(
    thread: &codex_app_server_protocol::Thread,
    repo_root: Option<&Path>,
) -> Vec<ContextDocument> {
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
                    codex_app_server_protocol::PatchApplyStatus::Failed
                        | codex_app_server_protocol::PatchApplyStatus::Declined
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
                    status,
                    command_actions,
                    aggregated_output,
                    ..
                } if matches!(
                    status,
                    codex_app_server_protocol::CommandExecutionStatus::Completed
                ) =>
                {
                    let output_excerpt = aggregated_output
                        .as_deref()
                        .and_then(|output| context_excerpt(output, 14, 1_600));
                    for action in command_actions {
                        match action {
                            codex_app_server_protocol::CommandAction::Read { path, .. } => {
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
                            codex_app_server_protocol::CommandAction::Search { path, .. } => {
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
                            codex_app_server_protocol::CommandAction::ListFiles { .. }
                            | codex_app_server_protocol::CommandAction::Unknown { .. } => {}
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
                } if matches!(
                    status,
                    codex_app_server_protocol::DynamicToolCallStatus::Completed
                ) && success.unwrap_or(false) =>
                {
                    let body = content_items.as_ref().and_then(|items| {
                        context_excerpt(
                            &items
                                .iter()
                                .filter_map(|item| match item {
                                    codex_app_server_protocol::DynamicToolCallOutputContentItem::InputText { text } => {
                                        non_empty_string(text.clone())
                                    }
                                    codex_app_server_protocol::DynamicToolCallOutputContentItem::InputImage {
                                        image_url,
                                    } => Some(format!("[image] {image_url}")),
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
                    status,
                    result,
                    ..
                } if matches!(
                    status,
                    codex_app_server_protocol::McpToolCallStatus::Completed
                ) =>
                {
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
                ThreadItem::CommandExecution { .. }
                | ThreadItem::FileChange { .. }
                | ThreadItem::McpToolCall { .. }
                | ThreadItem::DynamicToolCall { .. } => {}
                ThreadItem::UserMessage { .. }
                | ThreadItem::AgentMessage { .. }
                | ThreadItem::Reasoning { .. }
                | ThreadItem::CollabAgentToolCall { .. }
                | ThreadItem::WebSearch { .. }
                | ThreadItem::EnteredReviewMode { .. }
                | ThreadItem::ExitedReviewMode { .. }
                | ThreadItem::ContextCompaction { .. } => {}
            }
        }
    }

    let mut documents = documents_by_ref_id.into_values().collect::<Vec<_>>();
    documents.sort_by_key(context_default_sort_key);
    documents
}

fn upsert_thread_file_document(
    documents_by_ref_id: &mut HashMap<String, ContextDocument>,
    thread: &codex_app_server_protocol::Thread,
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
    let content = render_context_write_file(
        ContextWriteMetadata {
            id,
            kind: kind.clone(),
            title: title.clone(),
            branch,
            source_threads,
            source_files,
            last_validated_at: Utc::now().format("%Y-%m-%d").to_string(),
        },
        context_write_body(document, existing_content.as_deref()),
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

fn context_write_body(document: &ContextDocument, existing_content: Option<&str>) -> String {
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
    ) -> Result<ThreadReadResponse, AppServerError> {
        self.request_with_retry(
            "thread/read",
            serde_json::to_value(ThreadReadParams {
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

    async fn thread_start(
        &mut self,
        cwd: Option<String>,
        model: Option<String>,
        approval_policy: Option<codex_protocol::protocol::AskForApproval>,
        sandbox: Option<codex_protocol::protocol::SandboxPolicy>,
    ) -> Result<ThreadStartResponse, AppServerError> {
        let approval_policy = approval_policy.map(Into::into);
        let sandbox = sandbox.and_then(thread_start_sandbox_mode_from_policy);
        self.request_with_retry(
            "thread/start",
            serde_json::to_value(ThreadStartParams {
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
                persist_extended_history: false,
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
    use super::build_context_graph;
    use super::plan_context_write_files;
    use super::repo_context_documents;
    use super::search_context_documents;
    use super::thread_context_document;
    use super::thread_context_documents;
    use codex_app_server_protocol::CommandAction;
    use codex_app_server_protocol::CommandExecutionStatus;
    use codex_app_server_protocol::GitInfo;
    use codex_app_server_protocol::SessionSource;
    use codex_app_server_protocol::Thread;
    use codex_app_server_protocol::ThreadItem;
    use codex_app_server_protocol::ThreadStatus;
    use codex_app_server_protocol::Turn;
    use codex_app_server_protocol::TurnStatus as AppTurnStatus;
    use codex_protocol::ThreadId;
    use codex_together_protocol::ContextKind;
    use std::path::PathBuf;

    #[test]
    fn repo_context_documents_parse_frontmatter_and_body() {
        let temp_root = temp_test_dir("repo-context-docs");
        let context_dir = temp_root.join(".codex").join("context");
        std::fs::create_dir_all(&context_dir).expect("create context dir");
        std::fs::write(
            context_dir.join("overview.md"),
            "---\ntitle: Planning Notes\nkind: plan\napplies_to:\n  branches:\n    - \"rewrite-codex-2gether-v2\"\nsource_threads:\n  - \"thread-1\"\nsource_files:\n  - \".codex/context/reference.md\"\nvisibility: \"repo\"\n---\n# Planning Notes\n\nowner=zanechee@local · shared_by=zanechee@local · shared_at=2026-03-15T22:41:10.744997+00:00\nShip the context browser first.\nShared by: zanechee@local\n",
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
    fn thread_context_documents_keep_retained_artifacts_not_web_queries() {
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
            ]
        );
        assert!(documents.iter().all(|document| {
            document.title != "what is a context graph"
                && document
                    .body
                    .as_deref()
                    .is_none_or(|body| !body.contains("what is a context graph"))
        }));
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
}
