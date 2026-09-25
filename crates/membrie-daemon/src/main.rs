use anyhow::{Context, Result, anyhow};
use membrie_core::{Repository, Request, Response, database_path, socket_path};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};

const MAX_REQUEST_BYTES: u64 = 1_048_576;

fn main() -> Result<()> {
    let database_path = database_path();
    let socket_path = socket_path();
    let repository = Repository::open(&database_path)
        .with_context(|| format!("could not open {}", database_path.display()))?;

    prepare_socket(&socket_path)?;
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("could not bind {}", socket_path.display()))?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;

    println!("Membrie daemon ready");
    println!("  database: {}", database_path.display());
    println!("  socket:   {}", socket_path.display());

    let repository = Arc::new(Mutex::new(repository));
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let repository = Arc::clone(&repository);
                std::thread::spawn(move || {
                    if let Err(error) = handle_connection(stream, repository) {
                        eprintln!("request failed: {error:#}");
                    }
                });
            }
            Err(error) => eprintln!("socket accept failed: {error}"),
        }
    }
    Ok(())
}

fn prepare_socket(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(anyhow!("another Membrie daemon is already running"));
        }
        fs::remove_file(path)
            .with_context(|| format!("could not remove stale socket {}", path.display()))?;
    }
    Ok(())
}

fn handle_connection(stream: UnixStream, repository: Arc<Mutex<Repository>>) -> Result<()> {
    let mut line = String::new();
    BufReader::new(stream.try_clone()?)
        .take(MAX_REQUEST_BYTES)
        .read_line(&mut line)?;
    if line.is_empty() {
        return Ok(());
    }

    let response = match serde_json::from_str::<Request>(&line) {
        Ok(request) => dispatch(request, &repository).unwrap_or_else(|error| Response::Error {
            message: error.to_string(),
        }),
        Err(error) => Response::Error {
            message: format!("invalid request: {error}"),
        },
    };

    let mut stream = stream;
    serde_json::to_writer(&mut stream, &response)?;
    stream.write_all(b"\n")?;
    Ok(())
}

fn dispatch(request: Request, repository: &Mutex<Repository>) -> Result<Response> {
    let mut repository = repository
        .lock()
        .map_err(|_| anyhow!("database lock was poisoned"))?;
    match request {
        Request::Status => Ok(Response::Status {
            status: repository.status()?,
        }),
        Request::Create { remembrie } => Ok(Response::Created {
            remembrie: repository.create(remembrie)?,
        }),
        Request::Capture { candidate } => Ok(Response::CaptureResult {
            decision: repository.capture(candidate)?,
        }),
        Request::ListRecent { limit } => Ok(Response::Remembries {
            remembries: repository.list_recent(limit)?,
        }),
        Request::Search { query, limit } => Ok(Response::SearchResults {
            hits: repository.search(&query, limit)?,
        }),
        Request::SetPause { mode } => Ok(Response::PauseUpdated {
            status: repository.set_pause(mode)?,
        }),
        Request::SetClipboardEnabled { enabled } => Ok(Response::CaptureSourceUpdated {
            status: repository.set_clipboard_enabled(enabled)?,
        }),
        Request::ListCaptureRules => Ok(Response::CaptureRules {
            rules: repository.list_capture_rules()?,
        }),
        Request::AddCaptureRule {
            rule_type,
            pattern,
            label,
        } => Ok(Response::CaptureRuleAdded {
            rule: repository.add_capture_rule(&rule_type, &pattern, label.as_deref())?,
        }),
        Request::DeleteCaptureRule { id } => {
            if repository.delete_capture_rule(&id)? {
                Ok(Response::CaptureRuleDeleted { id })
            } else {
                Ok(Response::Error {
                    message: "capture rule was not found".to_owned(),
                })
            }
        }
        Request::DeleteSince { timestamp_ms } => Ok(Response::Deleted {
            count: repository.delete_since(timestamp_ms)?,
        }),
    }
}
