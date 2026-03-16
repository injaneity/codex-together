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
use codex_app_server_protocol::Turn;
use codex_app_server_protocol::UserInput;
use codex_core::RolloutRecorder;
use codex_core::config::find_codex_home;
use codex_core::git_info::current_branch_name;
use codex_core::git_info::get_git_repo_root;
use codex_core::git_info::get_head_commit_hash;
use codex_protocol::ThreadId;
use codex_protocol::items::TurnItem;
use codex_state::StateRuntime;
use codex_state::TogetherClientMode as StateTogetherClientMode;
use codex_state::TogetherClientSession as StateTogetherClientSession;
use codex_state::TogetherMemberRecord;
use codex_state::TogetherRole as StateTogetherRole;
use codex_state::TogetherServerRecord;
use codex_state::TogetherThreadAclRecord;
use codex_together_protocol::ConnectedMember;
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
use codex_together_protocol::METHOD_THREAD_INSPECT;
use codex_together_protocol::METHOD_THREAD_LIST;
use codex_together_protocol::METHOD_THREAD_SHARE;
use codex_together_protocol::METHOD_TOGETHER_AUTH;
use codex_together_protocol::NOTIFY_HOST_STOPPED;
use codex_together_protocol::NOTIFY_TOGETHER_MEMBER_UPDATED;
use codex_together_protocol::NOTIFY_TOGETHER_THREAD_SHARED;
use codex_together_protocol::TogetherAuthRequest;
use codex_together_protocol::TogetherAuthResponse;
use codex_together_protocol::TogetherJoinRequest;
use codex_together_protocol::TogetherJoinResponse;
use codex_together_protocol::TogetherLeaveResponse;
use codex_together_protocol::TogetherReplayMessage;
use codex_together_protocol::TogetherReplayRole;
use codex_together_protocol::TogetherRole;
use codex_together_protocol::TogetherServerCreateRequest;
use codex_together_protocol::TogetherServerCreateResponse;
use codex_together_protocol::TogetherServerInfoResponse;
use codex_together_protocol::TogetherThreadListRequest;
use codex_together_protocol::TogetherThreadListResponse;
use codex_together_protocol::TogetherThreadReadRequest;
use codex_together_protocol::TogetherThreadReadResponse;
use codex_together_protocol::TogetherThreadShareRequest;
use codex_together_protocol::TogetherThreadShareResponse;
use codex_together_protocol::TogetherThreadSummary;
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
    threads: HashMap<String, SharedThread>,
}

#[derive(Debug, Clone)]
struct SharedThread {
    thread_id: String,
    owner_email: String,
    shared_by_email: String,
    preview: Option<String>,
    shared_at: String,
    history: Option<Vec<codex_protocol::protocol::RolloutItem>>,
    repo_root: Option<String>,
    git_branch: Option<String>,
    git_sha: Option<String>,
    git_origin_url: Option<String>,
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
}

#[derive(Debug, Default)]
struct RepoContextMetadata {
    id: Option<String>,
    title: Option<String>,
    kind: Option<String>,
    visibility: Option<String>,
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
        METHOD_THREAD_SHARE => together_thread_share(state, ctx, req).await,
        METHOD_THREAD_LIST => together_thread_list(state, ctx, req).await,
        METHOD_THREAD_INSPECT => together_thread_read(state, ctx, req).await,
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
            threads: HashMap::new(),
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
        .upsert_together_member(&TogetherMemberRecord {
            server_id: server_id.clone(),
            email: owner_email.clone(),
            role: StateTogetherRole::Owner,
            added_at: created_at_epoch,
            removed_at: None,
        })
        .await
    {
        error!(error = %err, "failed to persist together owner member");
        return rpc_error(req.id, -32603, "failed to persist together server");
    }

    if let Err(err) = state
        .state_db
        .upsert_together_client_session(&StateTogetherClientSession {
            mode: StateTogetherClientMode::Host,
            server_id: Some(server_id.clone()),
            owner_email: Some(owner_email.clone()),
            endpoint: Some(public_base_url.clone()),
            checked_out_thread_id: None,
            host_pid: Some(i64::from(std::process::id())),
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
            checked_out_thread_id: None,
            host_pid: None,
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

async fn together_thread_share(
    state: &AppState,
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: TogetherThreadShareRequest = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };
    let caller_email = ctx
        .email
        .clone()
        .unwrap_or_else(|| "guest@local".to_string());
    let visibility = match normalized_thread_visibility(payload.visibility.as_deref()) {
        Some(value) => value,
        None => return rpc_error(req.id, -32602, "visibility must be on|off|public|private"),
    };
    let shared_at = Utc::now().to_rfc3339();
    let shared_at_epoch = Utc::now().timestamp();

    if visibility == "private" {
        let (owner_email, preview, response_shared_at) = {
            let mut guard = state.inner.lock().await;
            let Some(hosted) = guard.hosted.as_mut() else {
                return rpc_error(req.id, RPC_ERR_NOT_CONNECTED, "TOGETHER_NOT_CONNECTED");
            };

            if member_role(hosted, &caller_email).is_none() {
                return rpc_error(
                    req.id,
                    RPC_ERR_MEMBER_NOT_ALLOWED,
                    "TOGETHER_MEMBER_NOT_ALLOWED",
                );
            }

            if let Some(existing) = hosted.threads.get(&payload.thread_id)
                && existing.owner_email != caller_email
            {
                return rpc_error(req.id, RPC_ERR_FORBIDDEN, "TOGETHER_FORBIDDEN");
            }

            match hosted.threads.remove(&payload.thread_id) {
                Some(existing) => (existing.owner_email, existing.preview, existing.shared_at),
                None => (caller_email.clone(), None, shared_at.clone()),
            }
        };

        return JsonRpcResponse::ok(
            req.id,
            TogetherThreadShareResponse {
                thread_id: payload.thread_id,
                owner_email,
                preview,
                shared_at: response_shared_at,
                visibility: Some(visibility),
            },
        )
        .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"));
    }

    let has_existing_shared = {
        let guard = state.inner.lock().await;
        let Some(hosted) = guard.hosted.as_ref() else {
            return rpc_error(req.id, RPC_ERR_NOT_CONNECTED, "TOGETHER_NOT_CONNECTED");
        };

        if member_role(hosted, &caller_email).is_none() {
            return rpc_error(
                req.id,
                RPC_ERR_MEMBER_NOT_ALLOWED,
                "TOGETHER_MEMBER_NOT_ALLOWED",
            );
        }

        if let Some(existing) = hosted.threads.get(&payload.thread_id)
            && existing.owner_email != caller_email
        {
            return rpc_error(req.id, RPC_ERR_FORBIDDEN, "TOGETHER_FORBIDDEN");
        }

        hosted.threads.contains_key(&payload.thread_id)
    };

    let (preview, shared_history) = if let Some(history) = payload.history.clone() {
        if history.is_empty() {
            return rpc_error(
                req.id,
                -32602,
                "cannot share thread: no persisted turns yet; send at least one message first",
            );
        }
        let shared_history = canonicalize_shared_history(&payload.thread_id, history);
        (
            rollout_history_preview(shared_history.as_slice()),
            Some(shared_history),
        )
    } else {
        let (thread, include_turns_available) = {
            let mut bridge = state.app_server.lock().await;
            match bridge.thread_read(payload.thread_id.clone(), true).await {
                Ok(response) => (response.thread, true),
                Err(err) => {
                    if app_server_thread_not_loaded(&err) {
                        match bridge.thread_read(payload.thread_id.clone(), false).await {
                            Ok(response) => (response.thread, false),
                            Err(fallback_err) if app_server_thread_not_loaded(&fallback_err) => {
                                let reason = if has_existing_shared {
                                    "thread is not loaded on this together host; unable to refresh preview from latest turns"
                                } else {
                                    "thread is not loaded on this together host; if this thread was forked locally, fork it via together first"
                                };
                                return rpc_error(req.id, -32602, reason);
                            }
                            Err(fallback_err) => {
                                return app_server_error_response(req.id, fallback_err);
                            }
                        }
                    } else {
                        return app_server_error_response(req.id, err);
                    }
                }
            }
        };

        if include_turns_available && thread.turns.is_empty() {
            return rpc_error(
                req.id,
                -32602,
                "cannot share thread: no persisted turns yet; send at least one message first",
            );
        }

        (
            non_empty_string(thread.preview),
            forkable_history_from_rollout(thread.path.as_ref()).await,
        )
    };

    let (server_id, owner_email) = {
        let mut guard = state.inner.lock().await;
        let (server_id, owner_email, thread_id, owner_email_for_note, shared_at_for_note) = {
            let Some(hosted) = guard.hosted.as_mut() else {
                return rpc_error(req.id, RPC_ERR_NOT_CONNECTED, "TOGETHER_NOT_CONNECTED");
            };

            if member_role(hosted, &caller_email).is_none() {
                return rpc_error(
                    req.id,
                    RPC_ERR_MEMBER_NOT_ALLOWED,
                    "TOGETHER_MEMBER_NOT_ALLOWED",
                );
            }

            if let Some(existing) = hosted.threads.get(&payload.thread_id)
                && existing.owner_email != caller_email
            {
                return rpc_error(req.id, RPC_ERR_FORBIDDEN, "TOGETHER_FORBIDDEN");
            }

            let owner_email = hosted
                .threads
                .get(&payload.thread_id)
                .map(|existing| existing.owner_email.clone())
                .unwrap_or_else(|| caller_email.clone());

            hosted.threads.insert(
                payload.thread_id.clone(),
                SharedThread {
                    thread_id: payload.thread_id.clone(),
                    owner_email: owner_email.clone(),
                    shared_by_email: caller_email.clone(),
                    preview: preview.clone(),
                    shared_at: shared_at.clone(),
                    history: shared_history.clone(),
                    repo_root: payload.repo_root.clone(),
                    git_branch: payload.git_branch.clone(),
                    git_sha: payload.git_sha.clone(),
                    git_origin_url: payload.git_origin_url.clone(),
                },
            );

            (
                hosted.server_id.clone(),
                owner_email.clone(),
                payload.thread_id.clone(),
                owner_email,
                shared_at.clone(),
            )
        };
        broadcast_notification(
            &guard,
            NOTIFY_TOGETHER_THREAD_SHARED,
            serde_json::json!({
                "threadId": thread_id,
                "ownerEmail": owner_email_for_note,
                "sharedAt": shared_at_for_note,
            }),
        );

        (server_id, owner_email)
    };

    if let Err(err) = state
        .state_db
        .upsert_together_thread_acl(&TogetherThreadAclRecord {
            server_id,
            thread_id: payload.thread_id.clone(),
            owner_email: owner_email.clone(),
            shared_by_email: caller_email,
            shared_at: shared_at_epoch,
        })
        .await
    {
        error!(error = %err, "failed to persist shared thread ACL");
        return rpc_error(req.id, -32603, "failed to persist shared thread");
    }

    JsonRpcResponse::ok(
        req.id,
        TogetherThreadShareResponse {
            thread_id: payload.thread_id,
            owner_email,
            preview,
            shared_at,
            visibility: Some(visibility),
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn together_thread_read(
    state: &AppState,
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: TogetherThreadReadRequest = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };
    let email = ctx
        .email
        .clone()
        .unwrap_or_else(|| "guest@local".to_string());

    let maybe_snapshot = {
        let guard = state.inner.lock().await;
        let Some(hosted) = guard.hosted.as_ref() else {
            return rpc_error(req.id, RPC_ERR_NOT_CONNECTED, "TOGETHER_NOT_CONNECTED");
        };
        if member_role(hosted, &email).is_none() {
            return rpc_error(
                req.id,
                RPC_ERR_MEMBER_NOT_ALLOWED,
                "TOGETHER_MEMBER_NOT_ALLOWED",
            );
        }

        let Some(thread) = hosted.threads.get(&payload.thread_id) else {
            return rpc_error(req.id, -32602, "thread not shared");
        };
        (thread.owner_email.clone(), thread.history.clone())
    };

    let (owner_email, snapshot_history) = maybe_snapshot;

    if let Some(history) = snapshot_history {
        return JsonRpcResponse::ok(
            req.id,
            TogetherThreadReadResponse {
                thread_id: payload.thread_id,
                owner_email,
                history: Some(history.clone()),
                messages: replay_messages_from_history(history.as_slice()),
            },
        )
        .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"));
    }

    let thread_read = {
        let mut bridge = state.app_server.lock().await;
        match thread_read_with_turn_fallback(&mut bridge, payload.thread_id.as_str()).await {
            Ok(response) => response,
            Err(err) => return app_server_error_response(req.id, err),
        }
    };

    JsonRpcResponse::ok(
        req.id,
        TogetherThreadReadResponse {
            thread_id: payload.thread_id,
            owner_email,
            history: forkable_history_from_rollout(thread_read.thread.path.as_ref()).await,
            messages: replay_messages_from_turns(&thread_read.thread.turns),
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn together_thread_list(
    state: &AppState,
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: TogetherThreadListRequest =
        serde_json::from_value(req.params).unwrap_or(TogetherThreadListRequest {
            cursor: None,
            limit: Some(20),
            search_term: None,
        });

    let email = ctx
        .email
        .clone()
        .unwrap_or_else(|| "guest@local".to_string());

    let guard = state.inner.lock().await;
    let Some(hosted) = guard.hosted.as_ref() else {
        return rpc_error(req.id, RPC_ERR_NOT_CONNECTED, "TOGETHER_NOT_CONNECTED");
    };
    if member_role(hosted, &email).is_none() {
        return rpc_error(
            req.id,
            RPC_ERR_MEMBER_NOT_ALLOWED,
            "TOGETHER_MEMBER_NOT_ALLOWED",
        );
    }

    let mut rows: Vec<TogetherThreadSummary> = hosted
        .threads
        .values()
        .map(|thread| TogetherThreadSummary {
            thread_id: thread.thread_id.clone(),
            owner_email: thread.owner_email.clone(),
            preview: thread.preview.clone(),
            created_at: thread.shared_at.clone(),
            repo_root: thread.repo_root.clone(),
            git_branch: thread.git_branch.clone(),
            git_sha: thread.git_sha.clone(),
            git_origin_url: thread.git_origin_url.clone(),
        })
        .collect();

    rows.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    if let Some(term) = payload.search_term {
        let term_lower = term.to_lowercase();
        rows.retain(|row| {
            row.thread_id.to_lowercase().contains(&term_lower)
                || row.owner_email.to_lowercase().contains(&term_lower)
                || row
                    .preview
                    .as_ref()
                    .map(|p| p.to_lowercase().contains(&term_lower))
                    .unwrap_or(false)
                || row
                    .repo_root
                    .as_ref()
                    .map(|value| value.to_lowercase().contains(&term_lower))
                    .unwrap_or(false)
                || row
                    .git_branch
                    .as_ref()
                    .map(|value| value.to_lowercase().contains(&term_lower))
                    .unwrap_or(false)
                || row
                    .git_sha
                    .as_ref()
                    .map(|value| value.to_lowercase().contains(&term_lower))
                    .unwrap_or(false)
                || row
                    .git_origin_url
                    .as_ref()
                    .map(|value| value.to_lowercase().contains(&term_lower))
                    .unwrap_or(false)
        });
    }

    let limit = payload.limit.unwrap_or(20).max(1) as usize;
    if rows.len() > limit {
        rows.truncate(limit);
    }

    JsonRpcResponse::ok(
        req.id,
        TogetherThreadListResponse {
            data: rows,
            next_cursor: None,
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn context_search(
    state: &AppState,
    ctx: &ConnectionContext,
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
                ctx,
                payload.query.as_deref(),
                payload.limit.unwrap_or(CONTEXT_DEFAULT_LIMIT),
            )
            .await,
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn context_graph(
    state: &AppState,
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: ContextGraphParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    JsonRpcResponse::ok(
        req.id,
        ContextGraphResponse {
            nodes: context_search_results(
                state,
                ctx,
                payload.query.as_deref(),
                payload.limit.unwrap_or(CONTEXT_DEFAULT_LIMIT),
            )
            .await,
            edges: Vec::new(),
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn context_preview(
    state: &AppState,
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: ContextPreviewParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    let item = context_documents(state, ctx)
        .await
        .into_iter()
        .find(|document| document.ref_id == payload.ref_id)
        .map(ContextDocument::into_search_result);

    JsonRpcResponse::ok(req.id, ContextPreviewResponse { item })
        .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn context_resolve_bundle(
    state: &AppState,
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: ContextResolveBundleParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    let response = build_context_bundle(state, ctx, payload.context_refs).await;
    JsonRpcResponse::ok(req.id, response)
        .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn context_write_plan(
    state: &AppState,
    ctx: &ConnectionContext,
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

    let documents = context_documents(state, ctx).await;
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
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: HandoffPlanParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    let Some(source_thread_id) = payload.source_thread_id else {
        return rpc_error(req.id, -32602, "sourceThreadId is required");
    };

    let documents = context_documents(state, ctx).await;
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
    let now = Utc::now().timestamp();

    let (server_id, owner_email, endpoint, role, newly_added_member) = {
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
        let newly_added_member = if matches!(role, TogetherRole::Member) {
            hosted.members.insert(email.clone())
        } else {
            false
        };

        (
            hosted.server_id.clone(),
            hosted.owner_email.clone(),
            hosted.public_base_url.clone(),
            role,
            newly_added_member,
        )
    };

    if newly_added_member
        && let Err(err) = state
            .state_db
            .upsert_together_member(&TogetherMemberRecord {
                server_id: server_id.clone(),
                email: email.clone(),
                role: StateTogetherRole::Member,
                added_at: now,
                removed_at: None,
            })
            .await
    {
        error!(error = %err, "failed to persist together joined member");
        return rpc_error(req.id, -32603, "failed to persist member join");
    }

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

    let server_id = {
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

        let server_id = hosted.server_id.clone();
        broadcast_notification(
            &guard,
            NOTIFY_TOGETHER_MEMBER_UPDATED,
            serde_json::json!({
                "email": leaving_email.clone(),
                "added": false,
            }),
        );
        server_id
    };

    let now = Utc::now().timestamp();
    if let Err(err) = state
        .state_db
        .upsert_together_member(&TogetherMemberRecord {
            server_id,
            email: leaving_email,
            role: StateTogetherRole::Member,
            added_at: now,
            removed_at: Some(now),
        })
        .await
    {
        error!(error = %err, "failed to persist together leave update");
        return rpc_error(req.id, -32603, "failed to persist member leave");
    }

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

fn normalized_thread_visibility(value: Option<&str>) -> Option<String> {
    match value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("on") | Some("public") => Some("public".to_string()),
        Some("off") | Some("private") => Some("private".to_string()),
        Some(_) => None,
    }
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
    ctx: &ConnectionContext,
    query: Option<&str>,
    limit: u32,
) -> Vec<ContextSearchResult> {
    let documents = context_documents(state, ctx).await;
    search_context_documents(documents, query, limit)
}

async fn context_documents(state: &AppState, ctx: &ConnectionContext) -> Vec<ContextDocument> {
    let email = ctx
        .email
        .clone()
        .unwrap_or_else(|| "guest@local".to_string());
    let hosted = {
        let guard = state.inner.lock().await;
        guard.hosted.clone()
    };

    let repo_root = match resolve_context_root() {
        Ok(path) => path,
        Err(err) => {
            warn!(error = %err, "failed to resolve collaboration context root");
            return shared_thread_context_documents(hosted.as_ref(), &email);
        }
    };

    let mut documents = repo_context_documents(repo_root.as_path());
    documents.extend(shared_thread_context_documents(hosted.as_ref(), &email));
    documents.sort_by_key(context_default_sort_key);
    documents
}

fn resolve_context_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("failed to resolve current working directory")?;
    Ok(get_git_repo_root(&cwd).unwrap_or(cwd))
}

fn search_context_documents(
    documents: Vec<ContextDocument>,
    query: Option<&str>,
    limit: u32,
) -> Vec<ContextSearchResult> {
    let limit = limit.clamp(1, CONTEXT_MAX_LIMIT) as usize;
    let query = query.map(str::trim).filter(|value| !value.is_empty());

    let mut documents = match query {
        Some(query) => {
            let query_lower = query.to_ascii_lowercase();
            let tokens = query_lower.split_whitespace().collect::<Vec<_>>();
            let mut scored = documents
                .into_iter()
                .filter_map(|document| {
                    document_matches_query(&document, &tokens).then(|| {
                        (
                            context_match_score(&document, &query_lower, &tokens),
                            document,
                        )
                    })
                })
                .collect::<Vec<_>>();
            scored.sort_by(|(score_a, doc_a), (score_b, doc_b)| {
                score_b.cmp(score_a).then_with(|| {
                    context_default_sort_key(doc_a).cmp(&context_default_sort_key(doc_b))
                })
            });
            scored.into_iter().map(|(_, document)| document).collect()
        }
        None => documents,
    };

    if documents.len() > limit {
        documents.truncate(limit);
    }

    documents
        .into_iter()
        .map(ContextDocument::into_search_result)
        .collect()
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
    let kind_rank = match document.kind {
        ContextKind::RepoContextFile => 0,
        ContextKind::SharedThread => 1,
    };
    (
        kind_rank,
        document.title.to_ascii_lowercase(),
        document.ref_id.clone(),
    )
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
    let body = body.trim();
    let title = metadata
        .title
        .clone()
        .or_else(|| first_markdown_heading(body))
        .or_else(|| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| relative_path.display().to_string());
    let location = relative_path.display().to_string();
    let summary = repo_context_summary(&metadata, body);
    let body = non_empty_string(truncate_context_body(body));
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
    })
}

fn repo_context_summary(metadata: &RepoContextMetadata, body: &str) -> Option<String> {
    let leading_line = first_meaningful_body_line(body);
    match (
        metadata.kind.as_deref(),
        metadata.visibility.as_deref(),
        leading_line,
    ) {
        (Some(kind), Some(visibility), Some(line)) => {
            Some(format!("{kind} · {visibility} · {line}"))
        }
        (Some(kind), Some(visibility), None) => Some(format!("{kind} · {visibility}")),
        (Some(kind), None, Some(line)) => Some(format!("{kind} · {line}")),
        (None, Some(visibility), Some(line)) => Some(format!("{visibility} · {line}")),
        (Some(kind), None, None) => Some(kind.to_string()),
        (None, Some(visibility), None) => Some(visibility.to_string()),
        (None, None, Some(line)) => Some(line),
        (None, None, None) => None,
    }
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
    let mut metadata = RepoContextMetadata::default();
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("id:") {
            metadata.id = non_empty_string(strip_yaml_quotes(value).to_string());
        } else if let Some(value) = trimmed.strip_prefix("title:") {
            metadata.title = non_empty_string(strip_yaml_quotes(value).to_string());
        } else if let Some(value) = trimmed.strip_prefix("kind:") {
            metadata.kind = non_empty_string(strip_yaml_quotes(value).to_string());
        } else if let Some(value) = trimmed.strip_prefix("visibility:") {
            metadata.visibility = non_empty_string(strip_yaml_quotes(value).to_string());
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

fn shared_thread_context_documents(
    hosted: Option<&HostedServer>,
    email: &str,
) -> Vec<ContextDocument> {
    let Some(hosted) = hosted else {
        return Vec::new();
    };
    if member_role(hosted, email).is_none() {
        return Vec::new();
    }

    let mut threads = hosted.threads.values().cloned().collect::<Vec<_>>();
    threads.sort_by(|a, b| a.thread_id.cmp(&b.thread_id));
    threads
        .into_iter()
        .map(|thread| {
            let title = thread
                .preview
                .clone()
                .unwrap_or_else(|| thread.thread_id.clone());
            let summary = Some(format!(
                "owner={} · shared_by={} · shared_at={}",
                thread.owner_email, thread.shared_by_email, thread.shared_at
            ));
            let location = format!("thread/{}", thread.thread_id);
            let body = shared_thread_context_body(&thread);
            let search_text = format!(
                "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
                title,
                summary.clone().unwrap_or_default(),
                location,
                thread.thread_id,
                thread.repo_root.clone().unwrap_or_default(),
                thread.git_branch.clone().unwrap_or_default(),
                thread.git_sha.clone().unwrap_or_default(),
                thread.git_origin_url.clone().unwrap_or_default(),
                body.clone().unwrap_or_default()
            )
            .to_ascii_lowercase();

            ContextDocument {
                ref_id: format!("ctx:thread:{}", thread.thread_id),
                kind: ContextKind::SharedThread,
                title,
                summary,
                location: Some(location),
                body,
                search_text,
            }
        })
        .collect()
}

fn shared_thread_context_body(thread: &SharedThread) -> Option<String> {
    let mut lines = vec![
        format!("Thread: {}", thread.thread_id),
        format!("Owner: {}", thread.owner_email),
        format!("Shared by: {}", thread.shared_by_email),
        format!("Shared at: {}", thread.shared_at),
    ];
    if let Some(preview) = non_empty_string(thread.preview.clone().unwrap_or_default()) {
        lines.push(format!("Preview: {preview}"));
    }
    if let Some(repo_root) = non_empty_string(thread.repo_root.clone().unwrap_or_default()) {
        lines.push(format!("Repo root: {repo_root}"));
    }
    if let Some(git_branch) = non_empty_string(thread.git_branch.clone().unwrap_or_default()) {
        lines.push(format!("Git branch: {git_branch}"));
    }
    if let Some(git_sha) = non_empty_string(thread.git_sha.clone().unwrap_or_default()) {
        lines.push(format!("Git SHA: {git_sha}"));
    }
    if let Some(git_origin_url) =
        non_empty_string(thread.git_origin_url.clone().unwrap_or_default())
    {
        lines.push(format!("Git origin: {git_origin_url}"));
    }

    if let Some(history) = thread.history.as_deref() {
        let replay = replay_messages_from_history(history);
        if !replay.is_empty() {
            lines.push(String::new());
            lines.push("Recent transcript:".to_string());
            for message in replay.into_iter().take(8) {
                let role = match message.role {
                    TogetherReplayRole::User => "User",
                    TogetherReplayRole::Assistant => "Assistant",
                    TogetherReplayRole::System => "System",
                };
                lines.push(format!(
                    "{role}: {}",
                    single_line_excerpt(&message.text, 180)
                ));
            }
        }
    }

    non_empty_string(truncate_context_body(&lines.join("\n")))
}

async fn build_context_bundle(
    state: &AppState,
    ctx: &ConnectionContext,
    context_refs: Vec<ContextRef>,
) -> ContextResolveBundleResponse {
    if context_refs.is_empty() {
        return ContextResolveBundleResponse {
            bundle_text: String::new(),
            kept_refs: Vec::new(),
            dropped_refs: Vec::new(),
        };
    }

    let documents = context_documents(state, ctx).await;
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
    };

    ContextRef {
        ref_id: document.ref_id.clone(),
        kind: document.kind,
        display_label: document.title.clone(),
        source_thread_id,
        repo_context_id,
        git_branch: None,
        stale_state: Some(ContextStaleState::Fresh),
    }
}

fn document_bundle_text(document: &ContextDocument) -> String {
    let kind = match document.kind {
        ContextKind::SharedThread => "shared thread",
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

async fn source_thread_context_entry(
    bridge: &mut AppServerBridge,
    thread_id: &str,
) -> Result<ResolvedContextEntry, AppServerError> {
    let thread_read = thread_read_with_turn_fallback(bridge, thread_id).await?;
    let thread = thread_read.thread;
    let title = thread
        .name
        .clone()
        .or_else(|| non_empty_string(thread.preview.clone()))
        .unwrap_or_else(|| thread.id.clone());
    let location = format!("thread/{}", thread.id);
    let summary = Some(format!(
        "local thread · cwd={} · updated_at={}",
        thread.cwd.display(),
        thread.updated_at
    ));
    let body = local_thread_context_body(&thread);
    let document = ContextDocument {
        ref_id: format!("ctx:thread:{}", thread.id),
        kind: ContextKind::SharedThread,
        title,
        summary,
        location: Some(location),
        body,
        search_text: String::new(),
    };

    Ok(resolved_entry_from_document(document))
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

fn local_thread_context_body(thread: &codex_app_server_protocol::Thread) -> Option<String> {
    let mut lines = vec![
        format!("Thread: {}", thread.id),
        format!("Cwd: {}", thread.cwd.display()),
        format!("Updated at: {}", thread.updated_at),
    ];
    if let Some(name) = &thread.name {
        lines.push(format!("Title: {name}"));
    }
    if let Some(preview) = non_empty_string(thread.preview.clone()) {
        lines.push(format!("Preview: {preview}"));
    }

    let replay = replay_messages_from_turns(&thread.turns);
    if !replay.is_empty() {
        lines.push(String::new());
        lines.push("Recent transcript:".to_string());
        for message in replay.into_iter().take(8) {
            let role = match message.role {
                TogetherReplayRole::User => "User",
                TogetherReplayRole::Assistant => "Assistant",
                TogetherReplayRole::System => "System",
            };
            lines.push(format!(
                "{role}: {}",
                single_line_excerpt(&message.text, 180)
            ));
        }
    }

    non_empty_string(truncate_context_body(&lines.join("\n")))
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
        .unwrap_or_else(|| document.title.clone());
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
        .clone()
        .unwrap_or_else(|| context_write_id_from_path(&relative_path));
    let visibility = existing_metadata
        .visibility
        .unwrap_or_else(|| "repo".to_string());
    let source_threads = document
        .ref_id
        .strip_prefix("ctx:thread:")
        .map(|thread_id| vec![thread_id.to_string()])
        .unwrap_or_default();
    let source_files = match (&document.kind, &document.location) {
        (ContextKind::RepoContextFile, Some(location)) => vec![location.clone()],
        _ => Vec::new(),
    };
    let content = render_context_write_file(
        ContextWriteMetadata {
            id,
            kind: kind.clone(),
            title: title.clone(),
            branch,
            source_threads,
            source_files,
            last_validated_at: Utc::now().format("%Y-%m-%d").to_string(),
            visibility,
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
    visibility: String,
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
    lines.push(format!("visibility: {}", yaml_quoted(&metadata.visibility)));
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
    let slug = slugify_context_value(document.title.as_str());
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

    let mut lines = vec![format!("# {}", document.title)];
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
    match (&document.kind, &document.location) {
        (ContextKind::SharedThread, _) => {
            if let Some(thread_id) = document.ref_id.strip_prefix("ctx:thread:") {
                lines.push(format!("- shared thread: {thread_id}"));
            }
        }
        (ContextKind::RepoContextFile, Some(location)) => {
            lines.push(format!("- repo context: {location}"));
        }
        (ContextKind::RepoContextFile, None) => {}
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

fn replay_messages_from_turns(turns: &[Turn]) -> Vec<TogetherReplayMessage> {
    let mut out = Vec::new();
    for turn in turns {
        for item in &turn.items {
            match item {
                ThreadItem::UserMessage { content, .. } => {
                    if let Some(text) = replay_user_message_text(content) {
                        out.push(TogetherReplayMessage {
                            role: TogetherReplayRole::User,
                            text,
                        });
                    }
                }
                ThreadItem::AgentMessage { text, .. } => {
                    if let Some(text) = non_empty_string(text.clone()) {
                        out.push(TogetherReplayMessage {
                            role: TogetherReplayRole::Assistant,
                            text,
                        });
                    }
                }
                ThreadItem::Plan { text, .. } => {
                    if let Some(text) = non_empty_string(text.clone()) {
                        out.push(TogetherReplayMessage {
                            role: TogetherReplayRole::System,
                            text: format!("Plan update:\n{text}"),
                        });
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn replay_messages_from_history(
    history: &[codex_protocol::protocol::RolloutItem],
) -> Vec<TogetherReplayMessage> {
    history
        .iter()
        .filter_map(|item| match item {
            codex_protocol::protocol::RolloutItem::ResponseItem(
                codex_protocol::models::ResponseItem::Message { role, content, .. },
            ) => history_message_from_content(role.as_str(), content.as_slice()),
            _ => None,
        })
        .collect()
}

async fn forkable_history_from_rollout(
    path: Option<&PathBuf>,
) -> Option<Vec<codex_protocol::protocol::RolloutItem>> {
    let path = path?;

    match RolloutRecorder::get_rollout_history(path).await {
        Ok(codex_protocol::protocol::InitialHistory::New) => None,
        Ok(codex_protocol::protocol::InitialHistory::Resumed(resumed)) => Some(resumed.history),
        Ok(codex_protocol::protocol::InitialHistory::Forked(items)) => Some(items),
        Err(err) => {
            tracing::warn!(
                error = %err,
                rollout_path = %path.display(),
                "failed to read together rollout history"
            );
            None
        }
    }
}

fn canonicalize_shared_history(
    thread_id: &str,
    mut history: Vec<codex_protocol::protocol::RolloutItem>,
) -> Vec<codex_protocol::protocol::RolloutItem> {
    if let Ok(shared_thread_id) = ThreadId::from_string(thread_id)
        && let Some(codex_protocol::protocol::RolloutItem::SessionMeta(meta_line)) = history
            .iter_mut()
            .find(|item| matches!(item, codex_protocol::protocol::RolloutItem::SessionMeta(_)))
    {
        meta_line.meta.id = shared_thread_id;
    }
    history
}

fn rollout_history_preview(history: &[codex_protocol::protocol::RolloutItem]) -> Option<String> {
    history.iter().find_map(|item| match item {
        codex_protocol::protocol::RolloutItem::ResponseItem(response_item) => {
            let TurnItem::UserMessage(user_message) = codex_core::parse_turn_item(response_item)?
            else {
                return None;
            };
            let message = user_message.message();
            let preview = match message.find(codex_protocol::protocol::USER_MESSAGE_BEGIN) {
                Some(idx) => {
                    message[idx + codex_protocol::protocol::USER_MESSAGE_BEGIN.len()..].trim()
                }
                None => message.trim(),
            };
            non_empty_string(preview.to_string())
        }
        _ => None,
    })
}

fn history_message_from_content(
    role: &str,
    content: &[codex_protocol::models::ContentItem],
) -> Option<TogetherReplayMessage> {
    let text = non_empty_string(
        content
            .iter()
            .filter_map(|entry| match entry {
                codex_protocol::models::ContentItem::InputText { text }
                | codex_protocol::models::ContentItem::OutputText { text } => Some(text.clone()),
                codex_protocol::models::ContentItem::InputImage { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )?;

    let role = match role {
        "user" => TogetherReplayRole::User,
        "assistant" => TogetherReplayRole::Assistant,
        "system" => TogetherReplayRole::System,
        _ => return None,
    };

    Some(TogetherReplayMessage { role, text })
}

fn replay_user_message_text(content: &[UserInput]) -> Option<String> {
    let mut parts = Vec::new();
    for entry in content {
        match entry {
            UserInput::Text { text, .. } => {
                if let Some(value) = non_empty_string(text.clone()) {
                    parts.push(value);
                }
            }
            UserInput::Image { url } => parts.push(format!("[image] {url}")),
            UserInput::LocalImage { path } => {
                parts.push(format!("[local image] {}", path.display()))
            }
            UserInput::Skill { name, .. } => parts.push(format!("[skill] {name}")),
            UserInput::Mention { name, .. } => parts.push(format!("@{name}")),
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
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
    use super::HostedServer;
    use super::SharedThread;
    use super::plan_context_write_files;
    use super::repo_context_documents;
    use super::rollout_history_preview;
    use super::search_context_documents;
    use super::shared_thread_context_documents;
    use codex_protocol::ThreadId;
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::ResponseItem;
    use codex_protocol::protocol::RolloutItem;
    use codex_protocol::protocol::USER_MESSAGE_BEGIN;
    use codex_together_protocol::ContextKind;
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn rollout_history_preview_skips_contextual_messages() {
        let history = vec![
            RolloutItem::ResponseItem(ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "# AGENTS.md instructions for /tmp/project\n\n<INSTRUCTIONS>\nhide me\n</INSTRUCTIONS>"
                        .to_string(),
                }],
                end_turn: None,
                phase: None,
            }),
            RolloutItem::ResponseItem(ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "<environment_context>\n<cwd>/tmp/project</cwd>\n</environment_context>"
                        .to_string(),
                }],
                end_turn: None,
                phase: None,
            }),
            RolloutItem::ResponseItem(ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: format!("{USER_MESSAGE_BEGIN} can we like hide the system prompt and stuff lol"),
                }],
                end_turn: None,
                phase: None,
            }),
        ];

        assert_eq!(
            rollout_history_preview(&history),
            Some("can we like hide the system prompt and stuff lol".to_string())
        );
    }

    #[test]
    fn repo_context_documents_parse_frontmatter_and_body() {
        let temp_root = temp_test_dir("repo-context-docs");
        let context_dir = temp_root.join(".codex").join("context");
        std::fs::create_dir_all(&context_dir).expect("create context dir");
        std::fs::write(
            context_dir.join("overview.md"),
            "---\ntitle: Planning Notes\nkind: plan\nvisibility: public\n---\n# Planning Notes\n\nShip the context browser first.\n",
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
            Some("plan · public · Ship the context browser first.")
        );
        assert_eq!(
            document.body.as_deref(),
            Some("# Planning Notes\n\nShip the context browser first.")
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
            "---\nid: planning-overview\nkind: decision\ntitle: Planning Overview\nvisibility: repo\n---\n# Planning Overview\n\nKeep the body stable.\n",
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
    fn context_write_plan_creates_new_file_for_shared_thread() {
        let temp_root = temp_test_dir("repo-context-write-thread");
        let documents = shared_thread_context_documents(
            Some(&HostedServer {
                server_id: "srv_test".to_string(),
                owner_email: "owner@example.com".to_string(),
                public_base_url: "https://example.com".to_string(),
                members: HashSet::from(["owner@example.com".to_string()]),
                threads: HashMap::from([(
                    "thread-1".to_string(),
                    SharedThread {
                        thread_id: "thread-1".to_string(),
                        owner_email: "owner@example.com".to_string(),
                        shared_by_email: "owner@example.com".to_string(),
                        preview: Some("planning sync".to_string()),
                        shared_at: "2026-03-08T12:05:00Z".to_string(),
                        history: None,
                        repo_root: None,
                        git_branch: None,
                        git_sha: None,
                        git_origin_url: None,
                    },
                )]),
            }),
            "owner@example.com",
        );
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
            ".codex/context/concepts/planning-sync.md"
        );
        assert!(!planned[0].exists);
        assert!(planned[0].content.contains("source_threads:"));
        assert!(planned[0].content.contains("- \"thread-1\""));
        assert!(planned[0].content.contains("## Sources"));
        assert!(planned[0].content.contains("- shared thread: thread-1"));

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn search_context_documents_orders_repo_context_before_shared_thread_on_ties() {
        let temp_root = temp_test_dir("context-search-order");
        let context_dir = temp_root.join(".codex").join("context");
        std::fs::create_dir_all(&context_dir).expect("create context dir");
        std::fs::write(
            context_dir.join("planning.md"),
            "---\ntitle: Planning Overview\nkind: note\n---\nCapture the collaboration rewrite milestones.\n",
        )
        .expect("write repo context");

        let mut documents = repo_context_documents(&temp_root);
        let hosted = HostedServer {
            server_id: "srv_test".to_string(),
            owner_email: "owner@example.com".to_string(),
            public_base_url: "https://example.com".to_string(),
            members: HashSet::from(["owner@example.com".to_string()]),
            threads: HashMap::from([(
                "thread-1".to_string(),
                SharedThread {
                    thread_id: "thread-1".to_string(),
                    owner_email: "owner@example.com".to_string(),
                    shared_by_email: "owner@example.com".to_string(),
                    preview: Some("planning sync".to_string()),
                    shared_at: "2026-03-08T12:05:00Z".to_string(),
                    history: None,
                    repo_root: None,
                    git_branch: None,
                    git_sha: None,
                    git_origin_url: None,
                },
            )]),
        };
        documents.extend(shared_thread_context_documents(
            Some(&hosted),
            "owner@example.com",
        ));

        let results = search_context_documents(documents, Some("planning"), 10);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].kind, ContextKind::RepoContextFile);
        assert_eq!(results[0].title, "Planning Overview");
        assert_eq!(results[1].kind, ContextKind::SharedThread);
        assert_eq!(results[1].location.as_deref(), Some("thread/thread-1"));

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn search_context_documents_matches_shared_thread_git_metadata() {
        let documents = shared_thread_context_documents(
            Some(&HostedServer {
                server_id: "srv_test".to_string(),
                owner_email: "owner@example.com".to_string(),
                public_base_url: "https://example.com".to_string(),
                members: HashSet::from(["owner@example.com".to_string()]),
                threads: HashMap::from([(
                    "thread-1".to_string(),
                    SharedThread {
                        thread_id: "thread-1".to_string(),
                        owner_email: "owner@example.com".to_string(),
                        shared_by_email: "owner@example.com".to_string(),
                        preview: Some("planning sync".to_string()),
                        shared_at: "2026-03-08T12:05:00Z".to_string(),
                        history: None,
                        repo_root: Some("/tmp/repo".to_string()),
                        git_branch: Some("rewrite-codex-2gether-v2".to_string()),
                        git_sha: Some("abc123def456".to_string()),
                        git_origin_url: Some(
                            "git@github.com:openai/codex-together.git".to_string(),
                        ),
                    },
                )]),
            }),
            "owner@example.com",
        );

        let results = search_context_documents(documents, Some("rewrite-codex-2gether-v2"), 10);

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
    fn shared_thread_context_documents_require_membership() {
        let hosted = HostedServer {
            server_id: "srv_test".to_string(),
            owner_email: "owner@example.com".to_string(),
            public_base_url: "https://example.com".to_string(),
            members: HashSet::from(["owner@example.com".to_string()]),
            threads: HashMap::from([(
                "thread-1".to_string(),
                SharedThread {
                    thread_id: "thread-1".to_string(),
                    owner_email: "owner@example.com".to_string(),
                    shared_by_email: "owner@example.com".to_string(),
                    preview: Some("planning sync".to_string()),
                    shared_at: "2026-03-08T12:05:00Z".to_string(),
                    history: None,
                    repo_root: None,
                    git_branch: None,
                    git_sha: None,
                    git_origin_url: None,
                },
            )]),
        };

        assert_eq!(
            shared_thread_context_documents(Some(&hosted), "guest@example.com").len(),
            0
        );
        assert_eq!(
            shared_thread_context_documents(Some(&hosted), "owner@example.com").len(),
            1
        );
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", ThreadId::new()));
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        dir
    }
}
