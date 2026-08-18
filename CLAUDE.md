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
- Entity auto-merge is exact `(entity_type, canonical_key)` match only (case-
  insensitive, trimmed). Never auto-merge on fuzzy similarity — that's a
  manual `entity merge` action.
- CRUD/merge/split SQL lives in `eumeaus-engine/src/crud.rs` as free
  functions over `&Connection`/`&mut Connection`, not `Case` methods
  directly — `Case` (`case.rs`) just delegates. Keeps lifecycle and data
  logic separate, and lets `crud.rs`'s own tests run against a plain
  `Connection::open_in_memory()` with schema applied, no SQLCipher/keychain
  needed.
- A merge re-points the loser's facts/attributes/relationship endpoints at
  the survivor and deletes its entity row; a split does the reverse for a
  chosen set of facts. Either way the *facts' own data* (source, provenance,
  confidence) is never touched — only which entity currently owns them —
  and both are recorded as `audit_events` rows, never mutating a fact.
- Credentials are never passed via subprocess argv or env vars (visible to
  other processes) — injected into the gRPC request body by the plugin host.
- Plugin subprocess/gRPC wiring uses the tonic 0.12.x line (not latest —
  0.13+ split prost codegen into a separate `tonic-prost-build` crate with a
  less-established API; 0.12.x's classic `tonic_build::compile_protos` is
  better documented and lower-risk). `tower` is pinned to exactly what
  tonic 0.12 depends on (`0.4.7`) — bumping it alone would leave two
  incompatible `tower` majors in the graph.
- `eumeaus-plugin-host`'s public API (`load`/`invoke`/`shutdown`) is async,
  unlike SPEC.md §3.1's sync illustrative signature. `eumeaus-engine`'s
  `Case::start_scan`/`resume_scan` bridge it: they own a one-shot
  `tokio::runtime::Runtime` and `block_on` the whole scan to completion, so
  the public `Case` API stays sync throughout. Only the per-plugin
  `invoke()` calls run concurrently (`tokio::spawn`, bounded by
  `worker_pool`) — every DB write happens in the orchestrating task itself,
  since `rusqlite::Connection` is `!Sync` and must never cross a spawned
  task boundary. See `eumeaus-engine/src/scan.rs`'s module doc.
- A scan's `plugins_dir`/`TrustPolicy` aren't in SPEC.md §3.1's
  `resume_scan(scan_id)` signature (single arg), so they're persisted as
  JSON in `scans.config_snapshot` at `start_scan` time and read back on
  resume — including the trusted Ed25519 key as hex, if one was used. No
  CLI flag sets a trust key yet (that's credential/config management,
  M6+); `scan run` always loads plugins with `TrustPolicy::AllowUnsigned`
  until one exists.
- Test-fixture plugin binaries (`eumeaus-plugin-host/examples/stub_*.rs`,
  `eumeaus-engine/examples/scan_*.rs`) are Cargo *examples*, not `[[bin]]`
  targets — a same-package `[[bin]]` needing a dev-dependency (they depend
  on `eumeaus-plugin-sdk`) breaks plain `cargo build`, since dev-deps
  aren't linked for `[[bin]]`s outside `cargo test`. Examples are exempt
  from plain `cargo build` and still get dev-deps under `cargo test`, but
  have no `CARGO_BIN_EXE_`-style env var, so their tests locate the
  compiled binary relative to their own `current_exe()` instead. They're
  duplicated per-crate (plugin-host's `stub_*` vs. engine's `scan_*`)
  rather than shared, so `cargo test -p eumeaus-engine` alone still works
  — only `-p`'s own dev-deps get built, not another crate's.
- `.eum` case files are SQLCipher-encrypted SQLite; opening one takes an
  exclusive OS file lock (`Case::open`/`create`/`close` are implemented, M1).
  The key lives in the OS keychain under service `"eumeaus"`, entry =
  the case's UUID — never in the case file itself.
- A case's UUID has to be known *before* the DB can be decrypted (to look
  the key up), but it's also stored inside the encrypted DB — so a plaintext
  sidecar file `<name>.eum.meta` (just the UUID) sits next to each case file
  to break that chicken-and-egg. It carries no secret.

## Repo etiquette

- Commit format: Conventional Commits (`feat:`, `fix:`, `chore:`, `test:`, ...).
- Branch naming: `<type>/<short-description>` (e.g. `feat/case-lifecycle`).

## Current status

M0–M4 are done: case lifecycle over real SQLCipher (M1); entity/relationship
CRUD, merge/split, and audit trail via the CLI (M2); plugin manifest
discovery, semver/signature validation, and real subprocess+gRPC-over-UDS
spawn/handshake/invoke/timeout in `eumeaus-plugin-host` (M3); and scan
orchestration wired end to end through `scan run`/`status`/`resume` — worker
pool, rate limiting, crash-safe resumability, result ingestion through the
same auto-merge path as manual entry (M4). The credential store is still
NotImplemented — see SPEC.md §7 for the milestone order.

Deviations from SPEC.md's illustrative APIs, each with a reason documented
at the point of deviation: `Case::get_entity`/`list_attribute_records`/
`find_entity_by_key` (§3.1 gives no signatures, but `entity show`/`scan run
--target-type/--target-value` need them); `RelationshipType::Custom` plus a
`relationship_attributes` table (§4.2 only lists `entity_attributes`, but
`add_relationship` takes `attrs` too); `eumeaus-plugin-host`'s async API and
`Case::start_scan`'s `Vec<PluginRef>`/`plugins_dir`/`TrustPolicy` params
(see Conventions); and the plugin signature scheme (§3.3 doesn't specify
one — see `signature.rs`).

## Gotchas

- `cargo test` needs a running, *unlocked* OS Secret Service (gnome-keyring
  or equivalent) — any test that calls `Case::create`/`open` stores/reads a
  real key there. Works out of the box in a normal desktop session; CI
  starts one explicitly (see `.github/workflows/ci.yml`). In a headless/SSH
  shell with no keyring daemon, these tests will hang or fail on
  `EngineError::Keychain`.
- No system `protoc` is assumed to be installed; `eumeaus-plugin-protocol`'s
  build.rs points `PROTOC` at the `protoc-bin-vendored` crate's bundled
  binary instead. Don't add a "install protoc" CI step — it's unnecessary
  and would mask a build.rs regression if the vendoring ever broke.
- The starter entity/relationship taxonomy (SPEC.md §4.3) and several other
  points are flagged as **open questions** in SPEC.md §8 — check there before
  assuming a data-model detail is settled.
