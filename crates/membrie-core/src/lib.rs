pub mod config;
pub mod ipc;
pub mod model;
pub mod policy;
pub mod repository;

pub use config::{
    attachment_blob_dir, attachment_blob_path, attachment_inbox_dir, backup_dir, data_dir,
    database_path, mobile_pairing_invitation_path, mobile_tailnet_host_path, mobile_token_path,
    screen_spool_dir, socket_path, speech_model_path, transcription_spool_dir,
};
pub use ipc::{DaemonClient, Request, Response};
pub use model::{
    ActivityRecordResult, ActivitySnapshot, Attachment, AttachmentBatchImport, AttachmentImport,
    AttachmentProcessingJob, AudioTranscription, BackupInfo, BrieAnswer, BrieCitation,
    CalendarSnapshot, CalendarSourceSnapshot, CalendarSyncResult, CaptureCandidate,
    CaptureDecision, CaptureRule, CaptureStatus, EmbeddedChunk, EvidencePattern,
    IntelligenceSettings, IntelligenceStatus, LocalModel, MobileUsageSummary, NewRemembrie,
    PatternEvidence, PauseMode, ProcessingJob, Remembrie, ScreenAnalysis, ScreenCaptureCandidate,
    ScreenCaptureResult, SearchHit, SemanticCaptureCandidate, SemanticCaptureResult,
    StagedAttachmentImport, TimelineActivityObservation, TimelineActivitySummary, TimelineEntry,
    TimelineHistorySpan, TimelineMapSlice,
};
pub use repository::{Repository, RepositoryError};
