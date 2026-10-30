use thiserror::Error;

#[derive(Debug, Error)]
pub enum MaestroError {
    #[error("storage error: {0}")]
    Storage(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("plan validation failed: {0}")]
    PlanInvalid(String),

    #[error("policy denied {action} on {resource}: {reason}")]
    PolicyDenied {
        action: String,
        resource: String,
        reason: String,
    },

    /// A policy `ask` decision parked the task on a durable approval.
    #[error("approval required: {approval_id}")]
    ApprovalRequired { approval_id: String },

    #[error("provider error ({provider}): {message}")]
    Provider {
        provider: String,
        message: String,
        /// True when the caller may retry (429/5xx/timeouts).
        retryable: bool,
        /// Provider-supplied retry hint in milliseconds, if any.
        retry_after_ms: Option<i64>,
    },

    #[error("operation was cancelled")]
    Cancelled,

    #[error("budget exhausted: {0}")]
    BudgetExhausted(String),

    #[error("lease lost for task {task_id}")]
    LeaseLost { task_id: String },

    #[error("git error: {0}")]
    Git(String),

    #[error("sandbox error: {0}")]
    Sandbox(String),

    #[error("agent protocol violation: {0}")]
    Protocol(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("io error: {0}")]
    Io(String),

    #[error("{0}")]
    Other(String),
}

impl From<std::io::Error> for MaestroError {
    fn from(e: std::io::Error) -> Self {
        MaestroError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for MaestroError {
    fn from(e: serde_json::Error) -> Self {
        MaestroError::Protocol(format!("json: {e}"))
    }
}

impl MaestroError {
    pub fn other(msg: impl Into<String>) -> Self {
        MaestroError::Other(msg.into())
    }
}
