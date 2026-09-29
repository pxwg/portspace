use crate::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

pub const FILE_LIMIT: usize = 4 * 1024 * 1024;
const ENTRY_LIMIT: usize = 10_000;

pub struct LocalProvider {
    root: PathBuf,
}
impl LocalProvider {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(WorkspaceError::new(
                ErrorCode::InvalidPath,
                "root must be a directory",
            ));
        }
        Ok(Self { root })
    }
    fn resolve(&self, logical: &str) -> Result<PathBuf> {
        if logical.is_empty() {
            return Err(WorkspaceError::new(ErrorCode::InvalidPath, "empty path"));
        }
        let mut path = self.root.clone();
        for component in Path::new(logical).components() {
            match component {
                Component::CurDir => {}
                Component::Normal(p) => {
                    path.push(p);
                    match fs::symlink_metadata(&path) {
                        Ok(m) if m.file_type().is_symlink() => {
                            return Err(WorkspaceError::new(
                                ErrorCode::InvalidPath,
                                "symlinks are not supported",
                            ));
                        }
                        Ok(_) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                _ => {
                    return Err(WorkspaceError::new(
                        ErrorCode::InvalidPath,
                        "use workspace-relative paths without parent traversal",
                    ));
                }
            }
        }
        Ok(path)
    }
    fn bytes(&self, path: &Path) -> Result<Vec<u8>> {
        if !fs::metadata(path)?.is_file() {
            return Err(WorkspaceError::new(
                ErrorCode::InvalidPath,
                "expected regular file",
            ));
        }
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take((FILE_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > FILE_LIMIT {
            return Err(WorkspaceError::new(
                ErrorCode::LimitExceeded,
                "file exceeds 4 MiB",
            ));
        }
        Ok(bytes)
    }
    fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        if bytes.len() > FILE_LIMIT {
            return Err(WorkspaceError::new(
                ErrorCode::LimitExceeded,
                "file exceeds 4 MiB",
            ));
        }
        if path == self.root {
            return Err(WorkspaceError::new(
                ErrorCode::InvalidPath,
                "cannot replace workspace root",
            ));
        }
        let mut temp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        match fs::metadata(path) {
            Ok(meta) => {
                if !meta.is_file() {
                    return Err(WorkspaceError::new(
                        ErrorCode::InvalidPath,
                        "expected regular file",
                    ));
                }
                temp.as_file().set_permissions(meta.permissions())?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        temp.write_all(bytes)?;
        temp.persist(path)
            .map_err(|e| WorkspaceError::from(e.error))?;
        Ok(())
    }
    fn filesystem(&self, operation: Operation) -> Result<Value> {
        match operation {
            Operation::Read { path, encoding } => {
                let bytes = self.bytes(&self.resolve(&path)?)?;
                let size = bytes.len();
                let content = match encoding {
                    Encoding::Utf8 => String::from_utf8(bytes).map_err(|_| {
                        WorkspaceError::new(ErrorCode::InvalidInput, "not UTF-8; request base64")
                    })?,
                    Encoding::Base64 => STANDARD.encode(bytes),
                };
                Ok(json!({"content":content,"bytes":size,"encoding":encoding}))
            }
            Operation::Write {
                path,
                content,
                encoding,
            } => {
                let bytes = match encoding {
                    Encoding::Utf8 => content.into_bytes(),
                    Encoding::Base64 => STANDARD.decode(content).map_err(|_| {
                        WorkspaceError::new(ErrorCode::InvalidInput, "invalid base64")
                    })?,
                };
                self.atomic_write(&self.resolve(&path)?, &bytes)?;
                Ok(json!({"bytes":bytes.len()}))
            }
            Operation::Stat { path } => {
                let meta = fs::metadata(self.resolve(&path)?)?;
                Ok(
                    json!({"kind":if meta.is_dir() {"directory"} else if meta.is_file() {"file"} else {"other"},"bytes":meta.len()}),
                )
            }
            Operation::List { path } => {
                let mut entries = Vec::new();
                for entry in fs::read_dir(self.resolve(&path)?)? {
                    if entries.len() == ENTRY_LIMIT {
                        return Err(WorkspaceError::new(
                            ErrorCode::LimitExceeded,
                            "directory exceeds entry limit",
                        ));
                    }
                    let entry = entry?;
                    let name = entry.file_name().into_string().map_err(|_| {
                        WorkspaceError::new(ErrorCode::Unsupported, "non-UTF-8 filename")
                    })?;
                    let kind = entry.file_type()?;
                    entries.push(json!({"name":name,"kind":if kind.is_symlink() {"symlink"} else if kind.is_dir() {"directory"} else {"file"}}));
                }
                entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                Ok(json!({"entries":entries}))
            }
            Operation::Mkdir { path } => {
                fs::create_dir_all(self.resolve(&path)?)?;
                Ok(json!({}))
            }
            Operation::Remove { path, recursive } => {
                let path = self.resolve(&path)?;
                if path == self.root {
                    return Err(WorkspaceError::new(
                        ErrorCode::InvalidPath,
                        "cannot remove workspace root",
                    ));
                }
                if fs::metadata(&path)?.is_dir() {
                    if recursive {
                        fs::remove_dir_all(path)?;
                    } else {
                        fs::remove_dir(path)?;
                    }
                } else {
                    fs::remove_file(path)?;
                }
                Ok(json!({}))
            }
            Operation::Edit { path, replacements } => {
                let path = self.resolve(&path)?;
                let original = String::from_utf8(self.bytes(&path)?)
                    .map_err(|_| WorkspaceError::new(ErrorCode::InvalidInput, "not UTF-8"))?;
                let updated = replace_exact(&original, &replacements)?;
                self.atomic_write(&path, updated.as_bytes())?;
                Ok(json!({"replacements":replacements.len(),"bytes":updated.len()}))
            }
            Operation::Search {
                path,
                pattern,
                limit,
            } => {
                if !(1..=1000).contains(&limit) {
                    return Err(WorkspaceError::new(
                        ErrorCode::InvalidInput,
                        "search limit must be 1..1000",
                    ));
                }
                let regex = regex::Regex::new(&pattern)
                    .map_err(|e| WorkspaceError::new(ErrorCode::InvalidInput, e.to_string()))?;
                let mut matches = Vec::new();
                let mut truncated = false;
                let mut skipped = 0;
                for (visited, entry) in walkdir::WalkDir::new(self.resolve(&path)?)
                    .follow_links(false)
                    .sort_by_file_name()
                    .into_iter()
                    .enumerate()
                {
                    if visited == ENTRY_LIMIT {
                        truncated = true;
                        break;
                    }
                    let entry = entry.map_err(|e| {
                        WorkspaceError::new(
                            ErrorCode::Io,
                            e.io_error()
                                .map(|e| e.kind().to_string())
                                .unwrap_or_else(|| "walk failed".into()),
                        )
                    })?;
                    if !entry.file_type().is_file() {
                        continue;
                    }
                    let bytes = match self.bytes(entry.path()) {
                        Err(e) if e.code == ErrorCode::LimitExceeded => {
                            skipped += 1;
                            continue;
                        }
                        value => value?,
                    };
                    let Ok(text) = String::from_utf8(bytes) else {
                        skipped += 1;
                        continue;
                    };
                    for (line, content) in text.lines().enumerate() {
                        if regex.is_match(content) {
                            if matches.len() == limit {
                                truncated = true;
                                break;
                            }
                            let relative = entry
                                .path()
                                .strip_prefix(&self.root)
                                .unwrap()
                                .to_str()
                                .ok_or_else(|| {
                                    WorkspaceError::new(
                                        ErrorCode::Unsupported,
                                        "non-UTF-8 filename",
                                    )
                                })?;
                            matches.push(json!({"path":relative,"line":line+1,"text":content.chars().take(2048).collect::<String>(),"line_truncated":content.chars().count()>2048}));
                        }
                    }
                    if truncated {
                        break;
                    }
                }
                Ok(json!({"matches":matches,"truncated":truncated,"skipped_files":skipped}))
            }
            Operation::Exec { .. } => unreachable!(),
        }
    }
}

#[async_trait]
impl Provider for LocalProvider {
    fn info(&self, id: &str) -> WorkspaceInfo {
        WorkspaceInfo {
            id: id.into(),
            root: ".".into(),
            provider: "local".into(),
            platform: std::env::consts::OS.into(),
            capabilities: [
                "filesystem.read",
                "filesystem.write",
                "filesystem.atomicReplace",
                "filesystem.stat",
                "filesystem.list",
                "filesystem.mkdir",
                "filesystem.remove",
                "filesystem.exactEdit",
                "search.regex",
                "process.exec",
                "process.timeout",
                "process.cancel",
                "platform.posix",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }
    }
    async fn execute(&self, operation: Operation) -> Result<Value> {
        match operation {
            Operation::Exec {
                program,
                args,
                cwd,
                env,
                timeout_ms,
            } => crate::process::execute(program, args, self.resolve(&cwd)?, env, timeout_ms).await,
            other => self.filesystem(other),
        }
    }
}

pub fn replace_exact(original: &str, edits: &[Replacement]) -> Result<String> {
    if edits.is_empty() {
        return Err(WorkspaceError::new(
            ErrorCode::InvalidInput,
            "empty replacements",
        ));
    }
    let mut ranges = Vec::new();
    for edit in edits {
        if edit.old_text.is_empty() {
            return Err(WorkspaceError::new(
                ErrorCode::InvalidInput,
                "empty old_text",
            ));
        }
        // Check every character boundary to detect overlapping matches too (aaa / aa).
        let mut positions = original
            .char_indices()
            .filter_map(|(i, _)| original[i..].starts_with(&edit.old_text).then_some(i));
        let start = positions
            .next()
            .ok_or_else(|| WorkspaceError::new(ErrorCode::Conflict, "old_text not found"))?;
        if positions.next().is_some() {
            return Err(WorkspaceError::new(
                ErrorCode::AmbiguousEdit,
                "old_text must match exactly once",
            ));
        }
        ranges.push((start, start + edit.old_text.len(), &edit.new_text));
    }
    ranges.sort_by_key(|r| r.0);
    if ranges.windows(2).any(|r| r[0].1 > r[1].0) {
        return Err(WorkspaceError::new(
            ErrorCode::Conflict,
            "overlapping edits",
        ));
    }
    let mut output = original.to_string();
    for (start, end, new) in ranges.into_iter().rev() {
        output.replace_range(start..end, new);
    }
    Ok(output)
}
