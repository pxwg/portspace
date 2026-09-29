//! Harness-facing semantics live here, not in providers or client plugins.
mod fuzzy;
mod images;
pub mod pi;
pub mod search;
pub mod shell;

use crate::{ErrorCode, Result, WorkspaceError};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolProfile {
    #[default]
    Workspace,
    Pi,
}
impl std::str::FromStr for ToolProfile {
    type Err = WorkspaceError;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "workspace" => Ok(Self::Workspace),
            "pi" => Ok(Self::Pi),
            _ => Err(WorkspaceError::new(
                ErrorCode::Unsupported,
                "available tool profiles: workspace, pi",
            )),
        }
    }
}

/// Profile paths refer to a virtual root, never the core or provider home directory.
/// Reject traversal before normalization, even if it would resolve back inside.
pub fn workspace_path(input: &str) -> Result<String> {
    let input = input.strip_prefix('@').unwrap_or(input);
    let relative = if input == "/workspace" {
        "."
    } else if let Some(path) = input.strip_prefix("/workspace/") {
        path
    } else {
        input
    };
    if relative.is_empty()
        || relative.starts_with(['/', '~'])
        || relative.contains(['\0', '\\'])
        || relative.split('/').any(|p| p == "..")
    {
        return Err(WorkspaceError::new(
            ErrorCode::InvalidPath,
            "use workspace-relative paths or /workspace/...; home expansion and parent traversal are unsupported",
        ));
    }
    let normalized = relative
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect::<Vec<_>>()
        .join("/");
    Ok(if normalized.is_empty() {
        ".".into()
    } else {
        normalized
    })
}
