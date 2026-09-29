# Changelog

All notable changes are documented here. Versions follow Semantic Versioning.

## 0.1.0 — 2026-09-29

Initial public release under the MIT license.

- Typed Rust Workspace API, provider interface, and explicit multi-workspace runtime.
- Local POSIX filesystem, atomic writes and exact multi-edits, binary-safe encoding,
  bounded regex search, and coherent process execution.
- Bounded stdout/stderr, deadlines, cancellation and disconnect process-group cleanup.
- MCP stdio adapter using the official Rust SDK; SSH transports requests without
  translating filesystem operations into shell snippets.
- Functional and black-box tests supporting local and SSH transports.
- Native Linux/macOS ARM64 and x86_64 CI and versioned binary releases with SHA-256 sums.

Known limitations: trusted-user capability rather than sandbox; POSIX only; no
interactive stdin, process output streaming, HTTP transport or native harness adapters.
