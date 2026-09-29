use portspace::{Runtime, local::LocalProvider, mcp::McpAdapter, profiles::ToolProfile};
use rmcp::ServiceExt;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("portspace: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut runtime = Runtime::default();
    let mut profile = ToolProfile::default();
    let mut tool_workspace = None;
    let mut optional_tools = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "Usage: portspace --workspace ID=ROOT [--workspace ID=ROOT ...]\n                 [--tool-profile workspace|pi|claude-code] [--tool-workspace ID] [--pi-tools grep,find,ls]\n\nTrusted-user POSIX workspace MCP server over stdio. Roots must exist.\nThe default workspace profile exposes workspace_list/workspace_execute.\nHarness profiles require --tool-workspace.\nPi defaults to read/write/edit/bash; Claude Code exposes Read/Write/Edit/Glob/Grep/Bash.\nOptional grep/find/ls must be enabled explicitly with --pi-tools.\nUse ssh -T HOST /absolute/path/portspace --workspace ID=ROOT for remote workspaces."
                );
                return Ok(());
            }
            "--version" => {
                println!("portspace {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--tool-profile" => {
                profile = args
                    .next()
                    .ok_or("--tool-profile requires a name")?
                    .parse()?;
            }
            "--pi-tools" => {
                optional_tools.extend(
                    args.next()
                        .ok_or("--pi-tools requires grep,find,ls or a subset")?
                        .split(',')
                        .map(str::to_owned),
                );
            }
            "--tool-workspace" => {
                tool_workspace = Some(args.next().ok_or("--tool-workspace requires an ID")?);
            }
            "--workspace" => {
                let value = args.next().ok_or("--workspace requires ID=ROOT")?;
                let (id, root) = value
                    .split_once('=')
                    .ok_or("--workspace requires ID=ROOT")?;
                runtime.register(id.into(), Arc::new(LocalProvider::new(root)?))?;
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    if runtime.workspaces().is_empty() {
        return Err("configure at least one --workspace ID=ROOT".into());
    }
    let adapter =
        McpAdapter::with_profile_tools(runtime, profile, tool_workspace, &optional_tools)?;
    let input =
        portspace::transport::DisconnectReader::new(tokio::io::stdin(), adapter.disconnect_token());
    let service = adapter.serve((input, tokio::io::stdout())).await?;
    service.waiting().await?;
    Ok(())
}
