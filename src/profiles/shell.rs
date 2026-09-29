//! Deliberately Workspace semantics, not Pi's streaming/unbounded shell backend.
use crate::{ErrorCode, Operation, Request, Result, Runtime, WorkspaceError};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BashInput {
    pub command: String,
    /// Seconds. Default 300; finite range 0.001..=300.
    pub timeout: Option<f64>,
}

pub async fn bash(runtime: &Runtime, workspace: &str, input: BashInput) -> Result<String> {
    let timeout = input.timeout.unwrap_or(300.0);
    if !timeout.is_finite() || !(0.001..=300.0).contains(&timeout) {
        return Err(WorkspaceError::new(
            ErrorCode::InvalidInput,
            "timeout must be 0.001..300 seconds (default 300)",
        ));
    }
    // No string interpolation into shell syntax. The user's script is argv[1].
    // Suppress startup files; use the provider's environment, not a core-host env snapshot.
    let result = runtime
        .execute(Request {
            workspace: workspace.into(),
            operation: Operation::Exec {
                program: "/bin/bash".into(),
                args: vec![
                    "--noprofile".into(),
                    "--norc".into(),
                    "-c".into(),
                    "exec /bin/bash --noprofile --norc -c \"$1\" 2>&1".into(),
                    "portspace".into(),
                    input.command,
                ],
                cwd: ".".into(),
                env: BTreeMap::from([("BASH_ENV".into(), "".into()), ("ENV".into(), "".into())]),
                timeout_ms: (timeout * 1000.0).ceil() as u64,
            },
        })
        .await?;
    let stdout = result["stdout"].as_str().unwrap_or_default();
    let stderr = result["stderr"].as_str().unwrap_or_default();
    let combined = format!("{stdout}{stderr}");
    let provider_truncated =
        result["stdout_truncated"] == true || result["stderr_truncated"] == true;
    let mut output = if provider_truncated {
        // Never claim that a tail of a captured prefix is the tail of the command.
        let end = floor_boundary(&combined, 50 * 1024);
        format!(
            "{}\n\n[Output incomplete: provider capture exceeded 1 MiB per stream. Showing the first at most 50 KiB of captured output; no full output artifact retained. Redirect to a workspace file to retain output.]",
            &combined[..end]
        )
    } else {
        tail(&combined)
    };
    if output.is_empty() {
        output = "(no output)".into();
    }
    if result["exit_code"] == 0 {
        return Ok(output);
    }
    let status = if let Some(code) = result["exit_code"].as_i64() {
        format!("Command exited with code {code}")
    } else {
        format!("Command terminated by signal {}", result["signal"])
    };
    Err(WorkspaceError::new(
        ErrorCode::ProcessFailed,
        format!("{output}\n\n{status}"),
    ))
}
fn floor_boundary(text: &str, max: usize) -> usize {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}
fn tail(text: &str) -> String {
    let lines: Vec<_> = text.split_inclusive('\n').collect();
    if text.len() <= 50 * 1024 && lines.len() <= 2000 {
        return text.into();
    }
    let start_line = lines.len().saturating_sub(2000);
    let selected = lines[start_line..].concat();
    let mut start = selected.len().saturating_sub(50 * 1024);
    while !selected.is_char_boundary(start) {
        start += 1;
    }
    format!(
        "{}\n\n[Showing last at most 2000 lines / 50 KiB. No full output artifact retained; redirect to a workspace file to retain output.]",
        &selected[start..]
    )
}
