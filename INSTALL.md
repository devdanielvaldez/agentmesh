# Install AgentMesh

Prebuilt binaries ship with every GitHub release for macOS (ARM64, Intel),
Linux (x86_64), and Windows (x86_64). Each archive carries a `.sha256`
checksum file next to it.

## macOS and Linux

```bash
curl -fsSL https://raw.githubusercontent.com/devdanielvaldez/agentmesh/main/install.sh | sh
```

Options: `--version vX.Y.Z` (default: latest) and `--to DIR`
(default: `~/.local/bin`, created when missing, no sudo needed):

```bash
curl -fsSL .../install.sh | sh -s -- --version v0.1.0 --to /usr/local/bin
```

The script verifies the SHA-256 checksum before installing and prints a PATH
hint when the destination is not on `PATH`. Linux ARM64 has no prebuilt
binary; use the Rust route below.

## Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/devdanielvaldez/agentmesh/main/install.ps1 | iex
```

With options:

```powershell
& .\install.ps1 -Version v0.1.0 -InstallDir C:\tools\agentmesh
```

Only 64-bit x86 Windows has prebuilt binaries. The script verifies the
checksum, extracts to `%USERPROFILE%\.agentmesh\bin` by default, and adds it
to your user `PATH` (restart the terminal afterwards).

## Manual download

Pick the archive for your platform from the
[releases page](https://github.com/devdanielvaldez/agentmesh/releases):

| Platform         | Archive                                   |
| ---------------- | ----------------------------------------- |
| macOS ARM64      | `agentmesh-vX.Y.Z-aarch64-apple-darwin.tar.gz` |
| macOS Intel      | `agentmesh-vX.Y.Z-x86_64-apple-darwin.tar.gz`  |
| Linux x86_64     | `agentmesh-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` |
| Windows x86_64   | `agentmesh-vX.Y.Z-x86_64-pc-windows-msvc.zip`  |

Verify against the adjacent `.sha256` file, extract, and place `agentmesh`
(or `agentmesh.exe`) on `PATH`. Each archive also bundles `README.md`,
`LICENSE`, and `config/agentmesh.example.yaml`.

## From source with Rust

Requires Rust 1.85+ ([rustup](https://rustup.rs/)):

```bash
cargo install --git https://github.com/devdanielvaldez/agentmesh --locked
```

## Verify and uninstall

```bash
agentmesh --version
agentmesh doctor --config config/agentmesh.local.yaml
```

Uninstall: delete the installed `agentmesh` binary (and, on Windows, remove
its directory from your user `PATH`).
