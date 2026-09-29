//! Bounded POSIX execution. Dropping the future terminates its process group.
use crate::*;
use serde_json::json;
use std::{os::unix::process::CommandExt, path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

const OUTPUT_LIMIT: usize = 1024 * 1024;
struct ProcessGroup(u32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // SAFETY: kill is called with a valid negative process-group ID created by us.
        // ESRCH is harmless when the process group has already exited.
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}
async fn capture(mut reader: impl AsyncRead + Unpin) -> std::io::Result<(String, bool)> {
    let mut output = Vec::new();
    let mut chunk = [0; 8192];
    let mut truncated = false;
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        let keep = n.min(OUTPUT_LIMIT - output.len());
        output.extend_from_slice(&chunk[..keep]);
        truncated |= keep < n;
    }
    Ok((String::from_utf8_lossy(&output).into_owned(), truncated))
}
pub async fn execute(
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    timeout_ms: u64,
) -> Result<Value> {
    if !(1..=300_000).contains(&timeout_ms) || program.is_empty() {
        return Err(WorkspaceError::new(
            ErrorCode::InvalidInput,
            "program required; timeout_ms must be 1..300000",
        ));
    }
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command.as_std_mut().process_group(0);
    let mut child = command.spawn()?;
    let _group = ProcessGroup(child.id().expect("spawned process has ID"));
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let execution = async { tokio::try_join!(child.wait(), capture(stdout), capture(stderr)) };
    let (status, (stdout, out_truncated), (stderr, err_truncated)) =
        tokio::time::timeout(Duration::from_millis(timeout_ms), execution)
            .await
            .map_err(|_| WorkspaceError::new(ErrorCode::Timeout, "process deadline exceeded"))??;
    use std::os::unix::process::ExitStatusExt;
    Ok(
        json!({"exit_code":status.code(),"signal":status.signal(),"stdout":stdout,"stderr":stderr,
        "stdout_truncated":out_truncated,"stderr_truncated":err_truncated}),
    )
}
