//! Optional read-only tools. All filesystem access goes through the bound provider.
use super::workspace_path;
use crate::{Encoding, ErrorCode, Operation, Provider, Result, Runtime, WorkspaceError};
use globset::{GlobBuilder, GlobMatcher};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use regex::RegexBuilder;
use schemars::JsonSchema;
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LsInput {
    pub path: Option<String>,
    pub limit: Option<usize>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindInput {
    pub pattern: String,
    pub path: Option<String>,
    pub limit: Option<usize>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GrepInput {
    pub pattern: String,
    pub path: Option<String>,
    pub glob: Option<String>,
    pub ignore_case: Option<bool>,
    pub literal: Option<bool>,
    pub context: Option<usize>,
    pub limit: Option<usize>,
}
fn error(message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::new(ErrorCode::InvalidInput, message)
}
fn limit(value: Option<usize>, default: usize) -> Result<usize> {
    let value = value.unwrap_or(default);
    if !(1..=10000).contains(&value) {
        return Err(error("limit must be 1..10000"));
    }
    Ok(value)
}
fn path(value: Option<&str>) -> Result<String> {
    workspace_path(value.filter(|s| !s.is_empty()).unwrap_or("."))
}
fn join(dir: &str, name: &str) -> String {
    if dir == "." {
        name.into()
    } else {
        format!("{dir}/{name}")
    }
}
fn symbolic(path: &str) -> PathBuf {
    PathBuf::from("/workspace").join(path)
}
fn relative<'a>(root: &str, path: &'a str) -> &'a str {
    if root == "." {
        path
    } else {
        path.strip_prefix(root)
            .unwrap_or(path)
            .trim_start_matches('/')
    }
}
fn glob(pattern: &str) -> Result<GlobMatcher> {
    if pattern.len() > 8192 {
        return Err(error("glob exceeds 8192 bytes"));
    }
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|e| error(e.to_string()))
}
fn glob_matches(matcher: &GlobMatcher, pattern: &str, name: &str) -> bool {
    matcher.is_match(if pattern.contains('/') {
        name
    } else {
        name.rsplit('/').next().unwrap_or(name)
    })
}
fn bounded(lines: &[String], mut notices: Vec<String>, empty: &str) -> String {
    let mut out = String::new();
    for line in lines {
        if out.len() + line.len() + usize::from(!out.is_empty()) > 50 * 1024 {
            notices.push("50.0KB limit reached".into());
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    if out.is_empty() && notices.is_empty() {
        out = empty.into();
    }
    if !notices.is_empty() {
        out.push_str(&format!("\n\n[{}]", notices.join(". ")));
    }
    out
}

pub async fn ls(runtime: &Runtime, workspace: &str, input: LsInput) -> Result<String> {
    let path = path(input.path.as_deref())?;
    let limit = limit(input.limit, 500)?;
    runtime
        .with_workspace(workspace, |p| async move {
            let entries = p.execute(Operation::List { path }).await?;
            let mut names = Vec::new();
            let mut skipped = 0;
            for entry in entries["entries"]
                .as_array()
                .ok_or_else(|| error("invalid list result"))?
            {
                if entry["kind"] == "symlink" {
                    skipped += 1;
                    continue;
                }
                names.push(format!(
                    "{}{}",
                    entry["name"].as_str().unwrap_or_default(),
                    if entry["kind"] == "directory" {
                        "/"
                    } else {
                        ""
                    }
                ));
            }
            names.sort_by_cached_key(|name| (name.to_lowercase(), name.clone()));
            let mut notices = Vec::new();
            if names.len() > limit {
                notices.push(format!(
                    "{limit} entries limit reached. Use limit={} for more",
                    limit * 2
                ));
                names.truncate(limit);
            }
            if skipped > 0 {
                notices.push(format!("{skipped} symlinks skipped"));
            }
            Ok(bounded(&names, notices, "(empty directory)"))
        })
        .await
}

struct Entry {
    path: String,
    directory: bool,
}
struct Walk {
    entries: Vec<Entry>,
    notices: Vec<String>,
}

async fn walk(provider: &Arc<dyn Provider>, root: &str) -> Result<Walk> {
    let kind = provider
        .execute(Operation::Stat { path: root.into() })
        .await?;
    if kind["kind"] != "directory" {
        return Err(error("search root must be a directory"));
    }
    let mut stack: Vec<(String, Vec<Arc<Gitignore>>)> = vec![(root.into(), Vec::new())];
    let mut result = Walk {
        entries: Vec::new(),
        notices: Vec::new(),
    };
    let mut visited = 0;
    let mut symlinks = 0;
    let mut rules_bytes = 0;
    while let Some((dir, mut rules)) = stack.pop() {
        let mut builder = GitignoreBuilder::new(symbolic(&dir));
        for filename in [".gitignore", ".ignore"] {
            let contents = provider
                .execute(Operation::Read {
                    path: join(&dir, filename),
                    encoding: Encoding::Utf8,
                })
                .await;
            match contents {
                Ok(value) => {
                    let text = value["content"]
                        .as_str()
                        .ok_or_else(|| error("invalid ignore file"))?;
                    rules_bytes += text.len();
                    if rules_bytes > 1024 * 1024 {
                        return Err(WorkspaceError::new(
                            ErrorCode::LimitExceeded,
                            "ignore rules exceed 1 MiB",
                        ));
                    }
                    for line in text.lines() {
                        builder
                            .add_line(None, line)
                            .map_err(|e| error(format!("invalid ignore rule: {e}")))?;
                    }
                }
                Err(e) if e.code == ErrorCode::NotFound || e.code == ErrorCode::InvalidPath => {}
                Err(e) => return Err(e),
            }
        }
        rules.push(Arc::new(builder.build().map_err(|e| error(e.to_string()))?));
        let entries = provider
            .execute(Operation::List { path: dir.clone() })
            .await?;
        let mut dirs = Vec::new();
        for entry in entries["entries"]
            .as_array()
            .ok_or_else(|| error("invalid list result"))?
        {
            visited += 1;
            if visited > 10000 {
                result
                    .notices
                    .push("10000 visited entries limit reached; results incomplete".into());
                if symlinks > 0 {
                    result.notices.push(format!("{symlinks} symlinks skipped"));
                }
                return Ok(result);
            }
            let name = entry["name"]
                .as_str()
                .ok_or_else(|| error("invalid entry name"))?;
            if name == ".git" {
                continue;
            }
            if entry["kind"] == "symlink" {
                symlinks += 1;
                continue;
            }
            let path = join(&dir, name);
            let directory = entry["kind"] == "directory";
            let ignored = rules
                .iter()
                .rev()
                .map(|r| r.matched(symbolic(&path), directory))
                .find(|m| !m.is_none())
                .is_some_and(|m| m.is_ignore());
            if ignored {
                continue;
            }
            if directory {
                dirs.push((path.clone(), rules.clone()));
            }
            result.entries.push(Entry { path, directory });
        }
        stack.extend(dirs.into_iter().rev());
        tokio::task::yield_now().await;
    }
    if symlinks > 0 {
        result.notices.push(format!("{symlinks} symlinks skipped"));
    }
    result.entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

pub async fn find(runtime: &Runtime, workspace: &str, input: FindInput) -> Result<String> {
    let root = path(input.path.as_deref())?;
    let limit = limit(input.limit, 1000)?;
    let matcher = glob(&input.pattern)?;
    runtime.with_workspace(workspace, |p| async move {
        let Walk { entries,mut notices } = walk(&p,&root).await?;
        let mut matches = Vec::new();
        for entry in entries {
            let name = relative(&root,&entry.path);
            if glob_matches(&matcher,&input.pattern,name) {
                matches.push(format!("{name}{}",if entry.directory { "/" } else { "" }));
                if matches.len() >= limit { notices.push(format!("{limit} results limit reached. Use limit={} for more, or refine pattern",limit*2)); break; }
            }
        }
        Ok(bounded(&matches,notices,"No files found matching pattern"))
    }).await
}

pub async fn grep(runtime: &Runtime, workspace: &str, input: GrepInput) -> Result<String> {
    let root = path(input.path.as_deref())?;
    let limit = limit(input.limit, 100)?;
    let context = input.context.unwrap_or(0);
    if context > 1000 || input.pattern.len() > 8192 {
        return Err(error("context must be <=1000 and pattern <=8192 bytes"));
    }
    let pattern = if input.literal.unwrap_or(false) {
        regex::escape(&input.pattern)
    } else {
        input.pattern.clone()
    };
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(input.ignore_case.unwrap_or(false))
        .size_limit(1024 * 1024)
        .build()
        .map_err(|e| error(e.to_string()))?;
    let matcher = input.glob.as_deref().map(glob).transpose()?;
    runtime.with_workspace(workspace, |p| async move {
        let kind = p.execute(Operation::Stat { path: root.clone() }).await?;
        let is_dir = kind["kind"] == "directory";
        let Walk { entries,mut notices } = if is_dir { walk(&p,&root).await? } else { Walk { entries: vec![Entry { path: root.clone(),directory:false }], notices: Vec::new() } };
        let mut output = Vec::new();
        let mut count = 0;
        let mut skipped = 0;
        let mut shortened = false;
        let mut output_bytes = 0;
        let mut bytes_read = 0;
        'files: for entry in entries {
            if entry.directory { continue; }
            let name = if is_dir { relative(&root,&entry.path) } else { entry.path.rsplit('/').next().unwrap_or(&entry.path) };
            if matcher.as_ref().is_some_and(|m| !glob_matches(m,input.glob.as_deref().unwrap(),name)) { continue; }
            let result = p.execute(Operation::Read { path: entry.path.clone(),encoding: Encoding::Utf8 }).await;
            let value = match result {
                Ok(v) => v,
                Err(e) if e.code == ErrorCode::LimitExceeded || e.code == ErrorCode::InvalidInput => { skipped+=1; continue; },
                Err(e) => return Err(e),
            };
            let content = value["content"].as_str().ok_or_else(|| error("invalid read result"))?;
            bytes_read += content.len();
            if bytes_read > 64*1024*1024 { notices.push("64 MiB scan budget reached; results incomplete".into()); break; }
            if content.contains('\0') { skipped+=1; continue; }
            let content = content.replace("\r\n","\n").replace('\r',"\n");
            let lines: Vec<_> = content.split('\n').collect();
            // rg does not report the synthetic empty line after a terminal newline.
            let searchable = lines.len() - usize::from(content.ends_with('\n') || content.is_empty());
            for (i,line) in lines.iter().take(searchable).enumerate() {
                if !regex.is_match(line) { continue; }
                count += 1;
                for (j,line) in lines.iter().enumerate().take((i+context+1).min(lines.len())).skip(i.saturating_sub(context)) {
                    let mut text: String = line.chars().take(500).collect();
                    if line.chars().count() > 500 { text.push_str("... [truncated]"); shortened=true; }
                    let row = if i==j { format!("{name}:{}: {text}",j+1) } else { format!("{name}-{}- {text}",j+1) };
                    output_bytes += row.len()+1;
                    if output_bytes > 50*1024 { notices.push("50.0KB limit reached".into()); break 'files; }
                    output.push(row);
                }
                if count >= limit { notices.push(format!("{limit} matches limit reached. Use limit={} for more, or refine pattern",limit*2)); break 'files; }
            }
            tokio::task::yield_now().await;
        }
        if skipped>0 { notices.push(format!("{skipped} binary or oversized files skipped")); }
        if shortened { notices.push("Some lines truncated to 500 chars. Use read tool to see full lines".into()); }
        Ok(bounded(&output,notices,"No matches found"))
    }).await
}
