---
name: plugin-development
description: Writing or modifying an Eumeaus plugin (real or test-fixture), or touching the plugin protocol/host/SDK internals — subprocess+gRPC wiring, signing, scan resumability persistence.
---

# Plugin development

## `PluginRuntime::check` is `async fn`, not `fn`

It runs inside the same tokio runtime the gRPC server uses. Calling a
*blocking* HTTP client (e.g. `reqwest::blocking`) from it panics ("Cannot
start a runtime from within a runtime"). Use an async client
(`eumeaus-username-search-plugin` uses plain `reqwest::Client`) or
`tokio::time::sleep(...).await`, never `std::thread::sleep`, to simulate a
delay in a fixture plugin.

## Test-fixture plugins are Cargo examples, not `[[bin]]` targets

`eumeaus-plugin-host/examples/stub_*.rs`, `eumeaus-engine/examples/scan_*.rs`,
`eumeaus-cli/examples/quick_check.rs`. A same-package `[[bin]]` needing a
dev-dependency (they depend on `eumeaus-plugin-sdk`) breaks plain `cargo
build`, since dev-deps aren't linked for `[[bin]]`s outside `cargo test`.
Examples are exempt from plain `cargo build` and still get dev-deps under
`cargo test`.

There's no `CARGO_BIN_EXE_`-style env var for a binary in a *different*
crate, nor for examples at all — every workspace crate's compiled
artifacts land under one shared `target/<profile>/`, so tests locate a
binary relative to their own `current_exe()` instead:

```rust
fn workspace_bin(name: &str) -> PathBuf {          // regular [[bin]], any crate
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    if p.ends_with("deps") { p.pop(); }
    p.push(name);
    p
}
// same, but push("examples").push(name) at the end, for an [[example]]
```

Fixture plugins are duplicated per-crate (plugin-host's `stub_*` vs.
engine's `scan_*` vs. cli's `quick_check`) rather than shared, so `cargo
test -p eumeaus-engine` alone still works — only `-p`'s own dev-deps get
built, not another crate's.

`eumeaus-username-search-plugin` is different: a real shipped `[[bin]]`,
not a fixture, so it's just built normally by `cargo build --workspace`
and located via `workspace_bin`, not a dev-dependency trick.

## Finding a plugin's own files at runtime

`eumeaus-plugin-host` sets two env vars on every spawned plugin process
(`host.rs`'s `load`): `EUMEAUS_PLUGIN_DIR` — a fresh, per-invocation
scratch dir (currently just holds the gRPC handshake socket; wiped after
the call) — and `EUMEAUS_PLUGIN_MANIFEST_DIR` — the stable directory
containing the plugin's own `plugin.toml`, canonicalized. Neither is part
of `plugin.proto`; both are filesystem conveniences a plugin can ignore.

`EUMEAUS_PLUGIN_MANIFEST_DIR` is how a plugin finds *its own* sibling
config/data files without the engine or protocol needing to know anything
about them — `eumeaus-username-search-plugin`'s `sites.toml` (its site
list, externalized so a user can add/remove checks without a rebuild) is
the worked example: `load_sites()` checks
`EUMEAUS_USERNAME_SEARCH_SITES_FILE` (an explicit override, used by
tests) first, then `<EUMEAUS_PLUGIN_MANIFEST_DIR>/sites.toml`, falling
back to a compiled-in default if neither resolves or the file fails to
parse (warning on stderr — same "degrade, don't abort" policy SPEC.md §5
uses for a bad plugin manifest). Worth remembering: a plugin's signature
covers only `name + version + entrypoint-binary-hash` (see Signing,
below) — never a sidecar config file like this one — so externalizing
data this way means it's *not* covered by signature verification.

## Signing

`eumeaus-plugin-host/src/signature.rs` signs
`"{name}\n{version}\n{sha256(entrypoint binary)}"` — SPEC.md §3.3 doesn't
specify an exact encoding; the manifest TOML itself was rejected as the
signed payload because re-serializing it to strip the `signature` field
isn't guaranteed byte-identical to whatever tool produced it. Because the
entrypoint *path* isn't part of what's signed, a signature computed once
stays valid even after rewriting `entrypoint` to point wherever a test
build put the binary — sign against an unsigned draft, then write the
final manifest with that signature filled in (see any test's
`write_signed_manifest`/`write_manifest_text` pair).

`eumeaus_plugin_host::sign(&signing_key, &manifest)` is real API, not just
test scaffolding — what a future `eumeaus plugin sign` tool would call.

`scan run --trusted-key <hex-ed25519-pubkey>` sets
`TrustPolicy::RequireSignature`; omit it and plugins load `AllowUnsigned`.
No credential/config store exists yet (M6+) to read a *default* trust key
from, so every invocation passes it explicitly. A scan's `plugins_dir`/
`TrustPolicy` are persisted as JSON in `scans.config_snapshot` at
`start_scan` time and restored on `resume_scan` (whose SPEC.md §3.1
signature takes only `scan_id`).

## Async bridge

`eumeaus-plugin-host`'s public API (`load`/`invoke`/`shutdown`) is async;
the rest of the engine is sync (`rusqlite`). `eumeaus-engine`'s
`Case::start_scan`/`resume_scan` bridge it by owning a one-shot
`tokio::runtime::Runtime` and `block_on`-ing the whole scan to completion
— see `eumeaus-engine/src/scan.rs`'s module doc for the full reasoning
(only per-plugin `invoke()` calls run concurrently, bounded by
`worker_pool`; every DB write stays in the orchestrating task since
`rusqlite::Connection` is `!Sync`).

## tonic/tower version pin

Plugin subprocess/gRPC wiring uses the tonic 0.12.x line, not latest —
0.13+ split prost codegen into a separate `tonic-prost-build` crate with a
less-established API; 0.12.x's classic `tonic_build::compile_protos` is
better documented and lower-risk. `tower` is pinned to exactly what tonic
0.12 depends on (`0.4.7`) — bumping it alone would leave two incompatible
`tower` majors in the dependency graph.
