# Contributing to AgentMesh

Thank you for helping build reliable infrastructure for MCP. Focused issues and pull requests are
welcome.

## Before you begin

- Search existing issues and discussions before opening a new proposal.
- Use a discussion for broad architectural ideas and an issue for actionable work.
- For security-sensitive findings, follow [SECURITY.md](SECURITY.md).
- Keep pull requests small enough to review and test independently.

## Development setup

Install Rust 1.85 or newer and Docker when container testing is needed. Then run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo run -p agentmesh -- validate config/agentmesh.yaml
```

The equivalent maintainer shortcut is `make ci`.

The same checks run in CI. Public APIs should include rustdoc, behavioral changes should include
tests, and architecture changes should add or update an ADR under `docs/adr`.

## Documentation

- Write user-facing documentation in English.
- Keep the README useful as a standalone product overview and quick start.
- Put detailed workflows in `docs/` and add them to `docs/README.md`.
- Prefer Mermaid for architecture and sequence diagrams so changes remain reviewable as text.
- Mark roadmap behavior clearly; do not present an extension contract as a shipped production
  integration.
- Ensure examples contain no real credentials, private hosts, or personal data.

## Commit and pull request guidance

Use an imperative subject and explain why the change is necessary. Conventional Commit prefixes
such as `feat:`, `fix:`, `docs:`, and `chore:` are encouraged. Pull requests must describe testing,
security impact, and any compatibility implications.

By contributing, you agree that your work is licensed under the repository's MIT License and that
you will follow the [Code of Conduct](CODE_OF_CONDUCT.md).
