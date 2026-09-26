use crate::model::{
    ActivityRecordResult, ActivitySnapshot, BackupInfo, BrieAnswer, CalendarSyncResult,
    CaptureCandidate, CaptureDecision, CaptureRule, CaptureStatus, IntelligenceSettings,
    IntelligenceStatus, NewRemembrie, PauseMode, RecallSnapshot, Remembrie, ScreenCaptureCandidate,
    ScreenCaptureResult, SearchHit, SemanticCaptureCandidate, SemanticCaptureResult, TimelineEntry,
    TimelineHistorySpan, TimelineMapSlice,
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
    RecallSnapshot {
        recent_limit: u32,
        upcoming_limit: u32,
    },
    GetRemembrie {
        id: String,
    },
    TimelineHistory {
        since_ms: i64,
        until_ms: i64,
    },
    TimelineDay {
        start_ms: i64,
        end_ms: i64,
    },
    TimelineMap {
        start_ms: i64,
        end_ms: i64,
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
    SetActivityEnabled {
        enabled: bool,
    },
    SetActivityIdleThreshold {
        idle_threshold_ms: u64,
    },
    SetSemanticEnabled {
        enabled: bool,
    },
    SetSemanticSampleInterval {
        sample_interval_ms: u64,
    },
    SetScreenEnabled {
        enabled: bool,
    },
    SetScreenSampleInterval {
        sample_interval_ms: u64,
    },
    SetScreenModel {
        model: String,
    },
    SetCalendarEnabled {
        enabled: bool,
    },
    SetMobileEnabled {
        enabled: bool,
    },
    SetMobileAllowWhileLocked {
        allowed: bool,
    },
    SyncCalendarNow,
    RecordActivity {
        snapshot: ActivitySnapshot,
    },
    RecordSemantic {
        candidate: SemanticCaptureCandidate,
    },
    AnalyzeScreen {
        candidate: ScreenCaptureCandidate,
    },
    EndActivitySession {
        reason: String,
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
    IntelligenceStatus,
    SetIntelligenceSettings {
        settings: IntelligenceSettings,
    },
    RetryIntelligence,
    AskBrie {
        question: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Status { status: CaptureStatus },
    Created { remembrie: Remembrie },
    CaptureResult { decision: CaptureDecision },
    Remembries { remembries: Vec<Remembrie> },
    RecallSnapshot { snapshot: RecallSnapshot },
    Remembrie { remembrie: Option<Remembrie> },
    TimelineHistory { spans: Vec<TimelineHistorySpan> },
    TimelineDay { entries: Vec<TimelineEntry> },
    TimelineMap { slices: Vec<TimelineMapSlice> },
    SearchResults { hits: Vec<SearchHit> },
    PauseUpdated { status: CaptureStatus },
    CaptureSourceUpdated { status: CaptureStatus },
    ActivityRecorded { result: ActivityRecordResult },
    SemanticRecorded { result: SemanticCaptureResult },
    ScreenAnalyzed { result: ScreenCaptureResult },
    ActivitySessionEnded,
    CaptureRules { rules: Vec<CaptureRule> },
    CaptureRuleAdded { rule: CaptureRule },
    CaptureRuleDeleted { id: String },
    Deleted { count: u64 },
    CaptureAgentHeartbeatRecorded { status: CaptureStatus },
    BackupCreated { backup: BackupInfo },
    Backups { backups: Vec<BackupInfo> },
    IntelligenceStatus { status: IntelligenceStatus },
    IntelligenceSettingsUpdated { status: IntelligenceStatus },
    IntelligenceRetryQueued { count: u64 },
    BrieAnswered { answer: BrieAnswer },
    CalendarSynced { result: CalendarSyncResult },
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
        self.request_with_timeout(request, Duration::from_secs(10))
    }

    fn request_with_timeout(
        &self,
        request: &Request,
        timeout: Duration,
    ) -> Result<Response, ClientError> {
        let mut stream =
            UnixStream::connect(&self.socket_path).map_err(|source| ClientError::Connect {
                path: self.socket_path.display().to_string(),
                source,
            })?;
        stream.set_read_timeout(Some(timeout))?;
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

    pub fn recall_snapshot(
        &self,
        recent_limit: u32,
        upcoming_limit: u32,
    ) -> Result<RecallSnapshot, ClientError> {
        match self.request(&Request::RecallSnapshot {
            recent_limit,
            upcoming_limit,
        })? {
            Response::RecallSnapshot { snapshot } => Ok(snapshot),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn get_remembrie(&self, id: impl Into<String>) -> Result<Option<Remembrie>, ClientError> {
        match self.request(&Request::GetRemembrie { id: id.into() })? {
            Response::Remembrie { remembrie } => Ok(remembrie),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn timeline_history(
        &self,
        since_ms: i64,
        until_ms: i64,
    ) -> Result<Vec<TimelineHistorySpan>, ClientError> {
        match self.request(&Request::TimelineHistory { since_ms, until_ms })? {
            Response::TimelineHistory { spans } => Ok(spans),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn timeline_day(
        &self,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<Vec<TimelineEntry>, ClientError> {
        match self.request(&Request::TimelineDay { start_ms, end_ms })? {
            Response::TimelineDay { entries } => Ok(entries),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn timeline_map(
        &self,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<Vec<TimelineMapSlice>, ClientError> {
        match self.request(&Request::TimelineMap { start_ms, end_ms })? {
            Response::TimelineMap { slices } => Ok(slices),
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
        match self.request_with_timeout(
            &Request::Search {
                query: query.into(),
                limit,
            },
            Duration::from_secs(120),
        )? {
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

    pub fn set_activity_enabled(&self, enabled: bool) -> Result<CaptureStatus, ClientError> {
        match self.request(&Request::SetActivityEnabled { enabled })? {
            Response::CaptureSourceUpdated { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn set_activity_idle_threshold(
        &self,
        idle_threshold_ms: u64,
    ) -> Result<CaptureStatus, ClientError> {
        match self.request(&Request::SetActivityIdleThreshold { idle_threshold_ms })? {
            Response::CaptureSourceUpdated { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn set_semantic_enabled(&self, enabled: bool) -> Result<CaptureStatus, ClientError> {
        match self.request(&Request::SetSemanticEnabled { enabled })? {
            Response::CaptureSourceUpdated { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn set_semantic_sample_interval(
        &self,
        sample_interval_ms: u64,
    ) -> Result<CaptureStatus, ClientError> {
        match self.request(&Request::SetSemanticSampleInterval { sample_interval_ms })? {
            Response::CaptureSourceUpdated { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn set_screen_enabled(&self, enabled: bool) -> Result<CaptureStatus, ClientError> {
        match self.request(&Request::SetScreenEnabled { enabled })? {
            Response::CaptureSourceUpdated { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn set_screen_sample_interval(
        &self,
        sample_interval_ms: u64,
    ) -> Result<CaptureStatus, ClientError> {
        match self.request(&Request::SetScreenSampleInterval { sample_interval_ms })? {
            Response::CaptureSourceUpdated { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn set_screen_model(&self, model: impl Into<String>) -> Result<CaptureStatus, ClientError> {
        match self.request(&Request::SetScreenModel {
            model: model.into(),
        })? {
            Response::CaptureSourceUpdated { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn set_calendar_enabled(&self, enabled: bool) -> Result<CaptureStatus, ClientError> {
        match self.request(&Request::SetCalendarEnabled { enabled })? {
            Response::CaptureSourceUpdated { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn set_mobile_enabled(&self, enabled: bool) -> Result<CaptureStatus, ClientError> {
        match self.request(&Request::SetMobileEnabled { enabled })? {
            Response::CaptureSourceUpdated { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn set_mobile_allow_while_locked(
        &self,
        allowed: bool,
    ) -> Result<CaptureStatus, ClientError> {
        match self.request(&Request::SetMobileAllowWhileLocked { allowed })? {
            Response::CaptureSourceUpdated { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn sync_calendar_now(&self) -> Result<CalendarSyncResult, ClientError> {
        match self.request_with_timeout(&Request::SyncCalendarNow, Duration::from_secs(120))? {
            Response::CalendarSynced { result } => Ok(result),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn record_activity(
        &self,
        snapshot: ActivitySnapshot,
    ) -> Result<ActivityRecordResult, ClientError> {
        match self.request(&Request::RecordActivity { snapshot })? {
            Response::ActivityRecorded { result } => Ok(result),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn record_semantic(
        &self,
        candidate: SemanticCaptureCandidate,
    ) -> Result<SemanticCaptureResult, ClientError> {
        match self.request(&Request::RecordSemantic { candidate })? {
            Response::SemanticRecorded { result } => Ok(result),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn analyze_screen(
        &self,
        candidate: ScreenCaptureCandidate,
    ) -> Result<ScreenCaptureResult, ClientError> {
        match self.request_with_timeout(
            &Request::AnalyzeScreen { candidate },
            Duration::from_secs(15 * 60),
        )? {
            Response::ScreenAnalyzed { result } => Ok(result),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn end_activity_session(&self, reason: impl Into<String>) -> Result<(), ClientError> {
        match self.request(&Request::EndActivitySession {
            reason: reason.into(),
        })? {
            Response::ActivitySessionEnded => Ok(()),
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

    pub fn intelligence_status(&self) -> Result<IntelligenceStatus, ClientError> {
        match self.request(&Request::IntelligenceStatus)? {
            Response::IntelligenceStatus { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn set_intelligence_settings(
        &self,
        settings: IntelligenceSettings,
    ) -> Result<IntelligenceStatus, ClientError> {
        match self.request(&Request::SetIntelligenceSettings { settings })? {
            Response::IntelligenceSettingsUpdated { status } => Ok(status),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn retry_intelligence(&self) -> Result<u64, ClientError> {
        match self.request(&Request::RetryIntelligence)? {
            Response::IntelligenceRetryQueued { count } => Ok(count),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    pub fn ask_brie(&self, question: impl Into<String>) -> Result<BrieAnswer, ClientError> {
        match self.request_with_timeout(
            &Request::AskBrie {
                question: question.into(),
            },
            Duration::from_secs(15 * 60),
        )? {
            Response::BrieAnswered { answer } => Ok(answer),
            other => Err(ClientError::Rejected(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }
}
