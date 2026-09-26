pub mod config;
pub mod ipc;
pub mod model;
pub mod policy;
pub mod repository;

pub use config::{
    backup_dir, data_dir, database_path, mobile_token_path, screen_spool_dir, socket_path,
};
pub use ipc::{DaemonClient, Request, Response};
pub use model::{
    ActivityRecordResult, ActivitySnapshot, BackupInfo, BrieAnswer, BrieCitation,
    CalendarEventSnapshot, CalendarSnapshot, CalendarSourceSnapshot, CalendarSyncResult,
    CaptureCandidate, CaptureDecision, CaptureRule, CaptureStatus, EmbeddedChunk,
    IntelligenceSettings, IntelligenceStatus, LocalModel, NewRemembrie, PauseMode, ProcessingJob,
    Remembrie, ScreenAnalysis, ScreenCaptureCandidate, ScreenCaptureResult, SearchHit,
    SemanticCaptureCandidate, SemanticCaptureResult, TimelineActivityObservation,
    TimelineActivitySummary, TimelineEntry, TimelineHistorySpan, TimelineMapSlice,
};
pub use repository::{Repository, RepositoryError};
