# Changelog

All notable changes are documented here. Versions follow Semantic Versioning.

## Unreleased

- Server-side MCP tool profiles with explicit workspace binding; generic Workspace
  tools remain the default, without a harness-specific runtime plugin.
- Best-effort Pi profile: default read/write/edit/bash, including bounded image
  reads and controlled Unicode/whitespace fuzzy editing. Bash deliberately uses
  fresh-workspace, non-interactive, non-streaming execution with a 300-second cap.
- Optional provider-backed grep/find/ls, disabled unless selected with `--pi-tools`.
- Compound workspace locking, native Pi differential fixtures, and MCP image/process/
  search tests. Codex/Claude Code profiles remain unimplemented; supported behavior
  is best-effort rather than full native parity.

## 0.1.0 — 2026-09-29

Initial public release under the MIT license.

- Typed Rust Workspace API, provider interface, and explicit multi-workspace runtime.
- Local POSIX filesystem, atomic writes and exact multi-edits, binary-safe encoding,
  bounded regex search, and coherent process execution.
- Bounded stdout/stderr, deadlines, cancellation and disconnect process-group cleanup.
- MCP stdio adapter using the official Rust SDK; SSH transports requests without
  translating filesystem operations into shell snippets.
- Functional and black-box tests supporting both local and SSH transports.
- Native Linux/macOS ARM64 and x86_64 CI and versioned binary releases with SHA-256 sums.

Known limitations: trusted-user capability rather than sandbox; POSIX only; no
interactive stdin, process output streaming, HTTP transport or native harness adapters.
