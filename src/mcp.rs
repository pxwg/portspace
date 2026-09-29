//! MCP is an adapter; providers do not depend on MCP request types.
use crate::{
    ErrorCode, Request, Runtime, WorkspaceError,
    profiles::{ToolProfile, pi, search, shell},
};
use rmcp::{
    RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
pub struct McpAdapter {
    runtime: Arc<Runtime>,
    tool_router: ToolRouter<Self>,
    disconnected: tokio_util::sync::CancellationToken,
    workspace: Option<String>,
}
#[tool_router]
impl McpAdapter {
    pub fn new(runtime: Runtime) -> Self {
        Self {
            runtime: Arc::new(runtime),
            tool_router: Self::tool_router(),
            disconnected: tokio_util::sync::CancellationToken::new(),
            workspace: None,
        }
    }

    pub fn with_profile(
        runtime: Runtime,
        profile: ToolProfile,
        workspace: Option<String>,
    ) -> crate::Result<Self> {
        Self::with_profile_tools(runtime, profile, workspace, &[])
    }

    pub fn with_profile_tools(
        runtime: Runtime,
        profile: ToolProfile,
        workspace: Option<String>,
        optional_tools: &[String],
    ) -> crate::Result<Self> {
        if optional_tools
            .iter()
            .any(|t| !["grep", "find", "ls"].contains(&t.as_str()))
        {
            return Err(WorkspaceError::new(
                ErrorCode::InvalidInput,
                "--pi-tools accepts only grep,find,ls",
            ));
        }
        if profile == ToolProfile::Workspace && !optional_tools.is_empty() {
            return Err(WorkspaceError::new(
                ErrorCode::InvalidInput,
                "--pi-tools requires --tool-profile pi",
            ));
        }
        if profile == ToolProfile::Workspace {
            if workspace.is_some() {
                return Err(WorkspaceError::new(
                    ErrorCode::InvalidInput,
                    "--tool-workspace requires a harness profile",
                ));
            }
            return Ok(Self::new(runtime));
        }
        let id = workspace.as_deref().ok_or_else(|| {
            WorkspaceError::new(
                ErrorCode::InvalidInput,
                "harness profiles require --tool-workspace ID",
            )
        })?;
        let info = runtime
            .workspaces()
            .into_iter()
            .find(|w| w.id == id)
            .ok_or_else(|| {
                WorkspaceError::new(ErrorCode::WorkspaceUnavailable, "unknown tool workspace")
            })?;
        let mut required = vec![
            "filesystem.read",
            "filesystem.write",
            "filesystem.mkdir",
            "process.exec",
            "process.timeout",
            "process.cancel",
        ];
        if !optional_tools.is_empty() {
            required.extend(["filesystem.list", "filesystem.stat"]);
        }
        for capability in required {
            if !info.capabilities.iter().any(|c| c == capability) {
                return Err(WorkspaceError::new(
                    ErrorCode::Unsupported,
                    format!("profile requires {capability}"),
                ));
            }
        }
        let mut tool_router = Self::pi_router();
        for name in ["grep", "find", "ls"] {
            if !optional_tools.iter().any(|t| t == name) {
                tool_router.remove_route(name);
            }
        }
        Ok(Self {
            runtime: Arc::new(runtime),
            tool_router,
            disconnected: tokio_util::sync::CancellationToken::new(),
            workspace,
        })
    }

    pub fn disconnect_token(&self) -> tokio_util::sync::CancellationToken {
        self.disconnected.clone()
    }

    #[tool(
        description = "Discover authorized workspace IDs, platform and supported capabilities. Paths are workspace-relative. Closing the session never deletes workspaces."
    )]
    async fn workspace_list(&self) -> CallToolResult {
        CallToolResult::structured(json!({"workspaces":self.runtime.workspaces()}))
    }

    #[tool(
        description = "Execute a typed operation against an explicit workspace. Paths must be relative and symlinks are rejected. Exec uses program+argv, not implicit shell syntax, and grants full user authority. File limit 4 MiB; stdout/stderr limit 1 MiB each; timeout 1..300000 ms. Exact edits match uniquely against the original, without overlaps. Search limit 1..1000."
    )]
    async fn workspace_execute(
        &self,
        Parameters(request): Parameters<Request>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let result = tokio::select! {
            biased;
            _ = context.ct.cancelled() => Err(WorkspaceError::new(ErrorCode::Cancelled,"request cancelled")),
            _ = self.disconnected.cancelled() => Err(WorkspaceError::new(ErrorCode::Cancelled,"client disconnected")),
            result = self.runtime.execute(request) => result,
        };
        match result {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => CallToolResult::structured_error(json!({"error":error})),
        }
    }
}
#[tool_router(router = pi_router)]
impl McpAdapter {
    async fn profile_result<T: Into<pi::Output>>(
        &self,
        context: RequestContext<RoleServer>,
        future: impl std::future::Future<Output = crate::Result<T>>,
    ) -> CallToolResult {
        let result = tokio::select! {
            biased;
            _ = context.ct.cancelled() => Err(WorkspaceError::new(ErrorCode::Cancelled, "request cancelled")),
            _ = self.disconnected.cancelled() => Err(WorkspaceError::new(ErrorCode::Cancelled, "client disconnected")),
            result = future => result,
        };
        match result {
            Ok(output) => {
                let output = output.into();
                let mut content = vec![ContentBlock::text(output.text)];
                if let Some(image) = output.image {
                    content.push(ContentBlock::image(image.data, image.mime));
                }
                CallToolResult::success(content)
            }
            Err(error) => {
                let mut result =
                    CallToolResult::error(vec![ContentBlock::text(error.message.clone())]);
                result.structured_content = Some(json!({"error": error}));
                result
            }
        }
    }

    #[tool(
        name = "read",
        description = "Read text or images from the bound workspace. Text: optional 1-indexed offset/limit, 2000 lines or 50 KiB output. Images: PNG/JPEG/GIF/WebP/BMP detected by content, returned as MCP images; resized to fit 2000x2000, GIF first frame only. Input <=4 MiB and images <=16 megapixels. Paths relative or /workspace/...; no home expansion or symlinks. Oversized single text lines fail explicitly."
    )]
    async fn pi_read(
        &self,
        Parameters(input): Parameters<pi::ReadInput>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.profile_result(
            context,
            pi::read(&self.runtime, self.workspace.as_deref().unwrap(), input),
        )
        .await
    }

    #[tool(
        name = "write",
        description = "Write content to a file in the bound workspace. Creates or overwrites files and automatically creates parent directories. Paths are relative or /workspace/...; no home expansion or symlinks. UTF-8 content, maximum 4 MiB. Returns Pi-style confirmation."
    )]
    async fn pi_write(
        &self,
        Parameters(input): Parameters<pi::WriteInput>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.profile_result(
            context,
            pi::write(&self.runtime, self.workspace.as_deref().unwrap(), input),
        )
        .await
    }

    #[tool(
        name = "edit",
        description = "Edit one workspace file with edits: [{oldText,newText}]. Match against the original, exact first then controlled NFKC/quotes/dashes/spaces/trailing-whitespace normalization. Matches must be unique and disjoint; validate all before writing. Preserve BOM, first newline style and unaffected lines. Paths relative or /workspace/...; <=4 MiB. No semantic guessing, legacy argument coercions or native UI diff."
    )]
    async fn pi_edit(
        &self,
        Parameters(input): Parameters<pi::EditInput>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.profile_result(
            context,
            pi::edit(&self.runtime, self.workspace.as_deref().unwrap(), input),
        )
        .await
    }

    #[tool(
        name = "bash",
        description = "Execute a non-interactive bash command in the bound workspace root on the execution host. Each call starts fresh; cwd/env changes do not persist. Use relative paths: /workspace is virtual for file tools, not a shell directory. timeout in seconds defaults to 300, maximum 300. No login/startup files, stdin, streaming or background sessions. Uses server environment, never a core-host env snapshot. Output bounded to 2000 lines/50 KiB; provider capture 1 MiB; truncation explicit, no full-output artifact. Nonzero exit is an error. Full server-user authority: not a sandbox."
    )]
    async fn pi_bash(
        &self,
        Parameters(input): Parameters<shell::BashInput>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.profile_result(
            context,
            shell::bash(&self.runtime, self.workspace.as_deref().unwrap(), input),
        )
        .await
    }

    #[tool(
        name = "ls",
        description = "List a workspace directory, including dotfiles, with / after directories. Optional path defaults to root; limit defaults to 500 (max 10000). Case-folded deterministic sort, 50 KiB output. Symlinks skipped explicitly. Workspace paths only; never core-host files."
    )]
    async fn pi_ls(
        &self,
        Parameters(input): Parameters<search::LsInput>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.profile_result(
            context,
            search::ls(&self.runtime, self.workspace.as_deref().unwrap(), input),
        )
        .await
    }

    #[tool(
        name = "find",
        description = "Find workspace entries using a case-sensitive glob. Optional path defaults to root; limit defaults to 1000 (max 10000). Basename patterns or relative path patterns with /. Returns relative paths with / for directories. Includes hidden entries; respects .gitignore/.ignore within the search root; excludes .git and symlinks. Deterministic ordering, 10000 visited entries / 50 KiB output cap, incomplete results explicit. No external fd required."
    )]
    async fn pi_find(
        &self,
        Parameters(input): Parameters<search::FindInput>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.profile_result(
            context,
            search::find(&self.runtime, self.workspace.as_deref().unwrap(), input),
        )
        .await
    }

    #[tool(
        name = "grep",
        description = "Search workspace file contents: pattern, optional path/glob/ignoreCase/literal/context/limit. Rust regex (no lookaround/backrefs), literal supported. Defaults: root, case-sensitive, context 0, limit 100 (max 10000), context max 1000. Returns relative file:line: text; context uses file-line-. Respects .gitignore/.ignore within search root; includes dotfiles, excludes .git/symlinks, skips binary or >4 MiB files explicitly. Caps: 10000 visited entries, 64 MiB scan, 50 KiB output, 500 Unicode characters per line. No external rg required."
    )]
    async fn pi_grep(
        &self,
        Parameters(input): Parameters<search::GrepInput>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.profile_result(
            context,
            search::grep(&self.runtime, self.workspace.as_deref().unwrap(), input),
        )
        .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpAdapter {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
    }
}
