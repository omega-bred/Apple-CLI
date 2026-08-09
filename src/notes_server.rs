use anyhow::{anyhow, Context, Result};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::{
    STANDARD as BASE64, URL_SAFE as BASE64_URL_SAFE, URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

const OPENAPI_YAML: &str = include_str!("../openapi/notes-server.yaml");

include!(concat!(env!("OUT_DIR"), "/notes_server_routes.rs"));

#[derive(Debug)]
pub struct ServerState {
    helper_path: PathBuf,
    backend: String,
    token: Option<String>,
    poll_interval: Duration,
    webhooks: RwLock<HashMap<String, WebhookSubscription>>,
    helper_lock: Mutex<()>,
    http_client: reqwest::Client,
}

#[derive(Debug, Clone, Serialize)]
struct WebhookSubscription {
    id: String,
    url: String,
    events: Vec<String>,
    has_secret: bool,
    #[serde(skip)]
    secret: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ApiError {
    status: StatusCode,
    code: String,
    message: String,
    retryable: bool,
    details: Value,
}

impl ApiError {
    fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            retryable: false,
            details: Value::Null,
        }
    }

    fn from_helper(error: &Value) -> Self {
        let code = error
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("internal")
            .to_string();
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Apple Notes helper request failed")
            .to_string();
        let retryable = error
            .get("retryable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let details = error.get("details").cloned().unwrap_or(Value::Null);
        let status = match code.as_str() {
            "invalid.request" | "invalid.params" => StatusCode::BAD_REQUEST,
            "not.found" => StatusCode::NOT_FOUND,
            "permission.denied" => StatusCode::FORBIDDEN,
            "backend.unavailable" | "backend.unsupported" => StatusCode::SERVICE_UNAVAILABLE,
            "notes.timeout" => StatusCode::GATEWAY_TIMEOUT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            code,
            message,
            retryable,
            details,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut error = json!({
            "code": self.code,
            "message": self.message,
            "retryable": self.retryable
        });
        if !self.details.is_null() {
            error["details"] = self.details;
        }
        (
            self.status,
            Json(json!({
                "ok": false,
                "error": error
            })),
        )
            .into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct WriteNoteRequest {
    account: Option<String>,
    folder: Option<String>,
    title: Option<String>,
    name: Option<String>,
    html: Option<String>,
    body: Option<String>,
    #[serde(default)]
    attachments: Vec<AttachmentInput>,
}

#[derive(Debug, Deserialize)]
struct AttachmentInput {
    path: Option<String>,
    name: Option<String>,
    #[serde(rename = "mimeType")]
    mime_type: Option<String>,
    #[serde(rename = "dataBase64")]
    data_base64: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateFolderRequest {
    account: Option<String>,
    name: String,
    parent: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteFolderRequest {
    account: Option<String>,
    name: String,
    parent: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RenameFolderRequest {
    account: Option<String>,
    name: String,
    #[serde(rename = "newName")]
    new_name: String,
    parent: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MoveNoteRequest {
    account: Option<String>,
    folder: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteAttachmentRequest {
    #[serde(rename = "attachmentId")]
    attachment_id: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddAttachmentsRequest {
    attachments: Vec<AttachmentInput>,
}

#[derive(Debug, Deserialize)]
pub struct ShareNoteRequest {
    #[serde(rename = "noteId")]
    note_id: String,
    invitee: Option<String>,
    email: Option<String>,
    backend: Option<String>,
    timeout: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct AcceptShareRequest {
    url: String,
    timeout: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateWebhookRequest {
    url: String,
    secret: Option<String>,
    #[serde(default)]
    events: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListFoldersQuery {
    account: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListNotesQuery {
    account: Option<String>,
    folder: Option<String>,
    #[serde(rename = "sharedOnly")]
    shared_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct SearchNotesQuery {
    account: Option<String>,
    query: String,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct AttachmentContentQuery {
    #[serde(rename = "attachmentId")]
    attachment_id: Option<String>,
    name: Option<String>,
}

pub fn notes_server(args: crate::NotesServerArgs) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create Tokio runtime")?;
    runtime.block_on(run_notes_server(args))
}

async fn run_notes_server(args: crate::NotesServerArgs) -> Result<()> {
    let bind: SocketAddr = args
        .bind
        .parse()
        .with_context(|| format!("invalid bind address: {}", args.bind))?;
    let auth_secret = resolve_server_auth_secret(&args)?;
    if !args.allow_unauthenticated && auth_secret.is_none() && !bind.ip().is_loopback() {
        return Err(anyhow!(
            "refusing to bind unauthenticated Notes API on non-loopback address {bind}; pass --password, --token, or --allow-unauthenticated"
        ));
    }

    let state = Arc::new(ServerState {
        helper_path: resolve_helper_path(args.helper),
        backend: args.backend,
        token: auth_secret,
        poll_interval: Duration::from_secs(args.poll_interval),
        webhooks: RwLock::new(HashMap::new()),
        helper_lock: Mutex::new(()),
        http_client: reqwest::Client::new(),
    });

    tokio::spawn(webhook_poll_loop(state.clone()));

    let app = generated_notes_routes().with_state(state);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind Notes API server on {bind}"))?;
    eprintln!("apple notes server listening on http://{bind}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("Notes API server failed")
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

fn resolve_helper_path(helper: String) -> PathBuf {
    let helper_path = PathBuf::from(&helper);
    if helper_path.components().count() > 1 || helper_path.exists() {
        return helper_path;
    }

    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(&helper)))
        .filter(|path| path.exists())
        .unwrap_or(helper_path)
}

fn resolve_server_auth_secret(args: &crate::NotesServerArgs) -> Result<Option<String>> {
    if let Some(password) = non_empty_secret(args.password.as_deref()) {
        return Ok(Some(password.to_string()));
    }

    if let Some(path) = args
        .password_file
        .as_deref()
        .filter(|path| !path.trim().is_empty())
    {
        return read_password_file(path);
    }

    if let Some(path) = args
        .config
        .as_deref()
        .filter(|path| !path.trim().is_empty())
    {
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read Notes server config file {path}"))?;
        return parse_server_config_password(&text);
    }

    Ok(non_empty_secret(args.token.as_deref()).map(ToString::to_string))
}

fn read_password_file(path: &str) -> Result<Option<String>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read Notes server password file {path}"))?;
    Ok(non_empty_secret(Some(text.trim())).map(ToString::to_string))
}

fn parse_server_config_password(text: &str) -> Result<Option<String>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    if trimmed.starts_with('{') {
        let value: Value =
            serde_json::from_str(trimmed).context("failed to parse Notes server JSON config")?;
        return Ok(value
            .get("password")
            .or_else(|| value.get("token"))
            .and_then(Value::as_str)
            .and_then(|secret| non_empty_secret(Some(secret)))
            .map(ToString::to_string));
    }

    let mut raw_lines = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        raw_lines.push(line);
        if let Some(value) =
            config_value_for_key(line, "password").or_else(|| config_value_for_key(line, "token"))
        {
            return Ok(non_empty_secret(Some(&value)).map(ToString::to_string));
        }
    }

    if raw_lines.len() == 1 {
        return Ok(non_empty_secret(raw_lines.first().copied()).map(ToString::to_string));
    }

    Err(anyhow!(
        "Notes server config must contain password or token as JSON, key=value, or a single raw password line"
    ))
}

fn config_value_for_key(line: &str, expected_key: &str) -> Option<String> {
    let (key, value) = line.split_once('=').or_else(|| line.split_once(':'))?;
    if key.trim() != expected_key {
        return None;
    }
    Some(strip_config_quotes(value.trim()).to_string())
}

fn strip_config_quotes(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
}

fn non_empty_secret(secret: Option<&str>) -> Option<&str> {
    secret.map(str::trim).filter(|secret| !secret.is_empty())
}

pub async fn get_open_api() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/yaml"),
        )],
        OPENAPI_YAML,
    )
}

pub async fn get_health() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": "apple-notes-server",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

pub async fn get_capabilities(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    helper_ok(&state, "helper.capabilities", json!({})).await
}

pub async fn list_accounts(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    helper_ok(&state, "accounts.list", json!({})).await
}

pub async fn list_folders(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<ListFoldersQuery>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    helper_ok(
        &state,
        "folders.list",
        compact_json(json!({ "account": query.account })),
    )
    .await
}

pub async fn create_folder(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<CreateFolderRequest>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    helper_ok(
        &state,
        "folders.create",
        compact_json(json!({
            "account": request.account,
            "name": request.name,
            "parent": request.parent
        })),
    )
    .await
}

pub async fn delete_folder(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<DeleteFolderRequest>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    helper_ok(
        &state,
        "folders.delete",
        compact_json(json!({
            "account": request.account,
            "name": request.name,
            "parent": request.parent
        })),
    )
    .await
}

pub async fn rename_folder(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<RenameFolderRequest>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    helper_ok(
        &state,
        "folders.rename",
        compact_json(json!({
            "account": request.account,
            "name": request.name,
            "newName": request.new_name,
            "parent": request.parent
        })),
    )
    .await
}

pub async fn list_notes(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<ListNotesQuery>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    helper_ok(
        &state,
        "notes.list",
        compact_json(json!({
            "account": query.account,
            "folder": query.folder,
            "sharedOnly": query.shared_only.unwrap_or(false)
        })),
    )
    .await
}

pub async fn create_note(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<WriteNoteRequest>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    let (params, temp_paths) = write_note_params(request).await?;
    let result = helper_ok(&state, "notes.create", params).await;
    schedule_temp_paths_cleanup(temp_paths);
    result
}

pub async fn search_notes(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<SearchNotesQuery>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    helper_ok(
        &state,
        "notes.search",
        compact_json(json!({
            "account": query.account,
            "query": query.query,
            "limit": query.limit.unwrap_or(0)
        })),
    )
    .await
}

pub async fn get_note(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(note_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    let note_id = decode_note_id_ref(&note_id)?;
    helper_ok(&state, "notes.get", json!({ "id": note_id })).await
}

pub async fn update_note(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(note_id): Path<String>,
    Json(request): Json<WriteNoteRequest>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    let note_id = decode_note_id_ref(&note_id)?;
    let (mut params, temp_paths) = write_note_params(request).await?;
    params["id"] = Value::String(note_id);
    let result = helper_ok(&state, "notes.update", params).await;
    schedule_temp_paths_cleanup(temp_paths);
    result
}

pub async fn delete_note(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(note_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    let note_id = decode_note_id_ref(&note_id)?;
    helper_ok(&state, "notes.delete", json!({ "id": note_id })).await
}

pub async fn move_note(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(note_id): Path<String>,
    Json(request): Json<MoveNoteRequest>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    let note_id = decode_note_id_ref(&note_id)?;
    helper_ok(
        &state,
        "notes.move",
        compact_json(json!({
            "id": note_id,
            "account": request.account,
            "folder": request.folder
        })),
    )
    .await
}

pub async fn show_note(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(note_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    let note_id = decode_note_id_ref(&note_id)?;
    helper_ok(&state, "notes.show", json!({ "id": note_id })).await
}

pub async fn list_attachments(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(note_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    let note_id = decode_note_id_ref(&note_id)?;
    helper_ok(&state, "attachments.list", json!({ "noteId": note_id })).await
}

pub async fn add_attachments(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(note_id): Path<String>,
    Json(request): Json<AddAttachmentsRequest>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    let note_id = decode_note_id_ref(&note_id)?;
    let (attachment_paths, temp_paths) = materialize_attachments(request.attachments).await?;
    let result = helper_ok(
        &state,
        "notes.update",
        json!({
            "id": note_id,
            "attachments": attachment_paths
        }),
    )
    .await;
    schedule_temp_paths_cleanup(temp_paths);
    result
}

pub async fn get_attachment_content(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(note_id): Path<String>,
    Query(query): Query<AttachmentContentQuery>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    let note_id = decode_note_id_ref(&note_id)?;
    if query.attachment_id.is_none() && query.name.is_none() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid.params",
            "provide attachmentId or name",
        ));
    }

    let out_dir = std::env::temp_dir().join(format!("apple-notes-server-read-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&out_dir).await.map_err(|error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("failed to create temp output dir: {error}"),
        )
    })?;

    let save_params = compact_json(json!({
        "noteId": note_id,
        "attachmentId": query.attachment_id,
        "name": query.name,
        "output": out_dir
    }));
    let mut save_result = helper_call(&state, "attachments.save", save_params.clone()).await;
    for _ in 0..5 {
        let should_retry = save_result
            .as_ref()
            .err()
            .is_some_and(|error| error.message.contains("no local file URL or readable data"));
        if !should_retry {
            break;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
        save_result = helper_call(&state, "attachments.save", save_params.clone()).await;
    }
    let result = match save_result {
        Ok(saved) => read_saved_attachment(saved).await,
        Err(error) => Err(error),
    };
    let _ = tokio::fs::remove_dir_all(&out_dir).await;
    result.map(ok)
}

pub async fn delete_attachment(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(note_id): Path<String>,
    Json(request): Json<DeleteAttachmentRequest>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    let note_id = decode_note_id_ref(&note_id)?;
    if request.attachment_id.is_none() && request.name.is_none() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid.params",
            "provide attachmentId or name",
        ));
    }
    helper_ok(
        &state,
        "attachments.delete",
        compact_json(json!({
            "noteId": note_id,
            "attachmentId": request.attachment_id,
            "name": request.name
        })),
    )
    .await
}

pub async fn share_note(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<ShareNoteRequest>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    let note_id = decode_note_id_ref(&request.note_id)?;
    let invitee = request.invitee.or(request.email).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid.params",
            "provide invitee or email",
        )
    })?;
    helper_ok(
        &state,
        "shares.create",
        compact_json(json!({
            "noteId": note_id,
            "invitee": invitee,
            "backend": request.backend,
            "timeout": request.timeout
        })),
    )
    .await
}

pub async fn accept_share(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<AcceptShareRequest>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    helper_ok(
        &state,
        "shares.accept",
        compact_json(json!({
            "url": request.url,
            "timeout": request.timeout
        })),
    )
    .await
}

pub async fn list_webhooks(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    let webhooks: Vec<_> = state.webhooks.read().await.values().cloned().collect();
    Ok(ok(json!(webhooks)))
}

pub async fn create_webhook(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<CreateWebhookRequest>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    let events = if request.events.is_empty() {
        vec![
            "note.created".to_string(),
            "note.updated".to_string(),
            "note.deleted".to_string(),
        ]
    } else {
        request.events
    };
    let subscription = WebhookSubscription {
        id: Uuid::new_v4().to_string(),
        url: request.url,
        has_secret: request.secret.is_some(),
        secret: request.secret,
        events,
    };
    state
        .webhooks
        .write()
        .await
        .insert(subscription.id.clone(), subscription.clone());
    Ok(ok(json!(subscription)))
}

pub async fn delete_webhook(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(webhook_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state, &headers)?;
    let removed = state.webhooks.write().await.remove(&webhook_id).is_some();
    Ok(ok(json!({ "deleted": removed })))
}

fn require_auth(state: &ServerState, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(expected_token) = state.token.as_deref() else {
        return Ok(());
    };
    let Some(auth_value) = headers.get(header::AUTHORIZATION) else {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing Authorization bearer token",
        ));
    };
    let auth_text = auth_value.to_str().unwrap_or_default();
    if auth_text == format!("Bearer {expected_token}") {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "invalid Authorization bearer token",
        ))
    }
}

async fn helper_ok(
    state: &ServerState,
    op: &'static str,
    params: Value,
) -> Result<Json<Value>, ApiError> {
    helper_call(state, op, params).await.map(ok)
}

fn ok(result: Value) -> Json<Value> {
    Json(json!({
        "ok": true,
        "result": result
    }))
}

fn decode_note_id_ref(note_id: &str) -> Result<String, ApiError> {
    let Some(encoded) = note_id
        .strip_prefix("b64:")
        .or_else(|| note_id.strip_prefix("base64url:"))
    else {
        return Ok(note_id.to_string());
    };

    let bytes = BASE64_URL_SAFE_NO_PAD
        .decode(encoded)
        .or_else(|_| BASE64_URL_SAFE.decode(encoded))
        .map_err(|error| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid.params",
                format!("invalid base64url note id: {error}"),
            )
        })?;
    String::from_utf8(bytes).map_err(|error| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid.params",
            format!("note id is not valid UTF-8: {error}"),
        )
    })
}

async fn helper_call(
    state: &ServerState,
    op: &'static str,
    params: Value,
) -> Result<Value, ApiError> {
    let _guard = state.helper_lock.lock().await;
    helper_call_unlocked(state, op, params).await
}

async fn helper_call_unlocked(
    state: &ServerState,
    op: &'static str,
    params: Value,
) -> Result<Value, ApiError> {
    let helper_path = state.helper_path.clone();
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || helper_call_blocking(helper_path, backend, op, params))
        .await
        .map_err(|error| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                format!("helper task failed: {error}"),
            )
        })?
}

fn helper_call_blocking(
    helper_path: PathBuf,
    backend: String,
    op: &'static str,
    params: Value,
) -> Result<Value, ApiError> {
    let id = Uuid::new_v4().to_string();
    let request = json!({
        "id": id,
        "version": 1,
        "op": op,
        "params": params
    });
    let request_line = serde_json::to_string(&request).map_err(|error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("failed to encode helper request: {error}"),
        )
    })?;

    let mut child = Command::new(&helper_path)
        .arg("--stdio")
        .arg("--backend")
        .arg(&backend)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "helper.spawn_failed",
                format!("failed to spawn {}: {error}", helper_path.display()),
            )
        })?;

    {
        let stdin = child.stdin.as_mut().ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "helper.stdin_unavailable",
                "helper stdin unavailable",
            )
        })?;
        writeln!(stdin, "{request_line}").map_err(|error| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "helper.write_failed",
                format!("failed to write helper request: {error}"),
            )
        })?;
    }
    drop(child.stdin.take());

    let output = child.wait_with_output().map_err(|error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "helper.wait_failed",
            format!("failed to wait for helper: {error}"),
        )
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() && stdout.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "helper.failed",
            stderr.trim().to_string(),
        ));
    }

    let line = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "helper.empty_response",
                format!("helper returned no response: {}", stderr.trim()),
            )
        })?;
    let response: Value = serde_json::from_str(line).map_err(|error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "helper.invalid_response",
            format!("failed to parse helper response: {error}: {line}"),
        )
    })?;

    if response.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    } else {
        Err(ApiError::from_helper(
            response.get("error").unwrap_or(&Value::Null),
        ))
    }
}

async fn write_note_params(request: WriteNoteRequest) -> Result<(Value, Vec<PathBuf>), ApiError> {
    let (attachment_paths, temp_paths) = materialize_attachments(request.attachments).await?;
    let title = request.title.or(request.name);
    let html = request.html.or(request.body);
    Ok((
        compact_json(json!({
            "account": request.account,
            "folder": request.folder,
            "title": title,
            "html": html,
            "attachments": attachment_paths
        })),
        temp_paths,
    ))
}

async fn materialize_attachments(
    attachments: Vec<AttachmentInput>,
) -> Result<(Vec<String>, Vec<PathBuf>), ApiError> {
    let mut paths = Vec::new();
    let mut temp_paths = Vec::new();

    for attachment in attachments {
        if let Some(path) = attachment.path {
            paths.push(path);
            continue;
        }

        let Some(data_base64) = attachment.data_base64 else {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid.params",
                "attachment must include path or dataBase64",
            ));
        };
        let bytes = BASE64.decode(data_base64).map_err(|error| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid.params",
                format!("invalid attachment dataBase64: {error}"),
            )
        })?;
        let file_name = attachment_file_name(attachment.name, attachment.mime_type.as_deref());
        let attachment_dir =
            temp_attachment_dir().join(format!("apple-notes-server-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&attachment_dir)
            .await
            .map_err(|error| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    format!("failed to create temporary attachment dir: {error}"),
                )
            })?;
        let path = attachment_dir.join(file_name);
        tokio::fs::write(&path, bytes).await.map_err(|error| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                format!("failed to write temporary attachment: {error}"),
            )
        })?;
        paths.push(path.to_string_lossy().to_string());
        temp_paths.push(attachment_dir);
    }

    Ok((paths, temp_paths))
}

fn attachment_file_name(name: Option<String>, mime_type: Option<&str>) -> String {
    let mut name = name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "attachment".to_string());
    name = sanitize_file_name(&name);
    if FsPath::new(&name).extension().is_none() {
        if let Some(extension) = mime_type
            .and_then(mime_guess::get_mime_extensions_str)
            .and_then(|extensions| extensions.first())
        {
            name.push('.');
            name.push_str(extension);
        }
    }
    name
}

fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    cleaned.trim_matches('.').chars().take(120).collect()
}

async fn cleanup_temp_paths(paths: Vec<PathBuf>) {
    for path in paths {
        if path.is_dir() {
            let _ = tokio::fs::remove_dir_all(path).await;
        } else {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}

fn schedule_temp_paths_cleanup(paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(temp_attachment_ttl()).await;
        cleanup_temp_paths(paths).await;
    });
}

fn temp_attachment_ttl() -> Duration {
    std::env::var("APPLE_NOTES_SERVER_TEMP_ATTACHMENT_TTL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(600))
}

fn temp_attachment_dir() -> PathBuf {
    std::env::var("APPLE_NOTES_SERVER_TEMP_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

async fn read_saved_attachment(saved: Value) -> Result<Value, ApiError> {
    if let Some(data_base64) = saved.get("dataBase64").and_then(Value::as_str) {
        let size = saved
            .get("size")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| {
                BASE64
                    .decode(data_base64)
                    .map(|bytes| bytes.len() as u64)
                    .unwrap_or(0)
            });
        return Ok(json!({
            "name": saved.get("name").and_then(Value::as_str).unwrap_or("attachment"),
            "contentType": saved.get("contentType").and_then(Value::as_str).unwrap_or("application/octet-stream"),
            "size": size,
            "dataBase64": data_base64
        }));
    }

    let path = saved
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "helper.invalid_response",
                "attachment save response missing path",
            )
        })?
        .to_string();
    let bytes = tokio::fs::read(&path).await.map_err(|error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("failed to read saved attachment {path}: {error}"),
        )
    })?;
    let name = FsPath::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment")
        .to_string();
    let content_type = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .to_string();
    Ok(json!({
        "name": name,
        "contentType": content_type,
        "size": bytes.len(),
        "dataBase64": BASE64.encode(bytes)
    }))
}

fn compact_json(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter_map(|(key, value)| {
                    if value.is_null() {
                        None
                    } else if matches!(&value, Value::Array(items) if items.is_empty()) {
                        None
                    } else {
                        Some((key, value))
                    }
                })
                .collect(),
        ),
        other => other,
    }
}

async fn webhook_poll_loop(state: Arc<ServerState>) {
    let mut interval = tokio::time::interval(state.poll_interval);
    let mut previous: Option<HashMap<String, Value>> = None;

    loop {
        interval.tick().await;
        if state.webhooks.read().await.is_empty() {
            previous = None;
            continue;
        }

        let snapshot = match notes_snapshot_if_idle(&state).await {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => continue,
            Err(error) => {
                eprintln!("notes webhook poll failed: {}", error.message);
                continue;
            }
        };

        if let Some(previous_snapshot) = previous.as_ref() {
            dispatch_snapshot_changes(&state, previous_snapshot, &snapshot).await;
        }
        previous = Some(snapshot);
    }
}

async fn notes_snapshot_if_idle(
    state: &ServerState,
) -> Result<Option<HashMap<String, Value>>, ApiError> {
    let Ok(_guard) = state.helper_lock.try_lock() else {
        return Ok(None);
    };
    let result = helper_call_unlocked(state, "notes.list", json!({})).await?;
    let notes = result.as_array().ok_or_else(|| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "helper.invalid_response",
            "notes.list did not return an array",
        )
    })?;
    Ok(Some(
        notes
            .iter()
            .filter_map(|note| {
                note.get("id")
                    .and_then(Value::as_str)
                    .map(|id| (id.to_string(), note.clone()))
            })
            .collect(),
    ))
}

async fn dispatch_snapshot_changes(
    state: &ServerState,
    previous: &HashMap<String, Value>,
    current: &HashMap<String, Value>,
) {
    let previous_ids: HashSet<_> = previous.keys().cloned().collect();
    let current_ids: HashSet<_> = current.keys().cloned().collect();

    for id in current_ids.difference(&previous_ids) {
        dispatch_webhook_event(
            state,
            "note.created",
            json!({
                "event": "note.created",
                "noteId": id,
                "current": current.get(id),
                "observedAt": unix_timestamp()
            }),
        )
        .await;
    }

    for id in previous_ids.difference(&current_ids) {
        dispatch_webhook_event(
            state,
            "note.deleted",
            json!({
                "event": "note.deleted",
                "noteId": id,
                "previous": previous.get(id),
                "observedAt": unix_timestamp()
            }),
        )
        .await;
    }

    for id in current_ids.intersection(&previous_ids) {
        let before = previous.get(id);
        let after = current.get(id);
        if before != after {
            dispatch_webhook_event(
                state,
                "note.updated",
                json!({
                    "event": "note.updated",
                    "noteId": id,
                    "previous": before,
                    "current": after,
                    "observedAt": unix_timestamp()
                }),
            )
            .await;
        }
    }
}

async fn dispatch_webhook_event(state: &ServerState, event: &str, payload: Value) {
    let subscriptions: Vec<_> = state.webhooks.read().await.values().cloned().collect();
    for subscription in subscriptions {
        if !subscription.events.iter().any(|wanted| wanted == event) {
            continue;
        }

        let mut request = state.http_client.post(&subscription.url).json(&payload);
        if let Some(secret) = subscription.secret.as_deref() {
            request = request.header("X-Apple-Notes-Webhook-Secret", secret);
        }

        if let Err(error) = request.send().await {
            eprintln!(
                "notes webhook delivery failed for {} to {}: {error}",
                subscription.id, subscription.url
            );
        }
    }
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server_args() -> crate::NotesServerArgs {
        crate::NotesServerArgs {
            bind: "127.0.0.1:3768".to_string(),
            backend: "private".to_string(),
            helper: "apple-notes-helper".to_string(),
            poll_interval: 10,
            token: None,
            password: None,
            password_file: None,
            config: None,
            allow_unauthenticated: false,
        }
    }

    #[test]
    fn parse_server_config_reads_json_password() {
        let parsed = parse_server_config_password(r#"{ "password": "from-json" }"#).unwrap();
        assert_eq!(parsed.as_deref(), Some("from-json"));
    }

    #[test]
    fn parse_server_config_reads_key_value_password() {
        let parsed = parse_server_config_password("password = \"from-kv\"\n").unwrap();
        assert_eq!(parsed.as_deref(), Some("from-kv"));
    }

    #[test]
    fn explicit_password_takes_precedence_over_config_and_legacy_token() {
        let dir = std::env::temp_dir().join(format!("apple-notes-server-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("notes-server.conf");
        std::fs::write(&config, "password = from-config\n").unwrap();

        let mut args = server_args();
        args.token = Some("legacy-token".to_string());
        args.password = Some("from-cli".to_string());
        args.config = Some(config.to_string_lossy().to_string());

        let resolved = resolve_server_auth_secret(&args).unwrap();
        assert_eq!(resolved.as_deref(), Some("from-cli"));

        std::fs::remove_dir_all(dir).unwrap();
    }
}
