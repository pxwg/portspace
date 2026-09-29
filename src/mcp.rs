//! MCP is an adapter; providers do not depend on MCP request types.
use crate::{ErrorCode, Request, Runtime, WorkspaceError};
use rmcp::{
    RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Implementation, ServerCapabilities, ServerConfig},
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
}
#[tool_router]
impl McpAdapter {
    pub fn new(runtime: Runtime) -> Self {
        Self {
            runtime: Arc::new(runtime),
            tool_router: Self::tool_router(),
            disconnected: tokio_util::sync::CancellationToken::new(),
        }
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
#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpAdapter {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
    }
}
