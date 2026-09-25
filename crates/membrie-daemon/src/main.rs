mod intelligence;
mod ollama;

use anyhow::{Context, Result, anyhow};
use membrie_core::{
    BackupInfo, Repository, Request, Response, ScreenCaptureCandidate, backup_dir, database_path,
    screen_spool_dir, socket_path,
};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_REQUEST_BYTES: u64 = 1_048_576;
const MAX_SCREEN_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
const BACKUP_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;
const BACKUP_CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
const MAX_BACKUPS: usize = 14;
static CLIPBOARD_AGENT_LAST_SEEN_MS: AtomicI64 = AtomicI64::new(0);

fn main() -> Result<()> {
    let database_path = database_path();
    let socket_path = socket_path();
    let mut repository = Repository::open(&database_path)
        .with_context(|| format!("could not open {}", database_path.display()))?;
    repository.reset_interrupted_processing()?;
    repository.reset_interrupted_activity()?;
    if let Err(error) = maybe_create_automatic_backup(&repository) {
        eprintln!("automatic backup failed: {error:#}");
    }

    prepare_socket(&socket_path)?;
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("could not bind {}", socket_path.display()))?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;

    println!("Membrie daemon ready");
    println!("  database: {}", database_path.display());
    println!("  socket:   {}", socket_path.display());

    let repository = Arc::new(Mutex::new(repository));
    let ollama = ollama::OllamaClient::default();
    start_backup_scheduler(&repository);
    intelligence::start_worker(&repository, &ollama);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let repository = Arc::clone(&repository);
                let ollama = ollama.clone();
                std::thread::spawn(move || {
                    if let Err(error) = handle_connection(stream, repository, ollama) {
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

fn handle_connection(
    stream: UnixStream,
    repository: Arc<Mutex<Repository>>,
    ollama: ollama::OllamaClient,
) -> Result<()> {
    let mut line = String::new();
    BufReader::new(stream.try_clone()?)
        .take(MAX_REQUEST_BYTES)
        .read_line(&mut line)?;
    if line.is_empty() {
        return Ok(());
    }

    let response = match serde_json::from_str::<Request>(&line) {
        Ok(request) => {
            dispatch(request, &repository, &ollama).unwrap_or_else(|error| Response::Error {
                message: error.to_string(),
            })
        }
        Err(error) => Response::Error {
            message: format!("invalid request: {error}"),
        },
    };

    let mut stream = stream;
    serde_json::to_writer(&mut stream, &response)?;
    stream.write_all(b"\n")?;
    Ok(())
}

fn dispatch(
    request: Request,
    repository: &Mutex<Repository>,
    ollama: &ollama::OllamaClient,
) -> Result<Response> {
    match request {
        Request::Search { query, limit } => Ok(Response::SearchResults {
            hits: intelligence::search(repository, ollama, &query, limit)?,
        }),
        Request::IntelligenceStatus => Ok(Response::IntelligenceStatus {
            status: intelligence::status(repository, ollama)?,
        }),
        Request::SetIntelligenceSettings { settings } => {
            Ok(Response::IntelligenceSettingsUpdated {
                status: intelligence::update_settings(repository, ollama, &settings)?,
            })
        }
        Request::RetryIntelligence => Ok(Response::IntelligenceRetryQueued {
            count: intelligence::retry_failed(repository)?,
        }),
        Request::AskBrie { question } => Ok(Response::BrieAnswered {
            answer: intelligence::ask_brie(repository, ollama, &question)?,
        }),
        Request::AnalyzeScreen { candidate } => Ok(Response::ScreenAnalyzed {
            result: analyze_screen_candidate(repository, ollama, &candidate)?,
        }),
        request => dispatch_database(request, repository),
    }
}

fn dispatch_database(request: Request, repository: &Mutex<Repository>) -> Result<Response> {
    let mut repository = repository
        .lock()
        .map_err(|_| anyhow!("database lock was poisoned"))?;
    match request {
        Request::Status => Ok(Response::Status {
            status: with_capture_health(repository.status()?),
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
        Request::SetPause { mode } => Ok(Response::PauseUpdated {
            status: with_capture_health(repository.set_pause(mode)?),
        }),
        Request::SetClipboardEnabled { enabled } => Ok(Response::CaptureSourceUpdated {
            status: with_capture_health(repository.set_clipboard_enabled(enabled)?),
        }),
        Request::SetActivityEnabled { enabled } => Ok(Response::CaptureSourceUpdated {
            status: with_capture_health(repository.set_activity_enabled(enabled)?),
        }),
        Request::SetActivityIdleThreshold { idle_threshold_ms } => {
            Ok(Response::CaptureSourceUpdated {
                status: with_capture_health(
                    repository.set_activity_idle_threshold(idle_threshold_ms)?,
                ),
            })
        }
        Request::SetScreenEnabled { enabled } => Ok(Response::CaptureSourceUpdated {
            status: with_capture_health(repository.set_screen_enabled(enabled)?),
        }),
        Request::SetScreenSampleInterval { sample_interval_ms } => {
            Ok(Response::CaptureSourceUpdated {
                status: with_capture_health(
                    repository.set_screen_sample_interval(sample_interval_ms)?,
                ),
            })
        }
        Request::SetScreenModel { model } => Ok(Response::CaptureSourceUpdated {
            status: with_capture_health(repository.set_screen_model(&model)?),
        }),
        Request::RecordActivity { snapshot } => Ok(Response::ActivityRecorded {
            result: repository.record_activity_snapshot(snapshot)?,
        }),
        Request::EndActivitySession { reason } => {
            repository.end_activity_session(&reason)?;
            Ok(Response::ActivitySessionEnded)
        }
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
        Request::ClipboardAgentHeartbeat => {
            CLIPBOARD_AGENT_LAST_SEEN_MS.store(now_ms(), Ordering::Relaxed);
            Ok(Response::CaptureAgentHeartbeatRecorded {
                status: with_capture_health(repository.status()?),
            })
        }
        Request::CreateBackup => Ok(Response::BackupCreated {
            backup: create_backup(&repository)?,
        }),
        Request::ListBackups => Ok(Response::Backups {
            backups: list_backups()?,
        }),
        Request::Search { .. }
        | Request::IntelligenceStatus
        | Request::SetIntelligenceSettings { .. }
        | Request::RetryIntelligence
        | Request::AskBrie { .. }
        | Request::AnalyzeScreen { .. } => unreachable!("handled before database dispatch"),
    }
}

fn analyze_screen_candidate(
    repository: &Mutex<Repository>,
    ollama: &ollama::OllamaClient,
    candidate: &ScreenCaptureCandidate,
) -> Result<membrie_core::ScreenCaptureResult> {
    let image = take_temporary_screen_image(&candidate.screenshot_path)?;
    let model = repository
        .lock()
        .map_err(|_| anyhow!("database lock was poisoned"))?
        .screen_model_for_candidate(candidate)?;
    let analysis = match intelligence::analyze_screen(ollama, &model, &image) {
        Ok(analysis) => analysis,
        Err(error) => {
            if let Ok(repository) = repository.lock()
                && let Err(record_error) =
                    repository.record_screen_failure(candidate, &model, &error.to_string())
            {
                eprintln!("could not record local screen-analysis failure: {record_error}");
            }
            return Err(error);
        }
    };
    repository
        .lock()
        .map_err(|_| anyhow!("database lock was poisoned"))?
        .record_screen_analysis(candidate, &analysis)
        .map_err(Into::into)
}

fn take_temporary_screen_image(path: &str) -> Result<Vec<u8>> {
    let requested = std::path::PathBuf::from(path);
    let spool = screen_spool_dir();
    let canonical_spool = spool
        .canonicalize()
        .with_context(|| format!("screen spool is unavailable at {}", spool.display()))?;
    let canonical = requested
        .canonicalize()
        .with_context(|| "the temporary screen image is unavailable")?;
    if canonical.parent() != Some(canonical_spool.as_path())
        || canonical.extension().and_then(|value| value.to_str()) != Some("png")
    {
        return Err(anyhow!(
            "the temporary screen image is outside Membrie's private spool"
        ));
    }
    let metadata = fs::symlink_metadata(&canonical)?;
    if !metadata.file_type().is_file()
        || metadata.len() < 8
        || metadata.len() > MAX_SCREEN_IMAGE_BYTES
    {
        let _ = fs::remove_file(&canonical);
        return Err(anyhow!(
            "the temporary screen image has an invalid size or type"
        ));
    }
    let read_result = fs::read(&canonical);
    let remove_result = fs::remove_file(&canonical);
    let image = read_result.context("could not read the temporary screen image")?;
    remove_result.context("could not securely remove the temporary screen image")?;
    if !image.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(anyhow!("the temporary screen image was not a PNG"));
    }
    Ok(image)
}

fn with_capture_health(mut status: membrie_core::CaptureStatus) -> membrie_core::CaptureStatus {
    let last_seen = CLIPBOARD_AGENT_LAST_SEEN_MS.load(Ordering::Relaxed);
    status.clipboard_agent_last_seen_ms = (last_seen > 0).then_some(last_seen);
    status
}

fn start_backup_scheduler(repository: &Arc<Mutex<Repository>>) {
    let repository = Arc::clone(repository);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(BACKUP_CHECK_INTERVAL);
            let result = repository
                .lock()
                .map_err(|_| anyhow!("database lock was poisoned"))
                .and_then(|repository| maybe_create_automatic_backup(&repository));
            if let Err(error) = result {
                eprintln!("automatic backup failed: {error:#}");
            }
        }
    });
}

fn maybe_create_automatic_backup(repository: &Repository) -> Result<()> {
    let newest = list_backups()?.into_iter().next();
    let backup_is_current = newest
        .as_ref()
        .is_some_and(|backup| now_ms().saturating_sub(backup.created_at_ms) < BACKUP_INTERVAL_MS);
    if !backup_is_current {
        let backup = create_backup(repository)?;
        println!("Created automatic backup {}", backup.path);
    }
    Ok(())
}

fn create_backup(repository: &Repository) -> Result<BackupInfo> {
    let directory = backup_dir();
    fs::create_dir_all(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;

    let mut created_at_ms = now_ms();
    let destination = loop {
        let candidate = directory.join(format!("membrie-{created_at_ms}.db"));
        if !candidate.exists() {
            break candidate;
        }
        created_at_ms += 1;
    };
    let partial = destination.with_extension("db.partial");
    let backup_result = repository.backup_to(&partial);
    if let Err(error) = backup_result {
        let _ = fs::remove_file(&partial);
        return Err(error.into());
    }

    File::open(&partial)?.sync_all()?;
    fs::rename(&partial, &destination)?;
    File::open(&directory)?.sync_all()?;

    let backup = BackupInfo {
        path: destination.display().to_string(),
        created_at_ms,
        size_bytes: fs::metadata(&destination)?.len(),
    };
    if let Err(error) = prune_backups() {
        eprintln!("could not prune old backups: {error:#}");
    }
    Ok(backup)
}

fn list_backups() -> Result<Vec<BackupInfo>> {
    let directory = backup_dir();
    if !directory.exists() {
        return Ok(Vec::new());
    }

    let mut backups = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(created_at_ms) = name
            .to_str()
            .and_then(|name| name.strip_prefix("membrie-"))
            .and_then(|name| name.strip_suffix(".db"))
            .and_then(|timestamp| timestamp.parse::<i64>().ok())
        else {
            continue;
        };
        backups.push(BackupInfo {
            path: entry.path().display().to_string(),
            created_at_ms,
            size_bytes: entry.metadata()?.len(),
        });
    }
    backups.sort_by_key(|backup| std::cmp::Reverse(backup.created_at_ms));
    Ok(backups)
}

fn prune_backups() -> Result<()> {
    for backup in list_backups()?.into_iter().skip(MAX_BACKUPS) {
        fs::remove_file(&backup.path)
            .with_context(|| format!("could not prune old backup {}", backup.path))?;
    }
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
