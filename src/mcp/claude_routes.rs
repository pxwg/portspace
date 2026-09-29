use super::*;
use crate::profiles::claude;

impl McpAdapter {
    pub(super) fn claude_profile_router() -> ToolRouter<Self> {
        Self::claude_router()
    }
}

#[tool_router(router = claude_router)]
impl McpAdapter {
    #[tool(
        name = "Read",
        description = "Read a bound-workspace file, using file_path relative or /workspace/... (not host absolute paths). Text has 1-based cat-n line numbers; offset/limit positive; default/max 2000 lines, 2000 chars per line, 50 KiB total, file <=4 MiB. Supports images with Workspace resize limits. PDF/notebook rendering unsupported. Records a session snapshot for Edit/Write, including paginated reads; only 16 recent snapshots retained."
    )]
    async fn claude_read(
        &self,
        Parameters(input): Parameters<claude::ReadInput>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.profile_result(
            context,
            claude::read(
                &self.runtime,
                self.workspace.as_deref().unwrap(),
                &self.claude_state,
                input,
            ),
        )
        .await
    }
    #[tool(
        name = "Write",
        description = "Create or overwrite a UTF-8 workspace file, creating parents. Existing files require Read first and unchanged content. Session writes refresh snapshots. Maximum 4 MiB; paths relative or /workspace/...; no symlinks. Not a sandbox or an external-writer transaction."
    )]
    async fn claude_write(
        &self,
        Parameters(input): Parameters<claude::WriteInput>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.profile_result(
            context,
            claude::write(
                &self.runtime,
                self.workspace.as_deref().unwrap(),
                &self.claude_state,
                input,
            ),
        )
        .await
    }
    #[tool(
        name = "Edit",
        description = "Exact string replacement in a previously Read, unchanged workspace file. old_string must be nonempty, differ from new_string and occur uniquely unless replace_all=true. No fuzzy matching. Preserves untouched bytes; <=4 MiB. Paths relative or /workspace/...; no symlinks. Read snapshots are session-local and bounded to 16 files."
    )]
    async fn claude_edit(
        &self,
        Parameters(input): Parameters<claude::EditInput>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.profile_result(
            context,
            claude::edit(
                &self.runtime,
                self.workspace.as_deref().unwrap(),
                &self.claude_state,
                input,
            ),
        )
        .await
    }
    #[tool(
        name = "Glob",
        description = "Find workspace files by case-sensitive glob. Optional path defaults to workspace root. Returns /workspace/... paths in lexical order, NOT native modification-time order. Honors local .gitignore/.ignore; skips .git and symlinks. Up to 10000 visited entries, 1000 results, 50 KiB. No host paths or home expansion."
    )]
    async fn claude_glob(
        &self,
        Parameters(input): Parameters<claude::GlobInput>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.profile_result(
            context,
            claude::glob(&self.runtime, self.workspace.as_deref().unwrap(), input),
        )
        .await
    }
    #[tool(
        name = "Grep",
        description = "Search workspace files using Rust regex (not PCRE/ripgrep). output_mode: files_with_matches (default), content, count (matching lines). Supports glob, -i, -n (default true), -A/-B/-C context, head_limit (default 200, 0 means bounded maximum 10000), offset over rendered rows. Context windows merge. multiline=true and type filters unsupported. Local ignore rules; no symlinks. 64 MiB scan, 50 KiB output, 2000 chars/line; truncation explicit. Paths relative or /workspace/..., output paths virtual absolute."
    )]
    async fn claude_grep(
        &self,
        Parameters(input): Parameters<claude::GrepInput>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.profile_result(
            context,
            claude::grep(&self.runtime, self.workspace.as_deref().unwrap(), input),
        )
        .await
    }
    #[tool(
        name = "Bash",
        description = "Execute foreground Workspace bash. timeout in MILLISECONDS, default 120000, range 1..300000. description is metadata only. run_in_background=true and dangerouslyDisableSandbox=true are rejected. Fresh workspace-root cwd every call, no persistent cwd/stdin/background tasks, no login scripts. Inherits execution-server environment. Output bounded, nonzero exit is error; timeout/cancel/disconnect kills process group. /workspace is a virtual FILE-tool root, not a shell directory. Full server-user authority, NOT sandboxed; no native Claude permission policy."
    )]
    async fn claude_bash(
        &self,
        Parameters(input): Parameters<claude::BashInput>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.profile_result(
            context,
            claude::bash(&self.runtime, self.workspace.as_deref().unwrap(), input),
        )
        .await
    }
}
