use portspace::{Runtime, local::LocalProvider, mcp::McpAdapter};
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
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "Usage: portspace --workspace ID=ROOT [--workspace ID=ROOT ...]\n\nTrusted-user POSIX workspace MCP server over stdio. Roots must exist.\nUse ssh -T HOST /absolute/path/portspace --workspace ID=ROOT for remote workspaces."
                );
                return Ok(());
            }
            "--version" => {
                println!("portspace {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
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
    let service = McpAdapter::new(runtime)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
