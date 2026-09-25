use crate::model::{
    BackupInfo, CaptureCandidate, CaptureDecision, CaptureRule, CaptureStatus, NewRemembrie,
    PauseMode, Remembrie, SearchHit,
};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

pub const MAX_MESSAGE_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Status,
    Create {
        remembrie: NewRemembrie,
    },
    Capture {
        candidate: CaptureCandidate,
    },
    ListRecent {
        limit: u32,
    },
    Search {
        query: String,
        limit: u32,
    },
    SetPause {
        mode: PauseMode,
    },
    SetClipboardEnabled {
        enabled: bool,
    },
    ListCaptureRules,
    AddCaptureRule {
        rule_type: String,
        pattern: String,
        label: Option<String>,
    },
    DeleteCaptureRule {
        id: String,
    },
    DeleteSince {
        timestamp_ms: i64,
    },
    ClipboardAgentHeartbeat,
    CreateBackup,
    ListBackups,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Status { status: CaptureStatus },
    Created { remembrie: Remembrie },
    CaptureResult { decision: CaptureDecision },
    Remembries { remembries: Vec<Remembrie> },
    SearchResults { hits: Vec<SearchHit> },
    PauseUpdated { status: CaptureStatus },
    CaptureSourceUpdated { status: CaptureStatus },
    CaptureRules { rules: Vec<CaptureRule> },
    CaptureRuleAdded { rule: CaptureRule },
    CaptureRuleDeleted { id: String },
    Deleted { count: u64 },
    CaptureAgentHeartbeatRecorded { status: CaptureStatus },
    BackupCreated { backup: BackupInfo },
    Backups { backups: Vec<BackupInfo> },
    Error { message: String },
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("could not connect to the Membrie daemon at {path}: {source}")]
    Connect {
        path: String,
        source: std::io::Error,
    },
    #[error("daemon communication failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid daemon response: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the daemon returned an empty response")]
    EmptyResponse,
    #[error("the daemon rejected the request: {0}")]
    Rejected(String),
}

#[derive(Debug, Clone)]
pub struct DaemonClient {
    socket_path: PathBuf,
}

impl DaemonClient {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: path.into(),
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn request(&self, request: &Request) -> Result<Response, ClientError> {
        let mut stream =
            UnixStream::connect(&self.socket_path).map_err(|source| ClientError::Connect {
                path: self.socket_path.display().to_string(),
                source,
            })?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;

        serde_json::to_writer(&mut stream, request)?;
        stream.write_all(b"\n")?;
        stream.shutdown(std::net::Shutdown::Write)?;

        let mut line = String::new();
        BufReader::new(stream)
            .take(MAX_MESSAGE_BYTES)
            .read_line(&mut line)?;
        if line.is_empty() {
            return Err(ClientError::EmptyResponse);
        }

        let response: Response = serde_json::from_str(&line)?;
        if let Response::Error { message } = response {
            return Err(ClientError::Rejected(message));
        }
        Ok(response)
    }

    pub fn status(&self) -> Result<CaptureStatus, ClientError> {
        match self.request(&Request::Status)? {
            Response::Status { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn create(&self, remembrie: NewRemembrie) -> Result<Remembrie, ClientError> {
        match self.request(&Request::Create { remembrie })? {
            Response::Created { remembrie } => Ok(remembrie),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn capture(&self, candidate: CaptureCandidate) -> Result<CaptureDecision, ClientError> {
        match self.request(&Request::Capture { candidate })? {
            Response::CaptureResult { decision } => Ok(decision),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn list_recent(&self, limit: u32) -> Result<Vec<Remembrie>, ClientError> {
        match self.request(&Request::ListRecent { limit })? {
            Response::Remembries { remembries } => Ok(remembries),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn search(
        &self,
        query: impl Into<String>,
        limit: u32,
    ) -> Result<Vec<SearchHit>, ClientError> {
        match self.request(&Request::Search {
            query: query.into(),
            limit,
        })? {
            Response::SearchResults { hits } => Ok(hits),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn set_pause(&self, mode: PauseMode) -> Result<CaptureStatus, ClientError> {
        match self.request(&Request::SetPause { mode })? {
            Response::PauseUpdated { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn set_clipboard_enabled(&self, enabled: bool) -> Result<CaptureStatus, ClientError> {
        match self.request(&Request::SetClipboardEnabled { enabled })? {
            Response::CaptureSourceUpdated { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn list_capture_rules(&self) -> Result<Vec<CaptureRule>, ClientError> {
        match self.request(&Request::ListCaptureRules)? {
            Response::CaptureRules { rules } => Ok(rules),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn add_capture_rule(
        &self,
        rule_type: impl Into<String>,
        pattern: impl Into<String>,
        label: Option<String>,
    ) -> Result<CaptureRule, ClientError> {
        match self.request(&Request::AddCaptureRule {
            rule_type: rule_type.into(),
            pattern: pattern.into(),
            label,
        })? {
            Response::CaptureRuleAdded { rule } => Ok(rule),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn delete_capture_rule(&self, id: impl Into<String>) -> Result<(), ClientError> {
        match self.request(&Request::DeleteCaptureRule { id: id.into() })? {
            Response::CaptureRuleDeleted { .. } => Ok(()),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn delete_since(&self, timestamp_ms: i64) -> Result<u64, ClientError> {
        match self.request(&Request::DeleteSince { timestamp_ms })? {
            Response::Deleted { count } => Ok(count),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn record_clipboard_agent_heartbeat(&self) -> Result<CaptureStatus, ClientError> {
        match self.request(&Request::ClipboardAgentHeartbeat)? {
            Response::CaptureAgentHeartbeatRecorded { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn create_backup(&self) -> Result<BackupInfo, ClientError> {
        match self.request(&Request::CreateBackup)? {
            Response::BackupCreated { backup } => Ok(backup),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn list_backups(&self) -> Result<Vec<BackupInfo>, ClientError> {
        match self.request(&Request::ListBackups)? {
            Response::Backups { backups } => Ok(backups),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }
}
