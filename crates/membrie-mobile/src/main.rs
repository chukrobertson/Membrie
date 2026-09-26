// SPDX-License-Identifier: AGPL-3.0-or-later

use anyhow::{Context, Result, anyhow, bail};
use gio::prelude::*;
use glib::variant::ToVariant;
use membrie_core::{DaemonClient, NewRemembrie, Remembrie, mobile_token_path, socket_path};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const LISTEN_ADDRESS: &str = "127.0.0.1:47381";
const WORKERS: usize = 4;
const MAX_REQUEST_BYTES: u64 = 300 * 1024;
const MAX_NOTE_CHARACTERS: usize = 20_000;
const MAX_CLIPBOARD_BYTES: usize = 256 * 1024;
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
}

#[derive(Deserialize)]
struct NoteInput {
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
}

#[derive(Deserialize)]
struct BrieInput {
    question: String,
}

#[derive(Deserialize)]
struct ClipboardInput {
    text: String,
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
}

#[derive(Serialize)]
struct MobileRemembrance {
    id: String,
    kind: String,
    title: String,
    source: String,
    occurred_at_ms: i64,
    preview: String,
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
                "camera=(), microphone=(), geolocation=(), payment=()",
            ),
            (
                "Content-Security-Policy",
                "default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
            ),
        ] {
            response.add_header(header(name, value));
        }
        response
    }
}

fn main() -> Result<()> {
    let token = Arc::new(load_or_create_token()?);
    let state = AppState {
        client: DaemonClient::new(socket_path()),
        token,
    };
    let server = Arc::new(
        Server::http(LISTEN_ADDRESS)
            .map_err(|error| anyhow!("could not bind the local mobile companion: {error}"))?,
    );
    println!("Membrie Mobile Companion ready at http://{LISTEN_ADDRESS}");
    println!("  local-only: yes");
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
    if !host_is_local(request) || !origin_matches_host(request) {
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
        (_, path) if path.starts_with("/api/") => route_api(request, state, path),
        _ => Ok(PreparedResponse::error(404, "Not found")),
    }
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
            },
        )),
        (&Method::Post, "/api/note") => create_note(request, state),
        _ if !read_allowed => Ok(PreparedResponse::error(
            423,
            "The PC is locked; enable paired-device recall in Membrie to continue",
        )),
        (&Method::Get, "/api/recall") => recall(state),
        (&Method::Post, "/api/brie") => ask_brie(request, state),
        (&Method::Get, "/api/clipboard") => read_clipboard(),
        (&Method::Post, "/api/clipboard") => write_clipboard(request),
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
        .create(NewRemembrie::manual(title, body))
        .context("could not store the mobile note")?;
    Ok(PreparedResponse::json(
        201,
        &ApiMessage {
            message: "Remembrie saved locally",
        },
    ))
}

fn recall(state: &AppState) -> Result<PreparedResponse> {
    let snapshot = state.client.recall_snapshot(8, 6)?;
    Ok(PreparedResponse::json(
        200,
        &RecallResponse {
            recent: snapshot
                .recent
                .into_iter()
                .map(mobile_remembrance)
                .collect(),
            upcoming: snapshot
                .upcoming
                .into_iter()
                .map(mobile_remembrance)
                .collect(),
        },
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

fn mobile_remembrance(record: Remembrie) -> MobileRemembrance {
    let preview = record
        .summary
        .as_deref()
        .filter(|summary| !summary.trim().is_empty())
        .unwrap_or(record.body.as_str());
    MobileRemembrance {
        id: record.id,
        kind: record.kind,
        title: record.title,
        source: record
            .source_app
            .unwrap_or_else(|| "Unknown local source".to_owned()),
        occurred_at_ms: record.occurred_at_ms,
        preview: truncate(preview, 260),
    }
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

fn host_is_local(request: &Request) -> bool {
    header_value(request, "host").is_some_and(|host| {
        matches!(
            host.to_ascii_lowercase().as_str(),
            "127.0.0.1:47381" | "localhost:47381"
        )
    })
}

fn origin_matches_host(request: &Request) -> bool {
    let Some(origin) = header_value(request, "origin") else {
        return true;
    };
    matches!(
        origin.to_ascii_lowercase().as_str(),
        "http://127.0.0.1:47381" | "http://localhost:47381"
    )
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

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static HTTP header is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn locked_desktops_deny_reads_without_separate_consent() {
        assert!(mobile_read_allowed(false, false));
        assert!(mobile_read_allowed(false, true));
        assert!(!mobile_read_allowed(true, false));
        assert!(mobile_read_allowed(true, true));
    }
}
