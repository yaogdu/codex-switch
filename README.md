# Codex Switch

![macOS](https://img.shields.io/badge/macOS-desktop-111827)
![Version 0.1.4](https://img.shields.io/badge/Version-0.1.4-111827)
![Tauri 2](https://img.shields.io/badge/Tauri-2.x-24C8DB)
![React](https://img.shields.io/badge/React-18-61DAFB)
![Rust](https://img.shields.io/badge/Rust-1.85%2B-b45309)
![License MIT](https://img.shields.io/badge/License-MIT-0f766e)

Route local Codex sessions to different upstream profiles from a small macOS desktop app.

Codex Switch runs a local proxy, scans local Codex session metadata, and lets you bind each session to a different upstream profile. It is designed for people who switch between multiple Codex-compatible providers or API keys without editing `~/.codex/config.toml` by hand.

The app manages routing metadata only. It does not need to read conversation content to decide which profile a session should use.

## Start Here

| Need | Go to |
| --- | --- |
| Run the app locally | [Quick start](#quick-start) |
| Build a macOS release package | [Build and package](#build-and-package) |
| Understand Codex takeover | [Codex config takeover](#codex-config-takeover) |
| Check generated artifacts | [Release artifacts](#release-artifacts) |
| Review safety boundaries | [Safety model](#safety-model) |

## At a glance

| Question | Answer |
| --- | --- |
| What is stable? | Profile management, default profile selection, session binding, local proxy routing, menu bar controls, dated Codex config backups, and cancel takeover. |
| What is local? | App config is stored in macOS Application Support. Codex config backups stay under `~/.codex`. |
| What is not stored? | Codex conversation content is not copied into the app config. |
| What is not included yet? | Cloud sync, team sharing, auto-update, notarized release signing, and Windows/Linux desktop packaging. |

## When to use it

Use Codex Switch when you have more than one Codex-compatible upstream and want routing control without constantly changing Codex config files.

Typical cases:

- Keep a personal default provider while routing selected sessions elsewhere.
- Test a new gateway or provider on one session before making it the default.
- Switch API keys or base URLs from a desktop UI.
- Keep a visible menu bar control for starting, stopping, taking over, and canceling takeover.

## What it does

- Runs a local Codex-compatible HTTP proxy.
- Manages upstream profiles with base URL and optional API key.
- Sets one global default profile.
- Scans local Codex session metadata with pagination, search, and sorting.
- Binds individual sessions to a fixed profile.
- Updates route/profile changes immediately in the UI and proxy state.
- Provides menu bar controls with visible app notifications.
- Supports light, dark, and system appearance modes.
- Packages macOS `.app`, `.dmg`, `.zip`, and SHA-256 checksums.

## Codex config takeover

Codex Switch has two Codex config actions:

| Action | Meaning |
| --- | --- |
| Take over Codex | Back up `~/.codex/config.toml`, then point Codex's OpenAI provider base URL at the local Codex Switch proxy. Codex traffic can then be routed by session. |
| Cancel takeover | Restore Codex from the latest Codex Switch backup and remove the takeover marker. Codex stops using the local proxy and returns to the previous provider config. |

Backups are named:

```text
~/.codex/config.toml.codex-switch-backup-YYYYMMDD-HHMMSS
```

If multiple backups are created in the same second, Codex Switch appends a suffix such as `-2` to avoid overwriting an existing backup.

## Quick start

Requirements:

- macOS
- Node.js + npm
- Rust stable 1.85 or newer
- Xcode command line tools

Install dependencies and start the desktop app:

```bash
npm install
npm run dev
```

## Build and package

Build the release artifacts:

```bash
npm install
npm run package:mac
```

The package script runs the frontend typecheck, Rust tests, Tauri release build, app signature verification, DMG verification, zip packaging, and checksum generation.

## Versioning

The first public source release is `0.1.0`.

Keep these files on the same version before packaging:

- `package.json`
- `Cargo.toml`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`

`npm run package:mac` checks the four version values and fails if they drift.

## Release artifacts

Artifacts are written to `release/`:

| Artifact | Purpose |
| --- | --- |
| `codex-switch-<version>-macos-<arch>.dmg` | macOS installer image when DMG bundling succeeds. |
| `codex-switch-<version>-macos-<arch>.zip` | Zipped `.app` bundle fallback and GitHub release attachment. |
| `SHA256SUMS.txt` | SHA-256 checksums for release files. |

Current release builds use ad-hoc signing. They are suitable for local testing and source releases, but not notarized public binary distribution.

## Project layout

| Path | Description |
| --- | --- |
| `frontend/` | React UI and Vite config. |
| `src-tauri/` | Tauri desktop shell, tray menu, config takeover, session scanner, and app commands. |
| `src/` | Local proxy proof-of-concept library and tests. |
| `assets/icon.svg` | Source app icon. Regenerate Tauri icons only after changing it. |
| `scripts/package-mac.sh` | macOS release packaging script. |
| `docs/` | Planning and implementation notes. |

## Safety model

- Before takeover, the current Codex config is copied to a dated backup.
- Cancel takeover only restores from a Codex Switch backup when the current Codex config still points at the local proxy.
- If the user has already changed Codex config away from the local proxy, cancel takeover only clears Codex Switch's marker and does not overwrite the user's current config.
- On app startup, stale takeover markers are cleaned up.
- On app exit, Codex Switch attempts to cancel takeover so Codex is not left pointing at a dead local proxy.

## Development checks

Run the core checks:

```bash
npm run typecheck
npm run build:renderer
cargo test --manifest-path Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
```

Format Rust code:

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

## Project policy

- Repository: [github.com/yaogdu/codex-switch](https://github.com/yaogdu/codex-switch)
- License: [MIT](LICENSE)
- Platform target: macOS desktop first
- Release script: [scripts/package-mac.sh](scripts/package-mac.sh)
