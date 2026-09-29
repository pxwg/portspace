//! Harness-independent workspace contract and providers.
pub mod local;
pub mod mcp;
pub mod process;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotFound,
    PermissionDenied,
    InvalidPath,
    InvalidInput,
    AmbiguousEdit,
    Conflict,
    Unsupported,
    ProcessFailed,
    Timeout,
    Cancelled,
    TransportFailure,
    WorkspaceUnavailable,
    LimitExceeded,
    Io,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceError {
    pub code: ErrorCode,
    pub message: String,
}
impl WorkspaceError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}
impl std::error::Error for WorkspaceError {}
impl From<std::io::Error> for WorkspaceError {
    fn from(e: std::io::Error) -> Self {
        use std::io::ErrorKind;
        let code = match e.kind() {
            ErrorKind::NotFound => ErrorCode::NotFound,
            ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
            ErrorKind::AlreadyExists => ErrorCode::Conflict,
            _ => ErrorCode::Io,
        };
        // Avoid returning host-native paths to the harness.
        Self::new(code, e.kind().to_string())
    }
}
pub type Result<T> = std::result::Result<T, WorkspaceError>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Utf8,
    Base64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Replacement {
    pub old_text: String,
    pub new_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Read {
        path: String,
        encoding: Encoding,
    },
    Write {
        path: String,
        content: String,
        encoding: Encoding,
    },
    Stat {
        path: String,
    },
    List {
        path: String,
    },
    Mkdir {
        path: String,
    },
    Remove {
        path: String,
        recursive: bool,
    },
    Edit {
        path: String,
        replacements: Vec<Replacement>,
    },
    Search {
        path: String,
        pattern: String,
        limit: usize,
    },
    Exec {
        program: String,
        args: Vec<String>,
        cwd: String,
        env: BTreeMap<String, String>,
        timeout_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub workspace: String,
    pub operation: Operation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub id: String,
    pub root: String,
    pub provider: String,
    pub platform: String,
    pub capabilities: Vec<String>,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn info(&self, id: &str) -> WorkspaceInfo;
    async fn execute(&self, operation: Operation) -> Result<Value>;
}

struct Entry {
    provider: Arc<dyn Provider>,
    lock: Mutex<()>,
}
#[derive(Default)]
pub struct Runtime {
    entries: BTreeMap<String, Entry>,
}
impl Runtime {
    pub fn register(&mut self, id: String, provider: Arc<dyn Provider>) -> Result<()> {
        if id.is_empty() || self.entries.contains_key(&id) {
            return Err(WorkspaceError::new(
                ErrorCode::Conflict,
                "empty or duplicate workspace ID",
            ));
        }
        self.entries.insert(
            id,
            Entry {
                provider,
                lock: Mutex::new(()),
            },
        );
        Ok(())
    }
    pub fn workspaces(&self) -> Vec<WorkspaceInfo> {
        self.entries
            .iter()
            .map(|(id, entry)| entry.provider.info(id))
            .collect()
    }
    pub async fn execute(&self, request: Request) -> Result<Value> {
        let entry = self.entries.get(&request.workspace).ok_or_else(|| {
            WorkspaceError::new(ErrorCode::WorkspaceUnavailable, "unknown workspace ID")
        })?;
        let _guard = entry.lock.lock().await;
        entry.provider.execute(request.operation).await
    }
}
