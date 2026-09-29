//! Best-effort Pi 0.87.1 tool profile with bounded Workspace execution.
//! No Pi dependency at runtime. MCP tool descriptions declare supported behavior and limits.
use super::fuzzy::edit_text;
use super::workspace_path;
use crate::{Encoding, ErrorCode, Operation, Result, Runtime, WorkspaceError};
use base64::{Engine, engine::general_purpose::STANDARD};

pub struct ImageData {
    pub data: String,
    pub mime: String,
}
pub struct Output {
    pub text: String,
    pub image: Option<ImageData>,
}
impl From<String> for Output {
    fn from(text: String) -> Self {
        Self { text, image: None }
    }
}
use schemars::JsonSchema;
use serde::Deserialize;

const MAX_LINES: usize = 2000;
const MAX_BYTES: usize = 50 * 1024;
// Keep image decoding memory bounded even when several MCP reads arrive together.
static READ_WORKERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadInput {
    pub path: String,
    /// 1-indexed starting line. Defaults to 1.
    pub offset: Option<usize>,
    /// Maximum lines to read before the 2000-line / 50 KiB output cap.
    pub limit: Option<usize>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteInput {
    pub path: String,
    pub content: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditInput {
    pub path: String,
    pub edits: Vec<Edit>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Edit {
    pub old_text: String,
    pub new_text: String,
}

pub async fn read(runtime: &Runtime, workspace: &str, input: ReadInput) -> Result<Output> {
    let path = workspace_path(&input.path)?;
    let permit = READ_WORKERS
        .acquire()
        .await
        .map_err(|_| WorkspaceError::new(ErrorCode::Io, "read workers unavailable"))?;
    let value = runtime
        .execute(crate::Request {
            workspace: workspace.into(),
            operation: Operation::Read {
                path,
                encoding: Encoding::Base64,
            },
        })
        .await?;
    let bytes =
        STANDARD
            .decode(value["content"].as_str().ok_or_else(|| {
                WorkspaceError::new(ErrorCode::Io, "invalid provider read result")
            })?)
            .map_err(|_| WorkspaceError::new(ErrorCode::Io, "invalid provider base64"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if let Some(image) = super::images::read_image(&bytes)? {
            return Ok(image);
        }
        format_read(&String::from_utf8_lossy(&bytes), &input).map(Output::from)
    })
    .await
    .map_err(|_| WorkspaceError::new(ErrorCode::Io, "read worker failed"))?
}

fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Pi's text read counts the trailing split element for pagination, but excludes
/// a final empty line when applying the automatic head-truncation limit.
fn format_read(text: &str, input: &ReadInput) -> Result<String> {
    let lines: Vec<_> = text.split('\n').collect();
    let start = input.offset.unwrap_or(1).saturating_sub(1);
    if start >= lines.len() {
        return Err(WorkspaceError::new(
            ErrorCode::InvalidInput,
            format!(
                "Offset {} is beyond end of file ({} lines total)",
                input.offset.unwrap_or(1),
                lines.len()
            ),
        ));
    }
    let end = input
        .limit
        .map(|n| start.saturating_add(n).min(lines.len()))
        .unwrap_or(lines.len());
    let selected = lines[start..end].join("\n");
    let mut selected_lines: Vec<_> = selected.split('\n').collect();
    if selected.is_empty() {
        selected_lines.clear();
    } else if selected.ends_with('\n') {
        selected_lines.pop();
    }
    if selected.len() <= MAX_BYTES && selected_lines.len() <= MAX_LINES {
        return Ok(if input.limit.is_some() && end < lines.len() {
            format!(
                "{selected}\n\n[{} more lines in file. Use offset={} to continue.]",
                lines.len() - end,
                end + 1
            )
        } else {
            selected
        });
    }
    if selected_lines[0].len() > MAX_BYTES {
        // Bounded reads never split a line; callers can use Workspace bash for slices.
        return Err(WorkspaceError::new(
            ErrorCode::LimitExceeded,
            format!(
                "Line {} is {}, exceeds 50.0KB limit; partial-line reads are unsupported",
                start + 1,
                format_size(selected_lines[0].len())
            ),
        ));
    }
    let mut kept = Vec::new();
    let mut bytes = 0;
    let mut by_bytes = false;
    for line in selected_lines.iter().take(MAX_LINES) {
        let next = line.len() + usize::from(!kept.is_empty());
        if bytes + next > MAX_BYTES {
            by_bytes = true;
            break;
        }
        kept.push(*line);
        bytes += next;
    }
    let last = start + kept.len();
    Ok(format!(
        "{}\n\n[Showing lines {}-{} of {}{}. Use offset={} to continue.]",
        kept.join("\n"),
        start + 1,
        last,
        lines.len(),
        if by_bytes { " (50.0KB limit)" } else { "" },
        last + 1
    ))
}

pub async fn write(runtime: &Runtime, workspace: &str, input: WriteInput) -> Result<String> {
    let path = workspace_path(&input.path)?;
    // Validate the size before creating directories. No promise of rollback on I/O failure.
    if input.content.len() > crate::local::FILE_LIMIT {
        return Err(WorkspaceError::new(
            ErrorCode::LimitExceeded,
            "file exceeds 4 MiB",
        ));
    }
    runtime
        .with_workspace(workspace, |provider| async move {
            let parent = path
                .rsplit_once('/')
                .map(|(parent, _)| parent)
                .unwrap_or(".");
            provider
                .execute(Operation::Mkdir {
                    path: parent.into(),
                })
                .await?;
            provider
                .execute(Operation::Write {
                    path,
                    content: input.content,
                    encoding: Encoding::Utf8,
                })
                .await?;
            Ok(format!("Successfully wrote to {}", input.path))
        })
        .await
}

pub async fn edit(runtime: &Runtime, workspace: &str, input: EditInput) -> Result<String> {
    let path = workspace_path(&input.path)?;
    runtime
        .with_workspace(workspace, |provider| async move {
            let value = provider
                .execute(Operation::Read {
                    path: path.clone(),
                    encoding: Encoding::Utf8,
                })
                .await?;
            let original = value["content"].as_str().ok_or_else(|| {
                WorkspaceError::new(ErrorCode::Io, "invalid provider read result")
            })?;
            let updated = edit_text(original, &input)?;
            provider
                .execute(Operation::Write {
                    path,
                    content: updated,
                    encoding: Encoding::Utf8,
                })
                .await?;
            Ok(format!(
                "Successfully replaced {} block(s) in {}.",
                input.edits.len(),
                input.path
            ))
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(offset: Option<usize>, limit: Option<usize>) -> ReadInput {
        ReadInput {
            path: "a".into(),
            offset,
            limit,
        }
    }
    #[test]
    fn pagination_and_unicode_caps() {
        assert_eq!(format_read("", &input(None, None)).unwrap(), "");
        assert_eq!(
            format_read("a\nb\n", &input(Some(2), Some(1))).unwrap(),
            "b\n\n[1 more lines in file. Use offset=3 to continue.]"
        );
        assert_eq!(format_read("a\nb\n", &input(Some(3), None)).unwrap(), "");
        assert!(format_read("a", &input(Some(2), None)).is_err());
        let long = "中".repeat(18000);
        assert_eq!(
            format_read(&long, &input(None, None)).unwrap_err().code,
            ErrorCode::LimitExceeded
        );
        let text = "中\n".repeat(3000);
        let read = format_read(&text, &input(None, None)).unwrap();
        assert!(read.ends_with("[Showing lines 1-2000 of 3001. Use offset=2001 to continue.]"));
        let text = format!("{}\n{}", "中".repeat(10000), "中".repeat(10000));
        assert!(
            format_read(&text, &input(None, None))
                .unwrap()
                .ends_with("[Showing lines 1-1 of 2 (50.0KB limit). Use offset=2 to continue.]")
        );
    }
    #[test]
    fn edit_preserves_bom_crlf_and_validates_all_edits() {
        let mut input = EditInput {
            path: "a".into(),
            edits: vec![Edit {
                old_text: "a\nb".into(),
                new_text: "A\nB".into(),
            }],
        };
        assert_eq!(
            edit_text("\u{feff}a\r\nb\r\n", &input).unwrap(),
            "\u{feff}A\r\nB\r\n"
        );
        input.edits.push(Edit {
            old_text: "absent".into(),
            new_text: "x".into(),
        });
        assert_eq!(
            edit_text("a\nb", &input).unwrap_err().code,
            ErrorCode::Conflict
        );
    }
    #[test]
    fn profile_paths_never_resolve_against_core() {
        for path in [
            "../x",
            "/etc/passwd",
            "~/x",
            "/workspace/../x",
            "a/../x",
            "a\\x",
            "",
        ] {
            assert!(workspace_path(path).is_err(), "{path}");
        }
        assert_eq!(workspace_path("@/workspace/a/./b").unwrap(), "a/b");
    }
}
