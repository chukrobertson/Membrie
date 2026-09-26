// SPDX-License-Identifier: AGPL-3.0-or-later

use anyhow::{Context, Result, anyhow, bail};
use membrie_core::{CalendarSnapshot, CalendarSyncResult, Repository};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SYNC_INTERVAL: Duration = Duration::from_secs(15 * 60);
const HISTORY_DAYS: i64 = 365;
const FUTURE_DAYS: i64 = 365;
const MAX_HELPER_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
static SYNC_RUNNING: AtomicBool = AtomicBool::new(false);

pub fn start_scheduler(repository: &Arc<Mutex<Repository>>) {
    let repository = Arc::clone(repository);
    std::thread::spawn(move || {
        loop {
            let enabled = repository
                .lock()
                .ok()
                .and_then(|repository| repository.calendar_enabled().ok())
                .unwrap_or(false);
            if enabled && let Err(error) = sync(&repository) {
                eprintln!("calendar sync failed: {error:#}");
            }
            std::thread::sleep(SYNC_INTERVAL);
        }
    });
}

pub fn sync(repository: &Mutex<Repository>) -> Result<CalendarSyncResult> {
    if SYNC_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        bail!("a local calendar sync is already in progress");
    }
    let _guard = SyncGuard;

    {
        let repository = repository
            .lock()
            .map_err(|_| anyhow!("database lock was poisoned"))?;
        if !repository.calendar_enabled()? {
            bail!("calendar access is disabled");
        }
        if repository.status()?.paused {
            bail!("automatic capture is paused");
        }
    }

    let now = current_time_ms()?;
    let day_ms = 24 * 60 * 60 * 1000_i64;
    let start_ms = now.saturating_sub(HISTORY_DAYS * day_ms);
    let end_ms = now.saturating_add(FUTURE_DAYS * day_ms);
    let helper = calendar_helper_path()?;
    let output = Command::new(&helper)
        .arg("--start-ms")
        .arg(start_ms.to_string())
        .arg("--end-ms")
        .arg(end_ms.to_string())
        .output()
        .with_context(|| format!("could not start {}", helper.display()))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let message = if message.is_empty() {
            format!("calendar connector exited with {}", output.status)
        } else {
            message
        };
        record_failure(repository, &message);
        bail!(message);
    }
    if output.stdout.len() > MAX_HELPER_OUTPUT_BYTES {
        let message = "calendar connector returned more than 64 MiB";
        record_failure(repository, message);
        bail!(message);
    }
    let snapshot: CalendarSnapshot = match serde_json::from_slice(&output.stdout) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let message = format!("calendar connector returned invalid local data: {error}");
            record_failure(repository, &message);
            bail!(message);
        }
    };

    let mut repository = repository
        .lock()
        .map_err(|_| anyhow!("database lock was poisoned"))?;
    if !repository.calendar_enabled()? {
        bail!("calendar access was disabled before sync completed");
    }
    repository
        .sync_calendar_snapshot(&snapshot)
        .context("could not store the local calendar snapshot")
}

fn record_failure(repository: &Mutex<Repository>, error: &str) {
    if let Ok(repository) = repository.lock()
        && let Err(record_error) = repository.record_calendar_error(error)
    {
        eprintln!("could not record calendar failure: {record_error}");
    }
}

fn calendar_helper_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("MEMBRIE_CALENDAR_HELPER") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        bail!(
            "configured calendar connector is unavailable at {}",
            path.display()
        );
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(parent) = executable.parent()
    {
        let sibling = parent.join("membrie-calendar");
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    let development =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/membrie-calendar");
    if development.is_file() {
        return Ok(development);
    }
    bail!("the local calendar connector is not installed")
}

fn current_time_ms() -> Result<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_millis() as i64)
}

struct SyncGuard;

impl Drop for SyncGuard {
    fn drop(&mut self) {
        SYNC_RUNNING.store(false, Ordering::Release);
    }
}
