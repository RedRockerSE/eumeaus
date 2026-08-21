# Eumeaus

Local-first, plugin-extensible OSINT case management tool. No client-server
architecture. CLI (v1) + Tauri GUI (v2). Full design: `SPEC.md`.

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
- `crates/eumeaus-username-search-plugin` — the real v1 PoC plugin (M5): a
  small Sherlock-equivalent, built on the SDK. A real shipped `[[bin]]`,
  not a test fixture — `cargo build` produces it normally.
- `crates/eumeaus-email-lookup-plugin` — a second real plugin, same
  shipped-`[[bin]]` treatment: checks Gravatar/Libravatar for a
  registered avatar (MD5-of-email lookup, no API key).
- `crates/eumeaus-ip-lookup-plugin` — a third real plugin: geolocates an
  `IPAddress` via ip-api.com, emitting `Location`+`Organization`.
- `crates/eumeaus-cli` — thin CLI wrapper over the engine API; also the
  end-to-end test surface (`crates/eumeaus-cli/tests/`), including the v1
  proof (`e2e_v1_proof.rs`, SPEC.md §6).
- `crates/eumeaus-gui` — Tauri 2.x + React/TS GUI (SPEC.md §9, v2). Frontend
  at the crate root; `src-tauri/` (the actual workspace member) is a thin
  `#[tauri::command]` wrapper over the engine API, like the CLI.

## Conventions

- Workspace deps/versions/lints are centralized in the root `Cargo.toml`
  (`[workspace.dependencies]`, `[workspace.package]`); crate `Cargo.toml`
  files reference them via `foo.workspace = true`, not their own versions.
- Facts (`facts` table) are append-only — never updated or silently
  deleted; corrections are new fact rows. The one sanctioned exception is
  `fact redact` (SPEC.md §8.4, `crud::redact_fact`): a real `DELETE`,
  always paired with a permanent `audit_events` row recording it happened.
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
- Touching the plugin protocol/host/SDK, writing a plugin (real or test
  fixture), or scan resumability internals: load the `plugin-development`
  skill first — subprocess/gRPC/async gotchas, signing, and the
  test-fixture-as-Cargo-example pattern all live there, not here.
- Credentials (`eumeaus-plugin-host/src/credentials.rs`) live in a
  *separate* OS-keychain service (`"eumeaus-credentials"`) from case
  encryption keys (`"eumeaus"`, `keystore.rs`), global to the OS user, not
  scoped to a case — never passed via subprocess argv/env (visible to
  other processes), only injected into `CheckRequest.resolved_credentials`
  synchronously before the scan's tokio runtime starts; a missing declared
  credential marks just that plugin's run `ERROR`, not the whole scan.
- `credential set`'s interactive prompt (`rpassword`) needs a real TTY —
  it refuses to read a plain pipe on purpose. Not reproducible in a
  headless/CI test without a pty-emulation dependency; the underlying
  store and injection are tested directly instead (`eumeaus-engine/src/scan.rs`).
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

SPEC.md §7's milestones (M0–M6) are done — v1 CLI complete, released,
public (`CLI.md` is the full reference); §8.1–8.7 resolved, §8.8 has a
design (§9). `v0.1.0`–`v0.1.2` tagged/published via `.github/workflows/
release.yml` (bundles all three plugins below); `install.sh`/`install.ps1` fetch/verify from there.

GUI (SPEC.md §9): Tauri 2.x, React+TS, Linux+Windows only, `crates/`
workspace. G0–G6 done; `gui-v0.1.0`/`gui-v0.1.1` tagged/published
(unsigned — Windows Authenticode deferred). Updater verified live
end-to-end into `gui-v0.1.1`. UX redesign (claude.ai/design handover,
merged) replaced G0–G6's flat forms with real sidebar screens
(Overview/Entities/Graph/Scans/Plugins/Settings), no mock data — see
`api.ts`/`entityStyle.ts`. Report export + verify (`report_state.rs`)
on Overview. `usertests/`'s exploratory pass added native pickers (`pickers.ts`), a persisted default plugins dir (`settings_state.rs`),
a searchable `EntityPicker` combobox, and entity-detail "Add fact"
(`Case::add_fact_to_entity`, correct for keyless entities unlike
re-calling `add_entity`) — also fixed `entityStyle.ts`'s `"IpAddress"`→`"IPAddress"` key mismatch.

Deviations from SPEC.md's illustrative APIs, each documented at the point
of deviation: `Case::get_entity`/`list_attribute_records`/`find_entity_by_key`/
`create_scan` (§3.1 gives no signatures); `RelationshipType::Custom` +
`relationship_attributes` table (§4.2 only lists `entity_attributes`);
`eumeaus-plugin-host`'s async API, `Case::start_scan`'s
`Vec<PluginRef>`/`plugins_dir`/`TrustPolicy` params (Conventions); the plugin signature scheme (§3.3 has none).

## Gotchas

- `cargo test` needs a running, *unlocked* OS Secret Service (gnome-keyring
  or equivalent) — any test that calls `Case::create`/`open` stores/reads a
  real key there. Works out of the box in a normal desktop session; CI
  starts one explicitly (`.github/workflows/ci.yml`). Headless/SSH with no
  keyring daemon: these tests hang or fail on `EngineError::Keychain`.
- No system `protoc` assumed installed; `eumeaus-plugin-protocol`'s build.rs
  points `PROTOC` at `protoc-bin-vendored`'s bundled binary instead — don't
  add an "install protoc" CI step, it'd mask a build.rs regression.
- `rusqlite` uses `bundled-sqlcipher-vendored-openssl`, not plain
  `bundled-sqlcipher` — the latter dynamically links the *build machine's*
  OpenSSL (confirmed via `ldd`), breaking release binaries on any other
  machine. Don't "simplify" this back.
- `case export`'s `sqlite`/`portable`/`Case::import` lean on SQLCipher's
  `sqlcipher_export()` (`ATTACH DATABASE ... KEY ...`); it returns `NULL`,
  not a row count (`Option<i64>`). Its `KEY` clause differs by source: a
  passphrase (`Portable`) binds as a normal parameter, but the keychain's
  raw hex key needs literal `x'<hex>'` blob syntax in the SQL text — a
  bound parameter there just parses as a wrong passphrase.
- `eumeaus-gui`'s Rust side (`src-tauri`) won't even `cargo check` on
  Linux without real system dev headers first (`libwebkit2gtk-4.1-dev`,
  `libsoup-3.0-dev`, `libjavascriptcoregtk-4.1-dev`, `libayatana-
  appindicator3-dev`, `librsvg2-dev`, `libxdo-dev` — `libwebkit2gtk-4.1-dev`
  pulls most of these in transitively via apt). Not vendorable/cross-
  checkable the way Windows-target `cargo check` is (CLAUDE.md's rusqlite
  gotcha) — this is the *host's own* native linking, no target trick helps.
