use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Remembrie {
    pub id: String,
    pub kind: String,
    pub occurred_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub source_app: Option<String>,
    pub window_title: Option<String>,
    pub title: String,
    pub body: String,
    pub summary: Option<String>,
    pub sensitivity: String,
    pub importance: f64,
    pub pinned: bool,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewRemembrie {
    pub kind: String,
    pub title: String,
    pub body: String,
    pub source_app: Option<String>,
    pub window_title: Option<String>,
    pub occurred_at_ms: Option<i64>,
}

impl NewRemembrie {
    pub fn manual(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            kind: "note".to_owned(),
            title: title.into(),
            body: body.into(),
            source_app: Some("Membrie".to_owned()),
            window_title: None,
            occurred_at_ms: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchHit {
    pub remembrie: Remembrie,
    pub snippet: String,
    pub lexical_score: f64,
    #[serde(default)]
    pub semantic_score: Option<f64>,
    #[serde(default)]
    pub combined_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IntelligenceSettings {
    pub chat_model: String,
    pub embedding_model: String,
    pub context_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalModel {
    pub name: String,
    pub size_bytes: u64,
    pub parameter_size: Option<String>,
    pub quantization: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IntelligenceStatus {
    pub ollama_available: bool,
    pub settings: IntelligenceSettings,
    pub available_models: Vec<LocalModel>,
    pub total_remembries: u64,
    pub indexed_remembries: u64,
    pub pending_jobs: u64,
    pub running_jobs: u64,
    pub failed_jobs: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrieCitation {
    pub number: u32,
    pub remembrie: Remembrie,
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrieAnswer {
    pub answer: String,
    pub citations: Vec<BrieCitation>,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessingJob {
    pub id: String,
    pub remembrie: Remembrie,
    pub attempts: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedChunk {
    pub ordinal: u32,
    pub start_offset: usize,
    pub end_offset: usize,
    pub text: String,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaptureStatus {
    pub paused: bool,
    pub paused_until_ms: Option<i64>,
    pub clipboard_enabled: bool,
    pub remembrie_count: u64,
    pub skipped_total: u64,
    pub skipped_sensitive: u64,
    pub skipped_duplicate: u64,
    #[serde(default)]
    pub clipboard_agent_last_seen_ms: Option<i64>,
    #[serde(default)]
    pub activity_enabled: bool,
    #[serde(default = "default_activity_idle_threshold_ms")]
    pub activity_idle_threshold_ms: u64,
    #[serde(default)]
    pub activity_session_count: u64,
    #[serde(default)]
    pub activity_active_since_ms: Option<i64>,
    #[serde(default)]
    pub activity_current_app: Option<String>,
    #[serde(default)]
    pub activity_current_window: Option<String>,
    #[serde(default)]
    pub screen_enabled: bool,
    #[serde(default = "default_screen_sample_interval_ms")]
    pub screen_sample_interval_ms: u64,
    #[serde(default = "default_screen_model")]
    pub screen_model: String,
    #[serde(default)]
    pub screen_observation_count: u64,
    #[serde(default)]
    pub screen_failed_count: u64,
    pub database_path: String,
}

pub const fn default_activity_idle_threshold_ms() -> u64 {
    15 * 60 * 1000
}

pub const fn default_screen_sample_interval_ms() -> u64 {
    2 * 60 * 1000
}

pub fn default_screen_model() -> String {
    "gemma4:e2b".to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActivitySnapshot {
    pub app_id: String,
    pub app_name: String,
    pub window_title: String,
    pub idle_ms: u64,
    pub locked: bool,
    pub occurred_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActivityRecordResult {
    pub outcome: String,
    pub reason: Option<String>,
    pub session_id: Option<String>,
    #[serde(default)]
    pub screen_capture_allowed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScreenCaptureCandidate {
    pub session_id: String,
    pub screenshot_path: String,
    pub app_id: String,
    pub app_name: String,
    pub window_title: String,
    pub observed_at_ms: Option<i64>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScreenAnalysis {
    pub description: String,
    pub visible_text: String,
    pub confidence: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScreenCaptureResult {
    pub outcome: String,
    pub reason: Option<String>,
    pub observation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackupInfo {
    pub path: String,
    pub created_at_ms: i64,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaptureCandidate {
    pub kind: String,
    pub title: String,
    pub body: String,
    pub source_app: Option<String>,
    pub window_title: Option<String>,
    pub source_uri: Option<String>,
    pub occurred_at_ms: Option<i64>,
}

impl CaptureCandidate {
    pub fn clipboard(body: impl Into<String>) -> Self {
        Self {
            kind: "clipboard".to_owned(),
            title: "Clipboard".to_owned(),
            body: body.into(),
            source_app: Some("clipboard".to_owned()),
            window_title: None,
            source_uri: None,
            occurred_at_ms: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CaptureDecision {
    Stored { remembrie: Remembrie },
    Skipped { category: String, reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum PauseMode {
    Resume,
    Until { timestamp_ms: i64 },
    Indefinite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaptureRule {
    pub id: String,
    pub rule_type: String,
    pub pattern: String,
    pub label: Option<String>,
    pub enabled: bool,
    pub is_default: bool,
    pub created_at_ms: i64,
}
