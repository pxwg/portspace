//! Claude-shaped, bounded workspace tools; not a Claude Code runtime replica.
use super::{pi, shell, workspace_path};
use crate::{Encoding, ErrorCode, Operation, Result, Runtime, WorkspaceError};
use base64::{Engine, engine::general_purpose::STANDARD};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::VecDeque;
use tokio::sync::Mutex;

// Bounded session-local snapshots. Eviction requires another Read, never bypasses checking.
#[derive(Default)]
pub struct State(Mutex<VecDeque<(String, String)>>);
impl State {
    async fn remember(&self, path: String, text: String) {
        let mut seen = self.0.lock().await;
        seen.retain(|(key, _)| key != &path);
        if seen.len() >= 16 {
            seen.pop_front();
        }
        seen.push_back((path, text));
    }
    async fn check(&self, path: &str, text: &str) -> Result<()> {
        if self
            .0
            .lock()
            .await
            .iter()
            .any(|(key, old)| key == path && old == text)
        {
            Ok(())
        } else {
            Err(invalid(
                "Read this file first; it is unread, changed, or its snapshot was evicted",
            ))
        }
    }
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GlobInput {
    pub pattern: String,
    pub path: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrepInput {
    pub pattern: String,
    pub path: Option<String>,
    pub glob: Option<String>,
    pub output_mode: Option<String>,
    #[serde(rename = "-i")]
    pub ignore_case: Option<bool>,
    #[serde(rename = "-n")]
    pub line_numbers: Option<bool>,
    #[serde(rename = "-A")]
    pub after: Option<usize>,
    #[serde(rename = "-B")]
    pub before: Option<usize>,
    #[serde(rename = "-C")]
    pub context: Option<usize>,
    pub head_limit: Option<usize>,
    pub offset: Option<usize>,
    pub multiline: Option<bool>,
    #[serde(rename = "type")]
    pub file_type: Option<String>,
}
pub async fn glob(runtime: &Runtime, workspace: &str, input: GlobInput) -> Result<String> {
    use super::search;
    let root = workspace_path(input.path.as_deref().unwrap_or("."))?;
    let matcher = search::glob(&input.pattern)?;
    runtime
        .with_workspace(workspace, |p| async move {
            let search::Walk {
                entries,
                mut notices,
            } = search::walk(&p, &root).await?;
            let mut rows = Vec::new();
            for entry in entries {
                let name = if root == "." {
                    entry.path.as_str()
                } else {
                    entry
                        .path
                        .strip_prefix(&format!("{root}/"))
                        .unwrap_or(&entry.path)
                };
                if !entry.directory && search::glob_matches(&matcher, &input.pattern, name) {
                    rows.push(format!("/workspace/{}", entry.path));
                    if rows.len() == 1000 {
                        notices.push("1000 results limit reached".into());
                        break;
                    }
                }
            }
            Ok(search::bounded(&rows, notices, "No files found"))
        })
        .await
}
pub async fn grep(runtime: &Runtime, workspace: &str, input: GrepInput) -> Result<String> {
    use super::search;
    if input.multiline.unwrap_or(false) || input.file_type.is_some() {
        return Err(unsupported(
            "multiline and type filters are unsupported; use glob for file filtering",
        ));
    }
    let mode = input.output_mode.as_deref().unwrap_or("files_with_matches");
    if !["files_with_matches", "content", "count"].contains(&mode) {
        return Err(invalid("invalid output_mode"));
    }
    let before = input.context.or(input.before).unwrap_or(0);
    let after = input.context.or(input.after).unwrap_or(0);
    if before > 1000 || after > 1000 || input.pattern.len() > 8192 {
        return Err(invalid("context <=1000 and pattern <=8192 bytes required"));
    }
    let re = regex::RegexBuilder::new(&input.pattern)
        .case_insensitive(input.ignore_case.unwrap_or(false))
        .size_limit(1024 * 1024)
        .build()
        .map_err(|e| invalid(&e.to_string()))?;
    let matcher = input.glob.as_deref().map(search::glob).transpose()?;
    let root = workspace_path(input.path.as_deref().unwrap_or("."))?;
    let head = match input.head_limit.unwrap_or(200) {
        0 => 10000,
        n => n.min(10000),
    };
    let offset = input.offset.unwrap_or(0);
    runtime
        .with_workspace(workspace, |p| async move {
            let stat = p.execute(Operation::Stat { path: root.clone() }).await?;
            let search::Walk {
                entries,
                mut notices,
            } = if stat["kind"] == "directory" {
                search::walk(&p, &root).await?
            } else {
                search::Walk {
                    entries: vec![search::Entry {
                        path: root.clone(),
                        directory: false,
                    }],
                    notices: vec![],
                }
            };
            let mut rows = Vec::new();
            let mut scanned = 0;
            let mut seen = 0;
            let mut bytes = 0;
            let mut skipped = 0;
            'files: for entry in entries {
                if entry.directory {
                    continue;
                }
                let relative = if root == "." {
                    entry.path.as_str()
                } else {
                    entry
                        .path
                        .strip_prefix(&format!("{root}/"))
                        .unwrap_or(&entry.path)
                };
                if matcher.as_ref().is_some_and(|m| {
                    !search::glob_matches(m, input.glob.as_deref().unwrap(), relative)
                }) {
                    continue;
                }
                let value = match p
                    .execute(Operation::Read {
                        path: entry.path.clone(),
                        encoding: Encoding::Utf8,
                    })
                    .await
                {
                    Ok(v) => v,
                    Err(e)
                        if e.code == ErrorCode::LimitExceeded
                            || e.code == ErrorCode::InvalidInput =>
                    {
                        skipped += 1;
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                let text = value["content"]
                    .as_str()
                    .ok_or_else(|| invalid("invalid provider read"))?;
                scanned += text.len();
                if scanned > 64 * 1024 * 1024 {
                    notices.push("64 MiB scan budget reached; results incomplete".into());
                    break;
                }
                if text.contains('\0') {
                    skipped += 1;
                    continue;
                }
                let lines: Vec<_> = text.lines().collect();
                let hits: Vec<_> = lines
                    .iter()
                    .enumerate()
                    .filter_map(|(i, l)| re.is_match(l).then_some(i))
                    .collect();
                if hits.is_empty() {
                    continue;
                }
                let name = format!("/workspace/{}", entry.path);
                let mut selected = Vec::new();
                if mode == "content" {
                    let mut end = 0;
                    for &i in &hits {
                        let next = (i.saturating_add(after) + 1).min(lines.len());
                        selected.extend(i.saturating_sub(before).max(end)..next);
                        end = next;
                    }
                }
                // Render incrementally: even head_limit=0 is subject to the output budget.
                let single = if mode == "count" {
                    format!("{name}:{}", hits.len())
                } else {
                    name.clone()
                };
                let rendered: Box<dyn Iterator<Item = String> + Send> = if mode == "content" {
                    Box::new(selected.into_iter().map(|i| {
                        let line: String = lines[i].chars().take(2000).collect();
                        let suffix = if lines[i].chars().count() > 2000 {
                            " [line truncated]"
                        } else {
                            ""
                        };
                        if input.line_numbers.unwrap_or(true) {
                            format!("{name}:{}:{line}{suffix}", i + 1)
                        } else {
                            format!("{name}:{line}{suffix}")
                        }
                    }))
                } else {
                    Box::new(std::iter::once(single))
                };
                for row in rendered {
                    if seen < offset {
                        seen += 1;
                        continue;
                    }
                    if rows.len() >= head || bytes + row.len() + 1 > 50 * 1024 {
                        notices.push(
                            "Output limit reached; refine search or paginate with offset".into(),
                        );
                        break 'files;
                    }
                    bytes += row.len() + 1;
                    rows.push(row);
                }
                tokio::task::yield_now().await;
            }
            if skipped > 0 {
                notices.push(format!("{skipped} binary or oversized files skipped"));
            }
            Ok(search::bounded(&rows, notices, "No matches found"))
        })
        .await
}

fn invalid(message: &str) -> WorkspaceError {
    WorkspaceError::new(ErrorCode::InvalidInput, message)
}
fn unsupported(message: &str) -> WorkspaceError {
    WorkspaceError::new(ErrorCode::Unsupported, message)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadInput {
    pub file_path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    /// PDF pages are unsupported by this profile.
    pub pages: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteInput {
    pub file_path: String,
    pub content: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditInput {
    pub file_path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BashInput {
    pub command: String,
    /// Milliseconds, 1..300000; defaults to 120000.
    pub timeout: Option<u64>,
    pub description: Option<String>,
    #[serde(default)]
    pub run_in_background: bool,
    #[serde(default, rename = "dangerouslyDisableSandbox")]
    pub dangerously_disable_sandbox: bool,
}
pub async fn bash(runtime: &Runtime, workspace: &str, input: BashInput) -> Result<String> {
    if input.run_in_background || input.dangerously_disable_sandbox {
        return Err(unsupported(
            "Background tasks and sandbox overrides are unsupported; Workspace exec is not sandboxed",
        ));
    }
    let timeout = input.timeout.unwrap_or(120000);
    if !(1..=300000).contains(&timeout) {
        return Err(invalid("timeout must be 1..300000 milliseconds"));
    }
    shell::bash(
        runtime,
        workspace,
        shell::BashInput {
            command: input.command,
            timeout: Some(timeout as f64 / 1000.0),
        },
    )
    .await
}

pub async fn read(
    runtime: &Runtime,
    workspace: &str,
    state: &State,
    input: ReadInput,
) -> Result<pi::Output> {
    let path = workspace_path(&input.file_path)?;
    if input.pages.is_some()
        || path.to_ascii_lowercase().ends_with(".pdf")
        || path.to_ascii_lowercase().ends_with(".ipynb")
    {
        return Err(unsupported("PDF and notebook rendering are unsupported"));
    }
    if input.offset == Some(0) || input.limit == Some(0) {
        return Err(invalid("offset and limit must be positive"));
    }
    runtime
        .with_workspace(workspace, |p| async move {
            let value = p
                .execute(Operation::Read {
                    path: path.clone(),
                    encoding: Encoding::Base64,
                })
                .await?;
            let bytes = STANDARD
                .decode(
                    value["content"]
                        .as_str()
                        .ok_or_else(|| invalid("invalid provider read"))?,
                )
                .map_err(|_| invalid("invalid base64"))?;
            if bytes.starts_with(b"%PDF-") {
                return Err(unsupported("PDF rendering is unsupported"));
            }
            let (output, text) = tokio::task::spawn_blocking(move || -> Result<_> {
                if let Some(image) = super::images::read_image(&bytes)? {
                    return Ok((image, None));
                }
                let text = String::from_utf8(bytes)
                    .map_err(|_| unsupported("Binary files are unsupported"))?;
                if text.contains('\0') {
                    return Err(unsupported("Binary files are unsupported"));
                }
                let output = format_read(
                    &text,
                    input.offset.unwrap_or(1),
                    input.limit.unwrap_or(2000),
                );
                Ok((pi::Output::from(output), Some(text)))
            })
            .await
            .map_err(|_| invalid("read worker failed"))??;
            if let Some(text) = text {
                state.remember(path, text).await;
            }
            Ok(output)
        })
        .await
}
fn format_read(text: &str, offset: usize, limit: usize) -> String {
    if text.is_empty() {
        return "[File is empty]".into();
    }
    let mut output = String::new();
    for (count, (i, line)) in text.lines().enumerate().skip(offset - 1).enumerate() {
        let clipped: String = line.chars().take(2000).collect();
        let row = format!(
            "{:>6}\u{2192}{}{}\n",
            i + 1,
            clipped,
            if line.chars().count() > 2000 {
                " [line truncated]"
            } else {
                ""
            }
        );
        if count >= limit.min(2000) || output.len() + row.len() > 50 * 1024 {
            output.push_str(&format!("[Output limited; continue with offset={}]", i + 1));
            break;
        }
        output.push_str(&row);
    }
    if output.is_empty() {
        output.push_str("[Offset is beyond end of file]");
    }
    output
}

pub async fn write(
    runtime: &Runtime,
    workspace: &str,
    state: &State,
    input: WriteInput,
) -> Result<String> {
    let path = workspace_path(&input.file_path)?;
    if input.content.len() > crate::local::FILE_LIMIT {
        return Err(WorkspaceError::new(
            ErrorCode::LimitExceeded,
            "file exceeds 4 MiB",
        ));
    }
    runtime
        .with_workspace(workspace, |p| async move {
            match p
                .execute(Operation::Read {
                    path: path.clone(),
                    encoding: Encoding::Utf8,
                })
                .await
            {
                Ok(value) => {
                    state
                        .check(
                            &path,
                            value["content"]
                                .as_str()
                                .ok_or_else(|| invalid("invalid read result"))?,
                        )
                        .await?
                }
                Err(e) if e.code == ErrorCode::NotFound => {}
                Err(e) => return Err(e),
            }
            p.execute(Operation::Mkdir {
                path: path.rsplit_once('/').map(|x| x.0).unwrap_or(".").into(),
            })
            .await?;
            p.execute(Operation::Write {
                path: path.clone(),
                content: input.content.clone(),
                encoding: Encoding::Utf8,
            })
            .await?;
            state.remember(path, input.content).await;
            Ok(format!("File written: {}", input.file_path))
        })
        .await
}
pub async fn edit(
    runtime: &Runtime,
    workspace: &str,
    state: &State,
    input: EditInput,
) -> Result<String> {
    let path = workspace_path(&input.file_path)?;
    if input.old_string.is_empty() || input.old_string == input.new_string {
        return Err(invalid(
            "old_string must be nonempty and differ from new_string",
        ));
    }
    runtime
        .with_workspace(workspace, |p| async move {
            let value = p
                .execute(Operation::Read {
                    path: path.clone(),
                    encoding: Encoding::Utf8,
                })
                .await?;
            let original = value["content"]
                .as_str()
                .ok_or_else(|| invalid("invalid read result"))?;
            state.check(&path, original).await?;
            let count = original.matches(&input.old_string).count();
            if count == 0 || (count > 1 && !input.replace_all) {
                return Err(invalid(
                    "old_string must match exactly and uniquely unless replace_all is true",
                ));
            }
            let size = original
                .len()
                .checked_sub(count * input.old_string.len())
                .and_then(|n| {
                    count
                        .checked_mul(input.new_string.len())
                        .and_then(|added| n.checked_add(added))
                });
            if size.is_none_or(|n| n > crate::local::FILE_LIMIT) {
                return Err(WorkspaceError::new(
                    ErrorCode::LimitExceeded,
                    "file exceeds 4 MiB",
                ));
            }
            let content = original.replace(&input.old_string, &input.new_string);
            p.execute(Operation::Write {
                path: path.clone(),
                content: content.clone(),
                encoding: Encoding::Utf8,
            })
            .await?;
            state.remember(path, content).await;
            Ok(format!(
                "Updated {} ({count} replacement(s)).",
                input.file_path
            ))
        })
        .await
}
