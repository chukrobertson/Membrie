// SPDX-License-Identifier: AGPL-3.0-or-later

use anyhow::{Context, Result, anyhow, bail};
use gio::prelude::*;
use glib::variant::ToVariant;
use membrie_core::{
    AttachmentBatchImport, DaemonClient, MobileUsageSummary, NewRemembrie, Remembrie,
    StagedAttachmentImport, attachment_blob_path, attachment_inbox_dir,
    mobile_pairing_invitation_path, mobile_tailnet_host_path, mobile_token_path, socket_path,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const LISTEN_ADDRESS: &str = "127.0.0.1:47381";
const WORKERS: usize = 4;
const MAX_REQUEST_BYTES: u64 = 300 * 1024;
const MAX_ATTACHMENT_BYTES: u64 = 100 * 1024 * 1024;
const MAX_ATTACHMENTS_PER_REMEMBRIE: usize = 8;
const MAX_NOTE_CHARACTERS: usize = 20_000;
const MAX_CLIPBOARD_BYTES: usize = 256 * 1024;
const MAX_CLIPBOARD_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const TOKEN_BYTES: usize = 32;
const DBUS_NAME: &str = "com.chuk.Membrie.Clipboard";
const DBUS_PATH: &str = "/com/chuk/Membrie/Clipboard";
const DBUS_INTERFACE: &str = "com.chuk.Membrie.Clipboard";

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_CSS: &str = include_str!("../web/app.css");
const APP_JS: &str = include_str!("../web/app.js");

#[derive(Clone)]
struct AppState {
    client: DaemonClient,
    token: Arc<String>,
    tailnet_host: Option<Arc<String>>,
    pairing_lock: Arc<Mutex<()>>,
}

#[derive(Deserialize)]
struct NoteInput {
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
}

#[derive(Deserialize)]
struct AttachmentInput {
    #[serde(default)]
    upload_id: Option<String>,
    #[serde(default)]
    upload_ids: Vec<String>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
}

#[derive(Deserialize)]
struct UsageInput {
    active_ms: u64,
    #[serde(default)]
    opened: bool,
}

#[derive(Deserialize)]
struct CancelUploadsInput {
    upload_ids: Vec<String>,
}

#[derive(Deserialize)]
struct BrieInput {
    question: String,
}

#[derive(Deserialize)]
struct ClipboardInput {
    text: String,
}

#[derive(Deserialize)]
struct AttachmentCorrectionInput {
    remembrie_id: String,
    content_id: String,
    correction: String,
}

#[derive(Deserialize)]
struct AttachmentActionInput {
    remembrie_id: String,
    content_id: String,
}

#[derive(Deserialize)]
struct RemembranceActionInput {
    id: String,
}

#[derive(Deserialize)]
struct PairingInput {
    code: String,
}

#[derive(Serialize)]
struct ApiMessage<'a> {
    message: &'a str,
}

#[derive(Serialize)]
struct MobileStatus {
    paused: bool,
    locked: bool,
    allow_while_locked: bool,
    remembrie_count: u64,
    calendar_event_count: u64,
    usage: MobileUsageSummary,
}

#[derive(Serialize)]
struct MobileRemembrance {
    id: String,
    kind: String,
    title: String,
    source: String,
    occurred_at_ms: i64,
    preview: String,
    attachments: Vec<MobileAttachment>,
}

#[derive(Serialize)]
struct MobileAttachment {
    content_id: String,
    original_name: String,
    mime_type: String,
    byte_size: u64,
    analysis_state: String,
    analysis_text: Option<String>,
    user_correction: Option<String>,
    analysis_model: Option<String>,
    analysis_confidence: Option<String>,
}

#[derive(Serialize)]
struct RecallResponse {
    recent: Vec<MobileRemembrance>,
    upcoming: Vec<MobileRemembrance>,
}

#[derive(Serialize)]
struct MobileCitation {
    number: u32,
    id: String,
    kind: String,
    title: String,
    source: String,
    occurred_at_ms: i64,
    excerpt: String,
}

#[derive(Serialize)]
struct BrieResponse {
    answer: String,
    model: String,
    citations: Vec<MobileCitation>,
}

#[derive(Serialize)]
struct ClipboardResponse {
    text: String,
}

#[derive(Serialize)]
struct PairingResponse<'a> {
    token: &'a str,
}

#[derive(Serialize)]
struct UploadResponse {
    upload_id: String,
}

struct PreparedResponse {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl PreparedResponse {
    fn text(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type,
            body: body.into(),
        }
    }

    fn json<T: Serialize>(status: u16, value: &T) -> Self {
        match serde_json::to_vec(value) {
            Ok(body) => Self::text(status, "application/json; charset=utf-8", body),
            Err(_) => Self::error(500, "The local response could not be encoded"),
        }
    }

    fn error(status: u16, message: &'static str) -> Self {
        Self::json(status, &ApiMessage { message })
    }

    fn into_http(self) -> Response<Cursor<Vec<u8>>> {
        let mut response = Response::from_data(self.body)
            .with_status_code(StatusCode(self.status))
            .with_header(header("Content-Type", self.content_type));
        for (name, value) in [
            ("Cache-Control", "no-store"),
            ("X-Content-Type-Options", "nosniff"),
            ("Referrer-Policy", "no-referrer"),
            ("X-Frame-Options", "DENY"),
            (
                "Permissions-Policy",
                "camera=(), microphone=(self), geolocation=(), payment=()",
            ),
            (
                "Content-Security-Policy",
                "default-src 'self'; connect-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; style-src 'self'; script-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
            ),
        ] {
            response.add_header(header(name, value));
        }
        response
    }
}

fn main() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    if let Some(argument) = arguments.next() {
        if argument != "--trust-tailnet-host" {
            bail!("unknown Mobile Companion option: {argument}");
        }
        let host = arguments
            .next()
            .ok_or_else(|| anyhow!("--trust-tailnet-host needs an exact hostname"))?;
        if arguments.next().is_some() {
            bail!("--trust-tailnet-host accepts exactly one hostname");
        }
        let host = normalize_tailnet_host(&host)?;
        save_tailnet_host(&host)?;
        println!("Membrie trusts the exact tailnet origin https://{host}");
        return Ok(());
    }

    if let Err(error) = cleanup_stale_uploads() {
        eprintln!("could not clean old staged attachments: {error:#}");
    }
    let token = Arc::new(load_or_create_token()?);
    let tailnet_host = load_tailnet_host()?.map(Arc::new);
    let state = AppState {
        client: DaemonClient::new(socket_path()),
        token,
        tailnet_host,
        pairing_lock: Arc::new(Mutex::new(())),
    };
    let server = Arc::new(
        Server::http(LISTEN_ADDRESS)
            .map_err(|error| anyhow!("could not bind the local mobile companion: {error}"))?,
    );
    println!("Membrie Mobile Companion ready at http://{LISTEN_ADDRESS}");
    println!("  listener: loopback only");
    if let Some(host) = state.tailnet_host.as_deref() {
        println!("  trusted tailnet origin: https://{host}");
    } else {
        println!("  trusted tailnet origin: none");
    }
    println!("  authentication: pairing token required");

    let mut workers = Vec::with_capacity(WORKERS);
    for _ in 0..WORKERS {
        let server = Arc::clone(&server);
        let state = state.clone();
        workers.push(thread::spawn(move || {
            while let Ok(request) = server.recv() {
                handle_request(request, &state);
            }
        }));
    }
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

fn handle_request(mut request: Request, state: &AppState) {
    let response = route(&mut request, state).unwrap_or_else(|error| {
        eprintln!("Mobile Companion request failed: {error:#}");
        PreparedResponse::error(500, "The local request could not be completed")
    });
    let _ = request.respond(response.into_http());
}

fn route(request: &mut Request, state: &AppState) -> Result<PreparedResponse> {
    let path = request.url().split('?').next().unwrap_or("/").to_owned();
    if !remote_is_loopback(request.remote_addr()) {
        return Ok(PreparedResponse::error(
            403,
            "Direct network access is disabled",
        ));
    }
    if !host_is_trusted(request, state.tailnet_host.as_deref().map(String::as_str))
        || !origin_is_trusted(request, state.tailnet_host.as_deref().map(String::as_str))
    {
        return Ok(PreparedResponse::error(
            403,
            "This local request was not trusted",
        ));
    }

    match (request.method(), path.as_str()) {
        (&Method::Get, "/") | (&Method::Get, "/index.html") => Ok(PreparedResponse::text(
            200,
            "text/html; charset=utf-8",
            INDEX_HTML.as_bytes().to_vec(),
        )),
        (&Method::Get, "/app.css") => Ok(PreparedResponse::text(
            200,
            "text/css; charset=utf-8",
            APP_CSS.as_bytes().to_vec(),
        )),
        (&Method::Get, "/app.js") => Ok(PreparedResponse::text(
            200,
            "text/javascript; charset=utf-8",
            APP_JS.as_bytes().to_vec(),
        )),
        (&Method::Post, "/api/pair") => pair_device(request, state),
        (_, path) if path.starts_with("/api/") => route_api(request, state, path),
        _ => Ok(PreparedResponse::error(404, "Not found")),
    }
}

fn pair_device(request: &mut Request, state: &AppState) -> Result<PreparedResponse> {
    match state.client.status() {
        Ok(status) if status.mobile_enabled => {}
        Ok(_) => {
            return Ok(PreparedResponse::error(
                403,
                "Mobile Companion access is disabled on the PC",
            ));
        }
        Err(_) => return Ok(PreparedResponse::error(503, "Membrie is not available")),
    }
    let input: PairingInput = match read_json(request) {
        Ok(input) => input,
        Err(response) => return Ok(response),
    };
    let code = input.code.trim();
    if code.len() != 8 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        thread::sleep(Duration::from_millis(500));
        return Ok(PreparedResponse::error(
            401,
            "That temporary pairing code is not valid",
        ));
    }

    let _pairing_guard = state
        .pairing_lock
        .lock()
        .map_err(|_| anyhow!("the pairing lock was poisoned"))?;
    let path = mobile_pairing_invitation_path();
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() && metadata.len() <= 128 => {}
        _ => {
            thread::sleep(Duration::from_millis(500));
            return Ok(PreparedResponse::error(
                401,
                "Create a fresh pairing code in Membrie on the PC",
            ));
        }
    }
    let invitation = fs::read_to_string(&path)?;
    let Some((expected_code, expires_at_ms)) = parse_pairing_invitation(&invitation) else {
        let _ = fs::remove_file(&path);
        return Ok(PreparedResponse::error(
            401,
            "Create a fresh pairing code in Membrie on the PC",
        ));
    };
    if expires_at_ms < current_time_ms() {
        let _ = fs::remove_file(&path);
        return Ok(PreparedResponse::error(
            401,
            "That pairing code expired; create a fresh code on the PC",
        ));
    }
    if !constant_time_equal(code.as_bytes(), expected_code.as_bytes()) {
        thread::sleep(Duration::from_millis(500));
        return Ok(PreparedResponse::error(
            401,
            "That temporary pairing code is not valid",
        ));
    }
    fs::remove_file(&path)?;
    Ok(PreparedResponse::json(
        200,
        &PairingResponse {
            token: state.token.as_str(),
        },
    ))
}

fn parse_pairing_invitation(value: &str) -> Option<(&str, i64)> {
    let mut lines = value.lines();
    let code = lines.next()?.trim();
    let expires_at_ms = lines.next()?.trim().parse().ok()?;
    if code.len() != 8
        || !code.bytes().all(|byte| byte.is_ascii_digit())
        || lines.any(|line| !line.trim().is_empty())
    {
        return None;
    }
    Some((code, expires_at_ms))
}

fn route_api(request: &mut Request, state: &AppState, path: &str) -> Result<PreparedResponse> {
    if !authorized(request, &state.token) {
        thread::sleep(Duration::from_millis(250));
        return Ok(PreparedResponse::error(
            401,
            "Pair this device with Membrie",
        ));
    }

    let status = match state.client.status() {
        Ok(status) => status,
        Err(_) => return Ok(PreparedResponse::error(503, "Membrie is not available")),
    };
    if !status.mobile_enabled {
        return Ok(PreparedResponse::error(
            403,
            "Mobile Companion access is disabled on the PC",
        ));
    }
    let locked = desktop_locked().unwrap_or(true);
    let read_allowed = mobile_read_allowed(locked, status.mobile_allow_while_locked);

    match (request.method(), path) {
        (&Method::Get, "/api/status") => Ok(PreparedResponse::json(
            200,
            &MobileStatus {
                paused: status.paused,
                locked,
                allow_while_locked: status.mobile_allow_while_locked,
                remembrie_count: status.remembrie_count,
                calendar_event_count: status.calendar_event_count,
                usage: state.client.mobile_usage_summary()?,
            },
        )),
        (&Method::Post, "/api/note") => create_note(request, state),
        (&Method::Post, "/api/upload") => upload_attachment(request),
        (&Method::Post, "/api/uploads/cancel") => cancel_uploads(request),
        (&Method::Post, "/api/attachment") => create_attachment(request, state),
        (&Method::Post, "/api/usage") => record_mobile_usage(request, state),
        _ if !read_allowed => Ok(PreparedResponse::error(
            423,
            "The PC is locked; enable paired-device recall in Membrie to continue",
        )),
        (&Method::Get, "/api/recall") => recall(state),
        (&Method::Post, "/api/brie") => ask_brie(request, state),
        (&Method::Get, "/api/clipboard/image") => read_clipboard_image(),
        (&Method::Get, "/api/clipboard") => read_clipboard(),
        (&Method::Post, "/api/clipboard") => write_clipboard(request),
        (&Method::Post, "/api/attachment/correct") => correct_attachment(request, state),
        (&Method::Post, "/api/attachment/retry") => retry_attachment(request, state),
        (&Method::Post, "/api/attachment/delete") => delete_attachment(request, state),
        (&Method::Post, "/api/remembrance/delete") => delete_remembrance(request, state),
        (&Method::Get, path) if path.starts_with("/api/attachment/") => {
            read_attachment(state, path)
        }
        _ => Ok(PreparedResponse::error(404, "Not found")),
    }
}

fn create_note(request: &mut Request, state: &AppState) -> Result<PreparedResponse> {
    let input: NoteInput = match read_json(request) {
        Ok(input) => input,
        Err(response) => return Ok(response),
    };
    let title = input.title.trim();
    let body = input.body.trim();
    if title.is_empty() && body.is_empty() {
        return Ok(PreparedResponse::error(
            422,
            "Write something to remember first",
        ));
    }
    if title.chars().count() > 300 || body.chars().count() > MAX_NOTE_CHARACTERS {
        return Ok(PreparedResponse::error(413, "That note is too large"));
    }
    state
        .client
        .create(NewRemembrie::mobile_note(title, body))
        .context("could not store the mobile note")?;
    Ok(PreparedResponse::json(
        201,
        &ApiMessage {
            message: "Remembrie saved locally",
        },
    ))
}

fn upload_attachment(request: &mut Request) -> Result<PreparedResponse> {
    if request
        .body_length()
        .is_some_and(|length| length as u64 > MAX_ATTACHMENT_BYTES)
    {
        return Ok(PreparedResponse::error(
            413,
            "That attachment is larger than 100 MiB",
        ));
    }
    let original_name = header_value(request, "x-membrie-filename")
        .and_then(percent_decode)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| anyhow!("the attachment filename was missing"))?;
    if original_name.chars().count() > 255 {
        return Ok(PreparedResponse::error(
            422,
            "That attachment filename is too long",
        ));
    }
    if original_name
        .chars()
        .any(|character| character.is_control())
    {
        return Ok(PreparedResponse::error(
            422,
            "That attachment filename contains unsupported characters",
        ));
    }
    let declared_mime_type = header_value(request, "content-type")
        .map(str::trim)
        .filter(|value| value.len() <= 127 && !value.chars().any(char::is_control))
        .unwrap_or("application/octet-stream")
        .to_owned();
    let upload_id = random_hex_identifier()?;
    let inbox = attachment_inbox_dir();
    fs::create_dir_all(&inbox)?;
    #[cfg(unix)]
    fs::set_permissions(&inbox, fs::Permissions::from_mode(0o700))?;
    let path = inbox.join(&upload_id);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    let copied = std::io::copy(
        &mut request.as_reader().take(MAX_ATTACHMENT_BYTES + 1),
        &mut file,
    )?;
    if copied == 0 || copied > MAX_ATTACHMENT_BYTES {
        let _ = fs::remove_file(&path);
        return Ok(PreparedResponse::error(
            413,
            "Attachments must contain data and be no larger than 100 MiB",
        ));
    }
    file.sync_all()?;
    let metadata_result = (|| -> Result<()> {
        let metadata_path = inbox.join(format!("{upload_id}.name"));
        let mut metadata_options = OpenOptions::new();
        metadata_options.write(true).create_new(true);
        #[cfg(unix)]
        metadata_options.mode(0o600);
        let mut metadata = metadata_options.open(&metadata_path)?;
        writeln!(metadata, "{original_name}")?;
        writeln!(metadata, "{declared_mime_type}")?;
        metadata.sync_all()?;
        Ok(())
    })();
    if let Err(error) = metadata_result {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok(PreparedResponse::json(201, &UploadResponse { upload_id }))
}

fn create_attachment(request: &mut Request, state: &AppState) -> Result<PreparedResponse> {
    let mut input: AttachmentInput = match read_json(request) {
        Ok(input) => input,
        Err(response) => return Ok(response),
    };
    if input.upload_ids.is_empty()
        && let Some(upload_id) = input.upload_id.take()
    {
        input.upload_ids.push(upload_id);
    }
    if input.upload_ids.is_empty()
        || input.upload_ids.len() > MAX_ATTACHMENTS_PER_REMEMBRIE
        || input.upload_ids.iter().any(|upload_id| {
            upload_id.len() != 32
                || !upload_id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        || input.upload_ids.iter().collect::<HashSet<_>>().len() != input.upload_ids.len()
    {
        return Ok(PreparedResponse::error(
            422,
            "The pasted attachment set is invalid",
        ));
    }
    if input.title.chars().count() > 300 || input.body.chars().count() > MAX_NOTE_CHARACTERS {
        return Ok(PreparedResponse::error(
            413,
            "That attachment note is too large",
        ));
    }
    let inbox = attachment_inbox_dir();
    let mut staged = Vec::with_capacity(input.upload_ids.len());
    for upload_id in &input.upload_ids {
        let staged_path = inbox.join(upload_id);
        let metadata_path = inbox.join(format!("{upload_id}.name"));
        let metadata = fs::read_to_string(&metadata_path)
            .context("the staged attachment metadata was unavailable")?;
        let mut metadata_lines = metadata.lines();
        let original_name = metadata_lines.next().unwrap_or("").trim().to_owned();
        let declared_mime_type = metadata_lines
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        staged.push(StagedAttachmentImport {
            staged_path: staged_path.display().to_string(),
            original_name,
            declared_mime_type,
        });
    }
    let imported = state.client.import_attachments(AttachmentBatchImport {
        attachments: staged,
        title: input.title,
        note: input.body,
        source_app: "Membrie Companion".to_owned(),
        window_title: Some("Mobile WebUI".to_owned()),
    });
    for upload_id in &input.upload_ids {
        let _ = fs::remove_file(inbox.join(format!("{upload_id}.name")));
    }
    match imported {
        Ok((_remembrie, attachments)) => {
            let pending = attachments
                .iter()
                .filter(|attachment| attachment.analysis_state == "pending")
                .count();
            let message = match (attachments.len(), pending) {
                (1, 1) => "Attachment saved; local understanding is queued".to_owned(),
                (1, _) => "Attachment saved; its original is retained locally".to_owned(),
                (count, 0) => {
                    format!("{count} attachments saved; their originals are retained locally")
                }
                (count, pending) => format!(
                    "{count} attachments saved; local understanding is queued for {pending} item{}",
                    if pending == 1 { "" } else { "s" }
                ),
            };
            Ok(PreparedResponse::json(
                201,
                &ApiMessage { message: &message },
            ))
        }
        Err(error) => {
            for upload_id in &input.upload_ids {
                let _ = fs::remove_file(inbox.join(upload_id));
            }
            Err(error.into())
        }
    }
}

fn cancel_uploads(request: &mut Request) -> Result<PreparedResponse> {
    let input: CancelUploadsInput = match read_json(request) {
        Ok(input) => input,
        Err(response) => return Ok(response),
    };
    if input.upload_ids.len() > MAX_ATTACHMENTS_PER_REMEMBRIE
        || input.upload_ids.iter().any(|upload_id| {
            upload_id.len() != 32
                || !upload_id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    {
        return Ok(PreparedResponse::error(
            422,
            "The staged attachment set is invalid",
        ));
    }
    let inbox = attachment_inbox_dir();
    for upload_id in input.upload_ids {
        let _ = fs::remove_file(inbox.join(&upload_id));
        let _ = fs::remove_file(inbox.join(format!("{upload_id}.name")));
    }
    Ok(PreparedResponse::json(
        200,
        &ApiMessage {
            message: "Staged attachments discarded",
        },
    ))
}

fn record_mobile_usage(request: &mut Request, state: &AppState) -> Result<PreparedResponse> {
    let input: UsageInput = match read_json(request) {
        Ok(input) => input,
        Err(response) => return Ok(response),
    };
    if input.active_ms > 60_000 || (input.active_ms == 0 && !input.opened) {
        return Ok(PreparedResponse::error(422, "That usage pulse is invalid"));
    }
    let summary = state
        .client
        .record_mobile_usage(input.active_ms, input.opened)?;
    Ok(PreparedResponse::json(200, &summary))
}

fn recall(state: &AppState) -> Result<PreparedResponse> {
    let snapshot = state.client.recall_snapshot(8, 6)?;
    let recent = snapshot
        .recent
        .into_iter()
        .map(|record| mobile_remembrance(record, state))
        .collect::<Result<Vec<_>>>()?;
    let upcoming = snapshot
        .upcoming
        .into_iter()
        .map(|record| mobile_remembrance(record, state))
        .collect::<Result<Vec<_>>>()?;
    Ok(PreparedResponse::json(
        200,
        &RecallResponse { recent, upcoming },
    ))
}

fn ask_brie(request: &mut Request, state: &AppState) -> Result<PreparedResponse> {
    let input: BrieInput = match read_json(request) {
        Ok(input) => input,
        Err(response) => return Ok(response),
    };
    let question = input.question.trim();
    if question.is_empty() || question.chars().count() > 1_000 {
        return Ok(PreparedResponse::error(
            422,
            "Ask Brie one concise question",
        ));
    }
    let answer = state.client.ask_brie(question)?;
    Ok(PreparedResponse::json(
        200,
        &BrieResponse {
            answer: answer.answer,
            model: answer.model,
            citations: answer
                .citations
                .into_iter()
                .map(|citation| MobileCitation {
                    number: citation.number,
                    id: citation.remembrie.id,
                    kind: citation.remembrie.kind,
                    title: citation.remembrie.title,
                    source: citation
                        .remembrie
                        .source_app
                        .unwrap_or_else(|| "Unknown local source".to_owned()),
                    occurred_at_ms: citation.remembrie.occurred_at_ms,
                    excerpt: truncate(&citation.excerpt, 320),
                })
                .collect(),
        },
    ))
}

fn read_clipboard() -> Result<PreparedResponse> {
    let text = bridge_read_clipboard()?;
    if text.trim().is_empty() {
        return Ok(PreparedResponse::error(404, "The PC clipboard is empty"));
    }
    if let Some(reason) = membrie_core::policy::sensitive_reason(&text) {
        eprintln!("Mobile clipboard share blocked: {reason}");
        return Ok(PreparedResponse::error(
            422,
            "That clipboard looks sensitive and was not shared",
        ));
    }
    Ok(PreparedResponse::json(200, &ClipboardResponse { text }))
}

fn read_clipboard_image() -> Result<PreparedResponse> {
    let (mime_type, content) = match bridge_read_clipboard_image() {
        Ok(image) => image,
        Err(error) => {
            eprintln!("Mobile clipboard image fetch unavailable: {error:#}");
            let (status, message) = clipboard_image_bridge_error(&error.to_string());
            return Ok(PreparedResponse::error(status, message));
        }
    };
    if content.is_empty() || content.len() > MAX_CLIPBOARD_IMAGE_BYTES {
        return Ok(PreparedResponse::error(
            413,
            "The PC clipboard image is empty or too large",
        ));
    }
    let Some(content_type) = verified_clipboard_image_type(&mime_type, &content) else {
        return Ok(PreparedResponse::error(
            415,
            "The PC clipboard image format could not be verified",
        ));
    };
    Ok(PreparedResponse::text(200, content_type, content))
}

fn clipboard_image_bridge_error(error: &str) -> (u16, &'static str) {
    let lower = error.to_ascii_lowercase();
    if lower.contains("noimage") || lower.contains("does not contain a supported image") {
        return (
            404,
            "The PC clipboard currently contains no supported image; copy the image itself, not only its file",
        );
    }
    if lower.contains("busy") || lower.contains("already being read") {
        return (409, "The PC clipboard is busy; wait a moment and try again");
    }
    if lower.contains("timeout") || lower.contains("timed out") {
        return (504, "The PC took too long to provide that clipboard image");
    }
    if lower.contains("invalidimage") || lower.contains("empty or too large") {
        return (422, "The PC clipboard image is empty or larger than 32 MiB");
    }
    if lower.contains("unavailable")
        || lower.contains("serviceunknown")
        || lower.contains("namehasnoowner")
    {
        return (
            503,
            "The Membrie Desktop Bridge is not responding on the PC",
        );
    }
    (502, "The PC clipboard image could not be read")
}

fn verified_clipboard_image_type(mime_type: &str, content: &[u8]) -> Option<&'static str> {
    match mime_type {
        "image/png" if content.starts_with(b"\x89PNG\r\n\x1a\n") => Some("image/png"),
        "image/jpeg" if content.starts_with(b"\xff\xd8\xff") => Some("image/jpeg"),
        "image/webp"
            if content.len() >= 12 && &content[..4] == b"RIFF" && &content[8..12] == b"WEBP" =>
        {
            Some("image/webp")
        }
        _ => None,
    }
}

fn write_clipboard(request: &mut Request) -> Result<PreparedResponse> {
    let input: ClipboardInput = match read_json(request) {
        Ok(input) => input,
        Err(response) => return Ok(response),
    };
    if input.text.trim().is_empty() || input.text.len() > MAX_CLIPBOARD_BYTES {
        return Ok(PreparedResponse::error(
            422,
            "Clipboard text is empty or too large",
        ));
    }
    if membrie_core::policy::sensitive_reason(&input.text).is_some() {
        return Ok(PreparedResponse::error(
            422,
            "That text looks sensitive and was not placed on the PC clipboard",
        ));
    }
    if !bridge_write_clipboard(&input.text)? {
        return Ok(PreparedResponse::error(
            503,
            "The PC clipboard was not available",
        ));
    }
    Ok(PreparedResponse::json(
        200,
        &ApiMessage {
            message: "Text placed on the PC clipboard",
        },
    ))
}

fn correct_attachment(request: &mut Request, state: &AppState) -> Result<PreparedResponse> {
    let input: AttachmentCorrectionInput = match read_json(request) {
        Ok(input) => input,
        Err(response) => return Ok(response),
    };
    if !companion_attachment_is_available(state, &input.remembrie_id, &input.content_id)? {
        return Ok(PreparedResponse::error(404, "Attachment not found"));
    }
    let attachment = state
        .client
        .correct_attachment_transcript(input.content_id, input.correction)?;
    Ok(PreparedResponse::json(
        200,
        &ApiMessage {
            message: if attachment.user_correction.is_some() {
                "Transcript correction saved; the machine transcript and original audio were preserved"
            } else {
                "Transcript correction removed; the machine transcript and original audio remain"
            },
        },
    ))
}

fn retry_attachment(request: &mut Request, state: &AppState) -> Result<PreparedResponse> {
    let input: AttachmentActionInput = match read_json(request) {
        Ok(input) => input,
        Err(response) => return Ok(response),
    };
    if !companion_attachment_is_available(state, &input.remembrie_id, &input.content_id)? {
        return Ok(PreparedResponse::error(404, "Attachment not found"));
    }
    state.client.retry_attachment(input.content_id)?;
    Ok(PreparedResponse::json(
        200,
        &ApiMessage {
            message: "Local understanding was queued again",
        },
    ))
}

fn delete_attachment(request: &mut Request, state: &AppState) -> Result<PreparedResponse> {
    let input: AttachmentActionInput = match read_json(request) {
        Ok(input) => input,
        Err(response) => return Ok(response),
    };
    if !companion_attachment_is_available(state, &input.remembrie_id, &input.content_id)? {
        return Ok(PreparedResponse::error(404, "Attachment not found"));
    }
    state.client.delete_attachment(input.content_id)?;
    Ok(PreparedResponse::json(
        200,
        &ApiMessage {
            message: "Attachment deleted locally",
        },
    ))
}

fn delete_remembrance(request: &mut Request, state: &AppState) -> Result<PreparedResponse> {
    let input: RemembranceActionInput = match read_json(request) {
        Ok(input) => input,
        Err(response) => return Ok(response),
    };
    let Some(remembrance) = state.client.get_remembrie(&input.id)? else {
        return Ok(PreparedResponse::error(404, "Remembrie not found"));
    };
    if remembrance.source_app.as_deref() != Some("Membrie Companion") {
        return Ok(PreparedResponse::error(
            403,
            "Only Remembries created in Companion can be deleted here",
        ));
    }
    state.client.delete_remembrance(input.id)?;
    Ok(PreparedResponse::json(
        200,
        &ApiMessage {
            message: "Remembrie and its private attachments were deleted locally",
        },
    ))
}

fn companion_attachment_is_available(
    state: &AppState,
    remembrie_id: &str,
    content_id: &str,
) -> Result<bool> {
    let Some(remembrance) = state.client.get_remembrie(remembrie_id)? else {
        return Ok(false);
    };
    if remembrance.source_app.as_deref() != Some("Membrie Companion") {
        return Ok(false);
    }
    Ok(state
        .client
        .list_attachments(remembrie_id)?
        .into_iter()
        .any(|attachment| attachment.content_id == content_id))
}

fn read_attachment(state: &AppState, path: &str) -> Result<PreparedResponse> {
    let mut parts = path
        .trim_start_matches("/api/attachment/")
        .split('/')
        .filter(|part| !part.is_empty());
    let Some(remembrie_id) = parts.next() else {
        return Ok(PreparedResponse::error(404, "Attachment not found"));
    };
    let Some(content_id) = parts.next() else {
        return Ok(PreparedResponse::error(404, "Attachment not found"));
    };
    if parts.next().is_some() {
        return Ok(PreparedResponse::error(404, "Attachment not found"));
    }
    let Some(attachment) = state
        .client
        .list_attachments(remembrie_id)?
        .into_iter()
        .find(|attachment| attachment.content_id == content_id)
    else {
        return Ok(PreparedResponse::error(404, "Attachment not found"));
    };
    let content_type = match attachment.mime_type.as_str() {
        "image/png" => "image/png",
        "image/jpeg" => "image/jpeg",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        "audio/aac" | "audio/x-aac" => "audio/aac",
        "audio/flac" | "audio/x-flac" => "audio/flac",
        "audio/m4a" | "audio/mp4" | "audio/x-m4a" => "audio/mp4",
        "audio/mpeg" => "audio/mpeg",
        "audio/ogg" => "audio/ogg",
        "audio/opus" => "audio/opus",
        "audio/wav" | "audio/x-wav" => "audio/wav",
        "audio/webm" => "audio/webm",
        "audio/x-caf" => "audio/x-caf",
        _ => {
            return Ok(PreparedResponse::error(
                415,
                "This attachment format does not have an inline preview",
            ));
        }
    };
    if attachment.blob_hash.len() != 64
        || !attachment
            .blob_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Ok(PreparedResponse::error(
            500,
            "The stored attachment is invalid",
        ));
    }
    let blob_path = attachment_blob_path(&attachment.blob_hash);
    let metadata = fs::symlink_metadata(&blob_path)?;
    if !metadata.file_type().is_file()
        || metadata.len() != attachment.byte_size
        || metadata.len() > MAX_ATTACHMENT_BYTES
    {
        return Ok(PreparedResponse::error(
            500,
            "The stored attachment is invalid",
        ));
    }
    Ok(PreparedResponse::text(
        200,
        content_type,
        fs::read(blob_path)?,
    ))
}

fn mobile_remembrance(record: Remembrie, state: &AppState) -> Result<MobileRemembrance> {
    let preview = record
        .summary
        .as_deref()
        .filter(|summary| !summary.trim().is_empty())
        .unwrap_or(record.body.as_str());
    let attachments = state
        .client
        .list_attachments(&record.id)?
        .into_iter()
        .map(|attachment| MobileAttachment {
            content_id: attachment.content_id,
            original_name: attachment.original_name,
            mime_type: attachment.mime_type,
            byte_size: attachment.byte_size,
            analysis_state: attachment.analysis_state,
            analysis_text: attachment
                .analysis_text
                .map(|text| truncate(&text, MAX_NOTE_CHARACTERS)),
            user_correction: attachment
                .user_correction
                .map(|text| truncate(&text, MAX_NOTE_CHARACTERS)),
            analysis_model: attachment.analysis_model,
            analysis_confidence: attachment.analysis_confidence,
        })
        .collect();
    Ok(MobileRemembrance {
        id: record.id,
        kind: record.kind,
        title: record.title,
        source: record
            .source_app
            .unwrap_or_else(|| "Unknown local source".to_owned()),
        occurred_at_ms: record.occurred_at_ms,
        preview: truncate(preview, 260),
        attachments,
    })
}

fn read_json<T: for<'de> Deserialize<'de>>(
    request: &mut Request,
) -> std::result::Result<T, PreparedResponse> {
    if request
        .body_length()
        .is_some_and(|length| length as u64 > MAX_REQUEST_BYTES)
    {
        return Err(PreparedResponse::error(413, "That request is too large"));
    }
    let content_type = request
        .headers()
        .iter()
        .find(|header| {
            header
                .field
                .as_str()
                .as_str()
                .eq_ignore_ascii_case("content-type")
        })
        .map(|header| header.value.as_str())
        .unwrap_or("");
    if !content_type
        .to_ascii_lowercase()
        .starts_with("application/json")
    {
        return Err(PreparedResponse::error(415, "This request must use JSON"));
    }
    let mut body = Vec::new();
    request
        .as_reader()
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|_| PreparedResponse::error(400, "The request body could not be read"))?;
    if body.len() as u64 > MAX_REQUEST_BYTES {
        return Err(PreparedResponse::error(413, "That request is too large"));
    }
    serde_json::from_slice(&body)
        .map_err(|_| PreparedResponse::error(400, "The request was not valid JSON"))
}

fn authorized(request: &Request, expected: &str) -> bool {
    request.headers().iter().any(|header| {
        header
            .field
            .as_str()
            .as_str()
            .eq_ignore_ascii_case("x-membrie-token")
            && constant_time_equal(header.value.as_str().as_bytes(), expected.as_bytes())
    })
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn mobile_read_allowed(locked: bool, allow_while_locked: bool) -> bool {
    !locked || allow_while_locked
}

fn remote_is_loopback(remote: Option<&SocketAddr>) -> bool {
    remote.is_some_and(|remote| remote.ip().is_loopback())
}

fn host_is_trusted(request: &Request, tailnet_host: Option<&str>) -> bool {
    header_value(request, "host").is_some_and(|host| host_value_is_trusted(host, tailnet_host))
}

fn origin_is_trusted(request: &Request, tailnet_host: Option<&str>) -> bool {
    let Some(origin) = header_value(request, "origin") else {
        return true;
    };
    origin_value_is_trusted(origin, tailnet_host)
}

fn host_value_is_trusted(host: &str, tailnet_host: Option<&str>) -> bool {
    let host = host.to_ascii_lowercase();
    matches!(host.as_str(), "127.0.0.1:47381" | "localhost:47381")
        || tailnet_host.is_some_and(|tailnet_host| {
            host == tailnet_host || host == format!("{tailnet_host}:443")
        })
}

fn origin_value_is_trusted(origin: &str, tailnet_host: Option<&str>) -> bool {
    let origin = origin.to_ascii_lowercase();
    matches!(
        origin.as_str(),
        "http://127.0.0.1:47381" | "http://localhost:47381"
    ) || tailnet_host.is_some_and(|tailnet_host| origin == format!("https://{tailnet_host}"))
}

fn header_value<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|header| header.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

fn desktop_proxy() -> Result<gio::DBusProxy> {
    let proxy = gio::DBusProxy::for_bus_sync(
        gio::BusType::Session,
        gio::DBusProxyFlags::DO_NOT_AUTO_START,
        None,
        DBUS_NAME,
        DBUS_PATH,
        DBUS_INTERFACE,
        None::<&gio::Cancellable>,
    )?;
    if proxy.name_owner().is_none() {
        bail!("the GNOME Desktop Bridge is unavailable");
    }
    Ok(proxy)
}

fn desktop_locked() -> Result<bool> {
    let response = desktop_proxy()?.call_sync(
        "GetActivityState",
        None,
        gio::DBusCallFlags::NONE,
        1_000,
        None::<&gio::Cancellable>,
    )?;
    let (_, _, _, _, locked) = response.try_get::<(String, String, String, u32, bool)>()?;
    Ok(locked)
}

fn bridge_read_clipboard() -> Result<String> {
    let response = desktop_proxy()?.call_sync(
        "ReadText",
        None,
        gio::DBusCallFlags::NONE,
        6_000,
        None::<&gio::Cancellable>,
    )?;
    Ok(response.try_get::<(String,)>()?.0)
}

fn bridge_read_clipboard_image() -> Result<(String, Vec<u8>)> {
    let response = desktop_proxy()?.call_sync(
        "ReadImage",
        None,
        gio::DBusCallFlags::NONE,
        8_000,
        None::<&gio::Cancellable>,
    )?;
    Ok(response.try_get::<(String, Vec<u8>)>()?)
}

fn bridge_write_clipboard(text: &str) -> Result<bool> {
    let parameters = (text,).to_variant();
    let response = desktop_proxy()?.call_sync(
        "SetText",
        Some(&parameters),
        gio::DBusCallFlags::NONE,
        2_000,
        None::<&gio::Cancellable>,
    )?;
    Ok(response.try_get::<(bool,)>()?.0)
}

fn load_tailnet_host() -> Result<Option<String>> {
    let path = mobile_tailnet_host_path();
    let value = match fs::read_to_string(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    Ok(Some(normalize_tailnet_host(&value)?))
}

fn save_tailnet_host(host: &str) -> Result<()> {
    let path = mobile_tailnet_host_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() && !fs::symlink_metadata(&path)?.file_type().is_file() {
        bail!("the trusted tailnet-host path is not a regular file");
    }
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    #[cfg(unix)]
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(host.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn normalize_tailnet_host(value: &str) -> Result<String> {
    let host = value.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.len() > 253 || !host.ends_with(".ts.net") {
        bail!("the trusted hostname must be this PC's exact Tailscale .ts.net name");
    }
    if host.split('.').any(|label| {
        label.is_empty()
            || label.len() > 63
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || !label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
    }) {
        bail!("the trusted Tailscale hostname is not valid");
    }
    Ok(host)
}

fn load_or_create_token() -> Result<String> {
    let path = mobile_token_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let token = fs::read_to_string(&path)?.trim().to_owned();
        validate_token(&token)?;
        return Ok(token);
    }

    let mut random = [0_u8; TOKEN_BYTES];
    File::open("/dev/urandom")?.read_exact(&mut random)?;
    let token = hex(&random);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(token)
}

fn validate_token(token: &str) -> Result<()> {
    if token.len() != TOKEN_BYTES * 2 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("the Mobile Companion pairing token is invalid");
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

fn truncate(value: &str, maximum: usize) -> String {
    let mut result: String = value.chars().take(maximum).collect();
    if value.chars().count() > maximum {
        result.push('…');
    }
    result
}

fn random_hex_identifier() -> Result<String> {
    let mut random = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut random)?;
    Ok(random.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn cleanup_stale_uploads() -> Result<()> {
    let inbox = attachment_inbox_dir();
    if !inbox.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(inbox)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.file_type().is_file()
            || metadata.modified()?.elapsed().unwrap_or_default().as_secs() < 24 * 60 * 60
        {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let identifier = name.strip_suffix(".name").unwrap_or(name);
        if identifier.len() == 32
            && identifier
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            decoded.push(hex_digit(high)? * 16 + hex_digit(low)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn current_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static HTTP header is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn companion_policy_allows_only_local_and_private_blob_audio() {
        let response = PreparedResponse::text(200, "text/plain", b"ok".to_vec()).into_http();
        let policy = response
            .headers()
            .iter()
            .find(|header| {
                header
                    .field
                    .as_str()
                    .as_str()
                    .eq_ignore_ascii_case("content-security-policy")
            })
            .map(|header| header.value.as_str());

        assert!(policy.is_some_and(|policy| policy.contains("media-src 'self' blob:")));
    }

    #[test]
    fn pairing_tokens_are_compared_exactly() {
        assert!(constant_time_equal(b"abcdef", b"abcdef"));
        assert!(!constant_time_equal(b"abcdef", b"abcdeg"));
        assert!(!constant_time_equal(b"short", b"longer"));
    }

    #[test]
    fn token_shape_is_strict() {
        assert!(validate_token(&"a5".repeat(TOKEN_BYTES)).is_ok());
        assert!(validate_token("too-short").is_err());
        assert!(validate_token(&"zz".repeat(TOKEN_BYTES)).is_err());
    }

    #[test]
    fn recall_preview_is_bounded() {
        assert_eq!(truncate("remember me", 20), "remember me");
        assert_eq!(truncate("abcdef", 3), "abc…");
    }

    #[test]
    fn uploaded_image_names_are_decoded_without_losing_unicode() {
        assert_eq!(
            percent_decode("Duke%20notes%20%E2%80%94%20photo.png").as_deref(),
            Some("Duke notes — photo.png")
        );
        assert!(percent_decode("bad%2name.png").is_none());
        assert!(percent_decode("bad%FFname.png").is_none());
    }

    #[test]
    fn pc_clipboard_images_require_matching_content_signatures() {
        assert_eq!(
            verified_clipboard_image_type("image/png", b"\x89PNG\r\n\x1a\nprivate"),
            Some("image/png")
        );
        assert_eq!(
            verified_clipboard_image_type("image/jpeg", b"\xff\xd8\xffprivate"),
            Some("image/jpeg")
        );
        assert_eq!(
            verified_clipboard_image_type("image/webp", b"RIFF0000WEBPprivate"),
            Some("image/webp")
        );
        assert!(verified_clipboard_image_type("image/png", b"not-a-png").is_none());
        assert!(
            verified_clipboard_image_type("application/octet-stream", b"\x89PNG\r\n\x1a\n")
                .is_none()
        );
    }

    #[test]
    fn pc_clipboard_image_dbus_payload_round_trips() {
        let variant = ("image/png", vec![1_u8, 2, 3]).to_variant();
        let decoded = variant.try_get::<(String, Vec<u8>)>().unwrap();
        assert_eq!(decoded, ("image/png".to_owned(), vec![1, 2, 3]));
    }

    #[test]
    fn pc_clipboard_image_errors_remain_actionable() {
        assert_eq!(
            clipboard_image_bridge_error("GDBus.Error:com.chuk.Membrie.Error.NoImage"),
            (
                404,
                "The PC clipboard currently contains no supported image; copy the image itself, not only its file"
            )
        );
        assert_eq!(
            clipboard_image_bridge_error("GDBus.Error:com.chuk.Membrie.Error.Timeout"),
            (504, "The PC took too long to provide that clipboard image")
        );
        assert_eq!(
            clipboard_image_bridge_error("the GNOME Desktop Bridge is unavailable"),
            (
                503,
                "The Membrie Desktop Bridge is not responding on the PC"
            )
        );
    }

    #[test]
    fn locked_desktops_deny_reads_without_separate_consent() {
        assert!(mobile_read_allowed(false, false));
        assert!(mobile_read_allowed(false, true));
        assert!(!mobile_read_allowed(true, false));
        assert!(mobile_read_allowed(true, true));
    }

    #[test]
    fn tailnet_host_trust_is_exact_and_https_only() {
        let host = normalize_tailnet_host("RainbowBright.tail123.ts.net.\n").unwrap();
        assert_eq!(host, "rainbowbright.tail123.ts.net");
        assert!(host_value_is_trusted(&host, Some(&host)));
        assert!(host_value_is_trusted(
            "rainbowbright.tail123.ts.net:443",
            Some(&host)
        ));
        assert!(!host_value_is_trusted(
            "another.tail123.ts.net",
            Some(&host)
        ));
        assert!(origin_value_is_trusted(
            "https://rainbowbright.tail123.ts.net",
            Some(&host)
        ));
        assert!(!origin_value_is_trusted(
            "http://rainbowbright.tail123.ts.net",
            Some(&host)
        ));
    }

    #[test]
    fn tailnet_host_configuration_rejects_non_tailscale_names() {
        assert!(normalize_tailnet_host("example.com").is_err());
        assert!(normalize_tailnet_host("https://node.example.ts.net").is_err());
        assert!(normalize_tailnet_host("-node.tail123.ts.net").is_err());
    }

    #[test]
    fn pairing_invitations_have_a_strict_shape() {
        assert_eq!(
            parse_pairing_invitation("01234567\n1790000000000\n"),
            Some(("01234567", 1_790_000_000_000))
        );
        assert!(parse_pairing_invitation("1234\n1790000000000\n").is_none());
        assert!(parse_pairing_invitation("01234567\nnot-time\n").is_none());
        assert!(parse_pairing_invitation("01234567\n1790000000000\nextra").is_none());
    }
}
