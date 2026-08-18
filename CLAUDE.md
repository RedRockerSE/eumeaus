# Eumeaus

Local-first, plugin-extensible OSINT case management tool. No client-server
architecture, no GUI in v1 (CLI only). Full design: `SPEC.md`.

## Commands

```sh
cargo build --workspace                              # build everything
cargo run -p eumeaus-cli -- <args>                    # run the CLI (binary name: eumeaus)
cargo test --workspace                                # unit + e2e tests
cargo clippy --workspace --all-targets -- -D warnings # lint (must be warning-free)
cargo fmt --all                                       # format
```

CI (`.github/workflows/ci.yml`) runs `cargo fmt --check`, `cargo clippy -- -D
warnings`, and `cargo test --workspace` on every push/PR. All three must pass
before merge.

## Architecture

- `crates/eumeaus-engine` — the "brain": case lifecycle, entity/relationship/
  provenance data model, entity resolution, scan orchestration. Everything
  else is a client of it.
- `crates/eumeaus-plugin-host` — plugin discovery, manifest/signature
  verification, subprocess spawn + gRPC handshake, timeout/health monitoring.
- `crates/eumeaus-plugin-protocol` — the engine↔plugin wire contract
  (`plugin.proto`). No logic beyond generated types belongs here.
- `crates/eumeaus-plugin-sdk` — helper library for plugin authors (handshake
  + gRPC server boilerplate). Plugins are not required to use it or be Rust.
- `crates/eumeaus-cli` — thin CLI wrapper over the engine API; also the
  end-to-end test surface (`crates/eumeaus-cli/tests/`).

## Conventions

- Workspace deps/versions/lints are centralized in the root `Cargo.toml`
  (`[workspace.dependencies]`, `[workspace.package]`); crate `Cargo.toml`
  files reference them via `foo.workspace = true`, not their own versions.
- Facts (`facts` table) are append-only — never updated or deleted. Corrections
  are new fact rows; merges/splits get an explicit `audit_events` row.
- Entity auto-merge is exact `(entity_type, canonical_key)` match only. Never
  auto-merge on fuzzy similarity — that's a manual `entity merge` action.
- Credentials are never passed via subprocess argv or env vars (visible to
  other processes) — injected into the gRPC request body by the plugin host.
- `.eum` case files are SQLCipher-encrypted SQLite; opening one takes an
  exclusive OS file lock. A stub `Case::open`/`Case::create` currently returns
  `EngineError::NotImplemented` — this is expected until M1 lands.

## Repo etiquette

- Commit format: Conventional Commits (`feat:`, `fix:`, `chore:`, `test:`, ...).
- Branch naming: `<type>/<short-description>` (e.g. `feat/case-lifecycle`).

## Current status

Milestone M0 (workspace scaffolding) is done: workspace builds, `eumeaus-cli
--help` runs. Every engine/plugin-host method is a stub returning
`NotImplemented`. `crates/eumeaus-cli/tests/e2e_case_lifecycle.rs` is the
acceptance test for **M1** and is expected to fail until `Case::create`/
`Case::open` are implemented — see SPEC.md §7 for the milestone order.

## Gotchas

- No `protoc` is assumed to be installed; `eumeaus-plugin-protocol` ships a
  hand-written `stub` module mirroring `plugin.proto` until real
  tonic-build/prost-build codegen is wired up in M3.
- The starter entity/relationship taxonomy (SPEC.md §4.3) and several other
  points are flagged as **open questions** in SPEC.md §8 — check there before
  assuming a data-model detail is settled.
