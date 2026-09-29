# Portspace

[![CI](https://github.com/pxwg/portspace/actions/workflows/ci.yml/badge.svg)](https://github.com/pxwg/portspace/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/pxwg/portspace)](https://github.com/pxwg/portspace/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A portable workspace interface for coding agents, implemented in Rust.

```text
MCP client → stdio (optionally over SSH) → MCP adapter
                                          ↓
                                 typed Workspace API
                                          ↓
                                  local POSIX provider
```

The workspace, not MCP or SSH, is the core abstraction. File operations and processes
run in the same environment. Each request explicitly selects an authorized workspace.
The `Provider` trait and `Runtime` can also be used directly as a Rust library.

## Install

Download versioned binaries from [GitHub Releases](https://github.com/pxwg/portspace/releases). Verify the downloaded archive against the release SHA256SUMS before installation.

## Build and run

Requires Rust 1.88+ and a POSIX host (tested on macOS arm64). Python 3 is needed only
for black-box tests. Dependencies are pinned in `Cargo.lock`.

```sh
cargo build --locked --release
./target/release/portspace --workspace main=/path/to/project
# Multiple independent workspaces:
./target/release/portspace --workspace frontend=/path/to/frontend --workspace backend=/path/to/backend
```

Roots must already exist. No default workspace or implicit home-directory access is
configured. The server speaks newline-delimited MCP JSON-RPC on stdin/stdout; diagnostic
output goes to stderr. MCP handshake, schemas and cancellation use the official `rmcp` SDK.

All paths and host names below are placeholders; replace them with your own absolute paths and SSH host alias.

Example MCP client configuration:

```json
{
  "mcpServers": {
    "portspace": {
      "command": "/path/to/portspace",
      "args": ["--workspace", "main=/path/to/project"]
    }
  }
}
```

### Remote workspace over SSH

Install the binary on the remote host, then use existing OpenSSH authentication and host verification:

```json
{
  "mcpServers": {
    "portspace-remote": {
      "command": "ssh",
      "args": ["-T", "-o", "BatchMode=yes", "your-host",
        "/path/to/portspace",
        "--workspace", "main=/path/to/project"]
    }
  }
}
```

OpenSSH forwards stdio; the provider is **local on the remote host**, not local on the client.
Do not add `-t`, disable host-key checking, or print shell greetings to stdout. SSH
remote commands are shell-parsed: quote remote arguments if their paths contain spaces
or shell metacharacters. No SSH daemon or credentials are managed by Portspace.

## Tools

- `workspace_list`: workspace IDs, logical root, platform and capabilities.
- `workspace_execute`: `{ "workspace": "main", "operation": { ... } }`.

Supported operations (all listed fields are required):

| `op` | Fields |
| --- | --- |
| `read` | `path`, `encoding` (`utf8` / `base64`) |
| `write` | `path`, `content`, `encoding` |
| `stat`, `list`, `mkdir` | `path` |
| `remove` | `path`, `recursive` |
| `edit` | `path`, `replacements: [{old_text, new_text}]` |
| `search` | `path`, `pattern` (regex), `limit` (1–1000) |
| `exec` | `program`, `args`, `cwd`, `env`, `timeout_ms` (1–300000) |

Example arguments for `workspace_execute`:

```json
{
  "workspace": "main",
  "operation": {
    "op": "exec", "program": "/bin/sh", "args": ["-c", "cargo test"],
    "cwd": ".", "env": {}, "timeout_ms": 120000
  }
}
```

Tool results include both `structuredContent` and JSON text. Workspace errors have
`isError: true` and `{ "error": { "code": "not_found", "message": "..." } }`.
Nonzero process exit is a successful observation with `exit_code`, not a protocol error.

## Contract and limits

- Relative paths only, no `..` or symlink components. Search does not follow symlinks.
- Atomic file replacement; multi-edits validate uniqueness and overlap before writing.
  Existing file permissions and newline bytes are preserved; new files use private
  temporary-file permissions. Parent directories must exist for writes.
- Operations serialize per workspace ID. Do not configure overlapping roots if you need
  cross-request mutation serialization. External tools are outside the lock.
- Files: 4 MiB. Process capture: first 1 MiB of bytes per stream, UTF-8 lossy decoding,
  continued draining to avoid pipe deadlock. Search: at most 10,000 visited entries,
  2,048 characters per result line. Limits and skipped files are explicit in results.
- Deadline or cancellation terminates the process group. No detached background jobs.
- Closing a session preserves externally managed workspace files.
- Current scope: POSIX, stdio, local execution provider, SSH as transport. No interactive
  stdin, output streaming, glob, Windows, HTTP, native Pi/Codex/Claude tool emulation,
  or separate remote Workspace RPC protocol. Unsupported capabilities are not advertised.

**Trust boundary:** this is a trusted-user execution capability, not a sandbox. Commands
can access anything the server user can access, including credentials and paths outside
the root. Filesystem path checks do not protect against malicious concurrent renames.
Client-side permission approval remains the harness's responsibility. Never expose this
server to untrusted clients or an unauthenticated network.

## Test

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 tests/e2e.py --binary target/debug/portspace
python3 tests/e2e.py --ssh your-host \
  --binary /path/to/portspace
```

The same black-box assertions exercise local and SSH transports: initialization, tool
schemas, discovery, filesystem/process coherence, isolation, binary encoding, atomic
edit failures, search, structured errors, output bounds, timeout, process cancellation,
and persistent workspace lifecycle. Remote test data is created under a unique temporary
directory and removed afterward. No existing project files are used.

## License

[MIT](LICENSE). Contributions are welcome through issues and pull requests.
