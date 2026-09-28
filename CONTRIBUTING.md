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
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo run -p agentmesh -- validate config/agentmesh.yaml
```

The same checks run in CI. Public APIs should include rustdoc, behavioral changes should include
tests, and architecture changes should add or update an ADR under `docs/adr`.

## Commit and pull request guidance

Use an imperative subject and explain why the change is necessary. Conventional Commit prefixes
such as `feat:`, `fix:`, `docs:`, and `chore:` are encouraged. Pull requests must describe testing,
security impact, and any compatibility implications.

By contributing, you agree that your work is licensed under the repository's MIT License and that
you will follow the [Code of Conduct](CODE_OF_CONDUCT.md).

