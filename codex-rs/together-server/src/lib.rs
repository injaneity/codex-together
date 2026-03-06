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
use codex_app_server_protocol::ThreadForkParams;
use codex_app_server_protocol::ThreadForkResponse;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadReadParams;
use codex_app_server_protocol::ThreadReadResponse;
use codex_app_server_protocol::Turn;
use codex_app_server_protocol::UserInput;
use codex_core::config::find_codex_home;
use codex_state::StateRuntime;
use codex_state::TogetherClientMode as StateTogetherClientMode;
use codex_state::TogetherClientSession as StateTogetherClientSession;
use codex_state::TogetherMemberRecord;
use codex_state::TogetherRole as StateTogetherRole;
use codex_state::TogetherServerRecord;
use codex_state::TogetherThreadAclRecord;
use codex_state::TogetherThreadForkRecord;
use codex_together_protocol::CheckoutReason;
use codex_together_protocol::ConnectedMember;
use codex_together_protocol::JsonRpcNotification;
use codex_together_protocol::JsonRpcRequest;
use codex_together_protocol::JsonRpcResponse;
use codex_together_protocol::LineageEdge;
use codex_together_protocol::LineageNode;
use codex_together_protocol::METHOD_INITIALIZE;
use codex_together_protocol::METHOD_INITIALIZED;
use codex_together_protocol::METHOD_TOGETHER_AUTH;
use codex_together_protocol::METHOD_TOGETHER_HISTORY_LINEAGE;
use codex_together_protocol::METHOD_TOGETHER_JOIN;
use codex_together_protocol::METHOD_TOGETHER_LEAVE;
use codex_together_protocol::METHOD_TOGETHER_MEMBER_ADD;
use codex_together_protocol::METHOD_TOGETHER_MEMBER_REMOVE;
use codex_together_protocol::METHOD_TOGETHER_SERVER_CLOSE;
use codex_together_protocol::METHOD_TOGETHER_SERVER_CREATE;
use codex_together_protocol::METHOD_TOGETHER_SERVER_INFO;
use codex_together_protocol::METHOD_TOGETHER_THREAD_CHECKOUT;
use codex_together_protocol::METHOD_TOGETHER_THREAD_DELETE;
use codex_together_protocol::METHOD_TOGETHER_THREAD_FORK;
use codex_together_protocol::METHOD_TOGETHER_THREAD_LIST;
use codex_together_protocol::METHOD_TOGETHER_THREAD_READ;
use codex_together_protocol::METHOD_TOGETHER_THREAD_SHARE;
use codex_together_protocol::NOTIFY_TOGETHER_CONNECTION_REVOKED;
use codex_together_protocol::NOTIFY_TOGETHER_MEMBER_UPDATED;
use codex_together_protocol::NOTIFY_TOGETHER_SERVER_CLOSED;
use codex_together_protocol::NOTIFY_TOGETHER_THREAD_FORKED;
use codex_together_protocol::NOTIFY_TOGETHER_THREAD_SHARED;
use codex_together_protocol::TogetherAuthRequest;
use codex_together_protocol::TogetherAuthResponse;
use codex_together_protocol::TogetherHistoryLineageRequest;
use codex_together_protocol::TogetherHistoryLineageResponse;
use codex_together_protocol::TogetherJoinRequest;
use codex_together_protocol::TogetherJoinResponse;
use codex_together_protocol::TogetherLeaveResponse;
use codex_together_protocol::TogetherMemberUpdateRequest;
use codex_together_protocol::TogetherMemberUpdateResponse;
use codex_together_protocol::TogetherReplayMessage;
use codex_together_protocol::TogetherReplayRole;
use codex_together_protocol::TogetherRole;
use codex_together_protocol::TogetherServerCloseResponse;
use codex_together_protocol::TogetherServerCreateRequest;
use codex_together_protocol::TogetherServerCreateResponse;
use codex_together_protocol::TogetherServerInfoResponse;
use codex_together_protocol::TogetherThreadCheckoutRequest;
use codex_together_protocol::TogetherThreadCheckoutResponse;
use codex_together_protocol::TogetherThreadDeleteRequest;
use codex_together_protocol::TogetherThreadDeleteResponse;
use codex_together_protocol::TogetherThreadForkRequest;
use codex_together_protocol::TogetherThreadForkResponse;
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
    connections: HashMap<Uuid, ConnectionEntry>,
}

struct ConnectionEntry {
    tx: mpsc::UnboundedSender<String>,
    email: Option<String>,
}

#[derive(Debug, Clone)]
struct HostedServer {
    server_id: String,
    owner_email: String,
    public_base_url: String,
    members: HashSet<String>,
    threads: HashMap<String, SharedThread>,
    forks: Vec<ForkEdge>,
}

#[derive(Debug, Clone)]
struct SharedThread {
    thread_id: String,
    owner_email: String,
    preview: Option<String>,
    shared_at: String,
}

#[derive(Debug, Clone)]
struct ForkEdge {
    parent_thread_id: String,
    child_thread_id: String,
    actor_email: String,
    created_at: String,
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
    })
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
        METHOD_TOGETHER_SERVER_CREATE => together_server_create(state, ctx, req).await,
        METHOD_TOGETHER_SERVER_CLOSE => together_server_close(state, ctx, req).await,
        METHOD_TOGETHER_MEMBER_ADD => update_member(state, ctx, req, true).await,
        METHOD_TOGETHER_MEMBER_REMOVE => update_member(state, ctx, req, false).await,
        METHOD_TOGETHER_SERVER_INFO => together_server_info(state, ctx, req).await,
        METHOD_TOGETHER_THREAD_SHARE => together_thread_share(state, ctx, req).await,
        METHOD_TOGETHER_THREAD_CHECKOUT => together_thread_checkout(state, ctx, req).await,
        METHOD_TOGETHER_THREAD_READ => together_thread_read(state, ctx, req).await,
        METHOD_TOGETHER_THREAD_FORK => together_thread_fork(state, ctx, req).await,
        METHOD_TOGETHER_THREAD_DELETE => together_thread_delete(state, ctx, req).await,
        METHOD_TOGETHER_THREAD_LIST => together_thread_list(state, ctx, req).await,
        METHOD_TOGETHER_HISTORY_LINEAGE => together_history_lineage(state, ctx, req).await,
        METHOD_TOGETHER_JOIN => together_join(state, ctx, req).await,
        METHOD_TOGETHER_LEAVE => together_leave(state, connection_id, ctx, req).await,
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
            forks: Vec::new(),
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

async fn together_server_close(
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
            NOTIFY_TOGETHER_SERVER_CLOSED,
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

    JsonRpcResponse::ok(req.id, TogetherServerCloseResponse { closed: true })
        .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn update_member(
    state: &AppState,
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
    add: bool,
) -> JsonRpcResponse {
    let payload: TogetherMemberUpdateRequest = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

    let caller_email = ctx
        .email
        .clone()
        .unwrap_or_else(|| "guest@local".to_string());

    let (server_id, changed) = {
        let mut guard = state.inner.lock().await;
        let (server_id, changed, notify_email) = {
            let Some(hosted) = guard.hosted.as_mut() else {
                return rpc_error(req.id, RPC_ERR_NOT_CONNECTED, "TOGETHER_NOT_CONNECTED");
            };

            if hosted.owner_email != caller_email {
                return rpc_error(req.id, RPC_ERR_FORBIDDEN, "TOGETHER_FORBIDDEN");
            }
            if !add && payload.email == hosted.owner_email {
                return rpc_error(req.id, RPC_ERR_FORBIDDEN, "cannot remove owner");
            }

            let changed = if add {
                hosted.members.insert(payload.email.clone())
            } else {
                hosted.members.remove(&payload.email)
            };

            (hosted.server_id.clone(), changed, payload.email.clone())
        };
        broadcast_notification(
            &guard,
            NOTIFY_TOGETHER_MEMBER_UPDATED,
            serde_json::json!({
                "email": notify_email,
                "added": add,
            }),
        );

        if !add {
            notify_connection_revoked(&guard, &notify_email);
        }

        (server_id, changed)
    };

    if changed {
        let now = Utc::now().timestamp();
        let role = if add {
            StateTogetherRole::Member
        } else {
            StateTogetherRole::Member
        };
        let removed_at = if add { None } else { Some(now) };

        if let Err(err) = state
            .state_db
            .upsert_together_member(&TogetherMemberRecord {
                server_id,
                email: payload.email,
                role,
                added_at: now,
                removed_at,
            })
            .await
        {
            error!(error = %err, "failed to persist together member update");
            return rpc_error(req.id, -32603, "failed to persist member update");
        }
    }

    JsonRpcResponse::ok(req.id, TogetherMemberUpdateResponse { updated: changed })
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

    JsonRpcResponse::ok(
        req.id,
        TogetherServerInfoResponse {
            server_id: hosted.server_id.clone(),
            owner_email: hosted.owner_email.clone(),
            public_base_url: hosted.public_base_url.clone(),
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

    let preview = non_empty_string(thread.preview);
    let shared_at = Utc::now().to_rfc3339();
    let shared_at_epoch = Utc::now().timestamp();

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
                    preview: preview.clone(),
                    shared_at: shared_at.clone(),
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
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn together_thread_checkout(
    state: &AppState,
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: TogetherThreadCheckoutRequest = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };
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

    let Some(thread) = hosted.threads.get(&payload.thread_id) else {
        return rpc_error(req.id, -32602, "thread not shared");
    };
    let writable = thread.owner_email == email;

    JsonRpcResponse::ok(
        req.id,
        TogetherThreadCheckoutResponse {
            thread_id: thread.thread_id.clone(),
            writable,
            owner_email: thread.owner_email.clone(),
            reason: if writable {
                None
            } else {
                Some(CheckoutReason::NonOwnerMustFork)
            },
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

    let owner_email = {
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
        thread.owner_email.clone()
    };

    let thread_read = {
        let mut bridge = state.app_server.lock().await;
        match bridge.thread_read(payload.thread_id.clone(), true).await {
            Ok(response) => response,
            Err(err) => return app_server_error_response(req.id, err),
        }
    };

    JsonRpcResponse::ok(
        req.id,
        TogetherThreadReadResponse {
            thread_id: payload.thread_id,
            owner_email,
            messages: replay_messages_from_turns(&thread_read.thread.turns),
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn together_thread_fork(
    state: &AppState,
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: TogetherThreadForkRequest = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };
    let caller_email = ctx
        .email
        .clone()
        .unwrap_or_else(|| "guest@local".to_string());

    {
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
        if !hosted.threads.contains_key(&payload.thread_id) {
            return rpc_error(req.id, -32602, "thread not shared");
        }
    }

    {
        let mut bridge = state.app_server.lock().await;
        let parent_read = match bridge.thread_read(payload.thread_id.clone(), true).await {
            Ok(response) => response,
            Err(err) => return app_server_error_response(req.id.clone(), err),
        };
        if parent_read.thread.turns.is_empty() {
            return rpc_error(
                req.id,
                -32602,
                "cannot fork thread: no persisted turns yet; send at least one message first",
            );
        }
    }

    let forked = {
        let mut bridge = state.app_server.lock().await;
        match bridge
            .thread_fork(payload.thread_id.clone(), payload.cwd.clone())
            .await
        {
            Ok(response) => response,
            Err(err) => return app_server_error_response(req.id, err),
        }
    };

    let child_thread_id = forked.thread.id.clone();
    let preview = non_empty_string(forked.thread.preview);
    let now = Utc::now();
    let created_at = now.to_rfc3339();
    let created_at_epoch = now.timestamp();

    let server_id = {
        let mut guard = state.inner.lock().await;
        let (server_id, edge) = {
            let Some(hosted) = guard.hosted.as_mut() else {
                return rpc_error(req.id, RPC_ERR_NOT_CONNECTED, "TOGETHER_NOT_CONNECTED");
            };

            hosted.threads.insert(
                child_thread_id.clone(),
                SharedThread {
                    thread_id: child_thread_id.clone(),
                    owner_email: caller_email.clone(),
                    preview,
                    shared_at: created_at.clone(),
                },
            );

            let edge = ForkEdge {
                parent_thread_id: payload.thread_id.clone(),
                child_thread_id: child_thread_id.clone(),
                actor_email: caller_email.clone(),
                created_at: created_at.clone(),
            };
            hosted.forks.push(edge.clone());

            (hosted.server_id.clone(), edge)
        };
        broadcast_notification(
            &guard,
            NOTIFY_TOGETHER_THREAD_FORKED,
            serde_json::json!({
                "parentThreadId": edge.parent_thread_id,
                "childThreadId": edge.child_thread_id,
                "actorEmail": edge.actor_email,
                "createdAt": edge.created_at,
            }),
        );

        server_id
    };

    if let Err(err) = state
        .state_db
        .upsert_together_thread_acl(&TogetherThreadAclRecord {
            server_id: server_id.clone(),
            thread_id: child_thread_id.clone(),
            owner_email: caller_email.clone(),
            shared_by_email: caller_email.clone(),
            shared_at: created_at_epoch,
        })
        .await
    {
        error!(error = %err, "failed to persist forked thread ACL");
        return rpc_error(req.id, -32603, "failed to persist fork metadata");
    }

    if let Err(err) = state
        .state_db
        .insert_together_thread_fork(&TogetherThreadForkRecord {
            server_id,
            child_thread_id: child_thread_id.clone(),
            parent_thread_id: payload.thread_id.clone(),
            actor_email: caller_email,
            created_at: created_at_epoch,
        })
        .await
    {
        error!(error = %err, "failed to persist thread fork edge");
        return rpc_error(req.id, -32603, "failed to persist fork metadata");
    }

    JsonRpcResponse::ok(
        req.id,
        TogetherThreadForkResponse {
            parent_thread_id: payload.thread_id,
            child_thread_id,
            writable: true,
        },
    )
    .unwrap_or_else(|_| rpc_error(Value::Null, -32603, "serialization failed"))
}

async fn together_thread_delete(
    state: &AppState,
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: TogetherThreadDeleteRequest = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };
    let caller_email = ctx
        .email
        .clone()
        .unwrap_or_else(|| "guest@local".to_string());

    let deleted = {
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

        let Some(thread) = hosted.threads.get(&payload.thread_id) else {
            return rpc_error(req.id, -32602, "thread not shared");
        };
        if thread.owner_email != caller_email {
            return rpc_error(req.id, RPC_ERR_FORBIDDEN, "TOGETHER_FORBIDDEN");
        }

        if hosted.threads.remove(&payload.thread_id).is_some() {
            hosted.forks.retain(|edge| {
                edge.parent_thread_id != payload.thread_id
                    && edge.child_thread_id != payload.thread_id
            });
            true
        } else {
            false
        }
    };

    JsonRpcResponse::ok(
        req.id,
        TogetherThreadDeleteResponse {
            thread_id: payload.thread_id,
            deleted,
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

async fn together_history_lineage(
    state: &AppState,
    ctx: &ConnectionContext,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let payload: TogetherHistoryLineageRequest = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(_) => return rpc_error(req.id, -32602, "invalid params"),
    };

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

    let edges = lineage_edges(hosted, &payload.root_thread_id);
    let mut node_ids = HashSet::from([payload.root_thread_id.clone()]);
    for edge in &edges {
        node_ids.insert(edge.parent_thread_id.clone());
        node_ids.insert(edge.child_thread_id.clone());
    }

    let mut nodes: Vec<LineageNode> = node_ids
        .into_iter()
        .map(|thread_id| {
            let owner_email = hosted
                .threads
                .get(&thread_id)
                .map(|thread| thread.owner_email.clone())
                .unwrap_or_else(|| hosted.owner_email.clone());
            LineageNode {
                thread_id,
                owner_email,
            }
        })
        .collect();
    nodes.sort_by(|a, b| a.thread_id.cmp(&b.thread_id));

    JsonRpcResponse::ok(
        req.id,
        TogetherHistoryLineageResponse {
            root: payload.root_thread_id,
            nodes,
            edges,
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

fn notify_connection_revoked(state: &ServerState, target_email: &str) {
    let note = JsonRpcNotification {
        jsonrpc: "2.0".to_string(),
        method: NOTIFY_TOGETHER_CONNECTION_REVOKED.to_string(),
        params: serde_json::json!({
            "email": target_email,
        }),
    };

    if let Ok(text) = serde_json::to_string(&note) {
        for entry in state.connections.values() {
            if entry.email.as_deref() == Some(target_email) {
                let _ = entry.tx.send(text.clone());
            }
        }
    }
}

fn lineage_edges(hosted: &HostedServer, root: &str) -> Vec<LineageEdge> {
    let mut out = Vec::new();
    let mut frontier = HashSet::from([root.to_string()]);
    let mut seen_edges: HashSet<(String, String)> = HashSet::new();

    loop {
        let mut progressed = false;
        for edge in &hosted.forks {
            if !frontier.contains(&edge.parent_thread_id) {
                continue;
            }

            if seen_edges.insert((edge.parent_thread_id.clone(), edge.child_thread_id.clone())) {
                out.push(LineageEdge {
                    parent_thread_id: edge.parent_thread_id.clone(),
                    child_thread_id: edge.child_thread_id.clone(),
                    actor_email: edge.actor_email.clone(),
                    created_at: edge.created_at.clone(),
                });
            }

            if frontier.insert(edge.child_thread_id.clone()) {
                progressed = true;
            }
        }

        if !progressed {
            break;
        }
    }

    out.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.parent_thread_id.cmp(&b.parent_thread_id))
            .then_with(|| a.child_thread_id.cmp(&b.child_thread_id))
    });
    out
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
        AppServerError::Rpc { code, .. } if code == -32001 => {
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

    async fn thread_fork(
        &mut self,
        thread_id: String,
        cwd: Option<String>,
    ) -> Result<ThreadForkResponse, AppServerError> {
        let params = ThreadForkParams {
            thread_id,
            path: None,
            model: None,
            model_provider: None,
            cwd,
            approval_policy: None,
            sandbox: None,
            config: None,
            base_instructions: None,
            developer_instructions: None,
            persist_extended_history: false,
        };

        self.request_with_retry(
            "thread/fork",
            serde_json::to_value(params).map_err(|err| {
                AppServerError::Decode(anyhow::anyhow!(
                    "failed to serialize thread/fork params: {err}"
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
    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }

    match std::io::Error::last_os_error().raw_os_error() {
        Some(code) if code == libc::EPERM => true,
        _ => false,
    }
}

#[cfg(not(unix))]
fn is_pid_running(_pid: u32) -> bool {
    false
}
