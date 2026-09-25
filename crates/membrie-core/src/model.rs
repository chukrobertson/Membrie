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
    pub database_path: String,
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
