use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("{0}")]
    Invalid(String),
    #[error("{message}")]
    ModelResponseRejected {
        message: String,
        record: Box<ModelCall>,
    },
    #[error("Operation cancelled; completed stages are retained")]
    Cancelled,
    #[error("Operation exceeded its execution deadline")]
    TimedOut,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid task data: {0}")]
    Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, AgentError>;

#[derive(Clone, Default)]
pub struct CancelToken(pub Arc<AtomicBool>);
impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(AgentError::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub initial_frames: u32,
    pub max_frames: u32,
    pub max_model_calls: u32,
    pub max_output_tokens: u32,
    pub max_total_tokens: u64,
    pub max_estimated_cost_usd: f64,
    pub max_execution_secs: u64,
    pub process_timeout_secs: u64,
    pub model_timeout_secs: u64,
    pub max_sources: usize,
    pub max_concurrency: usize,
    pub max_clip_ms: u64,
    pub max_output_ms: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            initial_frames: 12,
            max_frames: 36,
            max_model_calls: 4,
            max_output_tokens: 2500,
            max_total_tokens: 100_000,
            max_estimated_cost_usd: 0.25,
            max_execution_secs: 600,
            process_timeout_secs: 120,
            model_timeout_secs: 45,
            max_sources: 3,
            max_concurrency: 1,
            max_clip_ms: 15_000,
            max_output_ms: 120_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub id: String,
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
    pub modified_ns: u64,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub id: String,
    pub source_id: String,
    pub time_ms: u64,
    pub image_path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClipProposal {
    pub source_id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub reason: String,
    pub uncertainty: String,
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Clip {
    pub id: String,
    pub source_id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub reason: String,
    pub uncertainty: String,
    pub evidence_ids: Vec<String>,
    pub human_modified: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioPolicy {
    PreserveWithSilence,
    Mute,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EditPlan {
    pub revision: u64,
    pub clips: Vec<Clip>,
    pub summary: String,
    pub audio_policy: AudioPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Approval {
    pub plan_digest: String,
    pub revision: u64,
    pub confirmed_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Created,
    Sampling,
    AwaitingConsent,
    Analyzing,
    Review,
    Exporting,
    Completed,
    Cancelled,
    Failed,
    Stale,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
    pub frames_created: u32,
    pub model_calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Estimate from configured price rates, NOT a provider invoice.
    pub estimated_cost_usd: f64,
    pub elapsed_ms: u64,
    /// In-flight durable upper bound, not measured/billed execution time.
    #[serde(default)]
    pub execution_reserved_ms: u64,
    /// Included in elapsed_ms: abandoned step bounds charged after a crash.
    #[serde(default)]
    pub conservative_elapsed_ms: u64,
    /// Zero only on older pre-ledger tasks; one requires its durable journal.
    #[serde(default)]
    pub execution_accounting_version: u32,
    /// Peak overlapping local extraction workers observed by completed batches.
    #[serde(default)]
    pub peak_media_workers: u32,
    /// Sum of returned extraction workers' wall time, not execution-budget time.
    #[serde(default)]
    pub media_worker_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelCall {
    pub call_id: String,
    pub backend: String,
    #[serde(default)]
    pub requested_model: String,
    /// Actual Responses-reported model when a response was received; requested model on a reservation.
    pub model: String,
    pub prompt_version: String,
    pub sample_ids: Vec<String>,
    pub provider_response_id: Option<String>,
    pub request_id: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_usd: f64,
    pub elapsed_ms: u64,
    pub status: String,
    pub decision: Option<VisionDecision>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
pub enum VisionDecision {
    ObserveInterval {
        source_id: String,
        start_ms: u64,
        end_ms: u64,
        count: u32,
        reason: String,
    },
    ProposePlan {
        clips: Vec<ClipProposal>,
        summary: String,
    },
}

#[derive(Debug, Clone)]
pub struct VisionRequest {
    pub call_id: String,
    pub intent: String,
    pub target_duration_ms: u64,
    pub assets: Vec<Asset>,
    pub samples: Vec<Sample>,
    pub remaining_frames: u32,
    pub limits: Limits,
    pub feedback: Option<String>,
}

pub struct VisionOutcome {
    pub decision: VisionDecision,
    pub record: ModelCall,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub at: String,
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Shard {
    pub clip_digest: String,
    pub path: PathBuf,
    pub sha256: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportRecord {
    pub plan_digest: String,
    pub path: PathBuf,
    pub sha256: String,
    pub duration_ms: u64,
    pub has_audio: bool,
    pub verified: bool,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub schema_version: u32,
    pub id: String,
    pub created_at: String,
    pub intent: String,
    pub target_duration_ms: u64,
    pub assets: Vec<Asset>,
    pub samples: Vec<Sample>,
    pub model_calls: Vec<ModelCall>,
    pub plan: Option<EditPlan>,
    pub approval: Option<Approval>,
    pub status: TaskStatus,
    pub last_error: Option<String>,
    pub limits: Limits,
    pub usage: Usage,
    /// Consent covers JPEG samples up to max_frames for these exact asset hashes.
    pub cloud_consent: bool,
    pub export: Option<ExportRecord>,
    pub shards: Vec<Shard>,
    pub history: Vec<Event>,
}
