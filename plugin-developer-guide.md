# Eumeaus Plugin Developer Guide

This guide is for third-party developers writing an Eumeaus **plugin**: a
collection technique (a username enumerator, a domain lookup, an email
breach checker, whatever your data source is) that plugs into the engine
without modifying it.

If you just want to *use* Eumeaus, see [`CLI.md`](./CLI.md) instead. If you
want the full architectural rationale, see [`SPEC.md`](./SPEC.md) — this
guide only covers what you need to build a plugin.

## 1. What a plugin actually is

A plugin is **a standalone executable**, not a library you link against.
When a scan runs, the engine spawns your binary as a subprocess, your
process starts a small gRPC server, tells the engine how to reach it, and
then answers one kind of request: *"check this value, tell me what you
find."* When the scan is done (or your process is killed), it's over —
there's no persistent plugin runtime, no shared memory, no trust beyond
the protocol boundary.

This buys you two things as a plugin author: you can write a plugin in
**any language** that can speak gRPC and print a line to stdout (this
guide's Quickstart uses Rust and a first-party SDK that does most of the
work for you, but nothing about the protocol requires Rust); and a bug or
crash in your plugin **cannot take down a scan** — the engine marks your
plugin's run `ERROR` and moves on to the next one.

```
┌─────────────┐  spawns, reads stdout handshake   ┌──────────────────┐
│   eumeaus    │ ─────────────────────────────────▶│  your plugin     │
│   engine     │                                    │  (subprocess)    │
│ (plugin-host)│ ◀───────────────────────────────── │                  │
└─────────────┘   gRPC: Describe / Check(stream)    └──────────────────┘
```

## 2. Quickstart (Rust + the SDK)

The fastest path is Rust + `eumeaus-plugin-sdk`, which handles the
handshake, transport (Unix socket / Windows named pipe), and gRPC server
boilerplate for you. You implement one trait:

```rust
use eumeaus_plugin_protocol::{CheckRequest, CheckResult};

#[async_trait::async_trait]
impl eumeaus_plugin_sdk::PluginRuntime for MyPlugin {
    fn describe(&self) -> (String, String) {
        ("my-plugin".to_string(), env!("CARGO_PKG_VERSION").to_string())
    }

    async fn check(&self, request: &CheckRequest) -> Vec<CheckResult> {
        // request.input_value is what you're checking (e.g. an email address)
        // return one CheckResult per thing you found (or one summarizing "not found")
        vec![]
    }
}
```

and a trivial `main`:

```rust
#[tokio::main]
async fn main() {
    eumeaus_plugin_sdk::serve(MyPlugin::new())
        .await
        .expect("plugin server failed");
}
```

`check` is `async fn`, not `fn` — it runs inside the same tokio runtime
the gRPC server uses. If your collection logic does I/O (almost always
true — an HTTP request per lookup, say), use an async client
(`reqwest::Client`, not `reqwest::blocking::Client`) or `tokio::time::
sleep`, never a blocking sleep. A blocking call here doesn't just slow
things down — it panics ("cannot start a runtime from within a runtime").

For the full, real, working example this quickstart is distilled from,
read `crates/eumeaus-username-search-plugin/src/lib.rs` in this
repository — it's a genuine Sherlock-equivalent doing real HTTP GETs
against real sites, not a toy.

## 3. The wire protocol, in depth

If you're not using the Rust SDK — writing in Python, Go, or anything
else — you need to implement this directly. It's small on purpose.

### 3.1 The handshake

Modeled on HashiCorp's `go-plugin` pattern. On startup, your process
must:

1. Start a gRPC server, listening on:
   - a **Unix domain socket**, if running on Unix, or
   - a **Windows named pipe** (`\\.\pipe\<name>`), if running on Windows.
2. Print exactly one line to **stdout**, then flush:

   ```
   EUMEAUS-PLUGIN|1|<network>|<address>|grpc
   ```

   - `EUMEAUS-PLUGIN` — literal magic string.
   - `1` — core handshake version (currently always `1`).
   - `<network>` — literal `unix` or `namedpipe`, matching what you
     actually bound. The host validates this against its own platform
     and rejects a mismatch rather than silently misinterpreting it —
     don't hardcode `unix` and expect it to work on a Windows build.
   - `<address>` — the socket path, or the named pipe's full
     `\\.\pipe\...` name.
   - `grpc` — literal, the wire protocol (the only one supported today).

The engine reads this line within a 5-second timeout, then connects as a
gRPC client. If it doesn't see a valid line in time, or your process
exits before printing one, the plugin load fails with a clear error —
your scan doesn't hang.

### 3.2 The service

```protobuf
service PluginRuntime {
  rpc Describe(DescribeRequest) returns (DescribeResponse);
  rpc Check(CheckRequest) returns (stream CheckResult);
}
```

**`Describe`** — takes nothing, returns your plugin's name and version.
Mostly used for diagnostics; the manifest (below) is the source of truth
the engine actually schedules against.

**`Check`** — takes one `CheckRequest`, returns a **stream** of
`CheckResult`. Streaming matters: a single invocation can check *many*
sources and report each as it completes (the username-search plugin
streams one `CheckResult` per site it checks against one input username).
You don't have to stream incrementally if you don't want to — the SDK's
`check()` trait method just returns a `Vec` and the SDK streams it for
you — but the wire contract supports true incremental streaming if your
plugin benefits from it.

```protobuf
message CheckRequest {
  string scan_id = 1;
  string input_entity_type = 2;         // e.g. "Email"
  string input_value = 3;                // e.g. "person@example.com"
  map<string, string> resolved_credentials = 4;  // see §6, Credentials
  RateLimitConfig rate_limit = 5;
}

message RateLimitConfig {
  uint32 requests_per_sec = 1;
  uint32 timeout_ms = 2;      // 0/unset = use your manifest's default_timeout_ms
}
```

```protobuf
enum ConfidenceStatus {
  CONFIDENCE_STATUS_UNSPECIFIED = 0;
  FOUND = 1;
  NOT_FOUND = 2;
  UNCERTAIN = 3;
  ERROR = 4;
}

message CheckResult {
  ConfidenceStatus status = 1;
  repeated EntityFinding entities = 2;
  repeated RelationshipFinding relationships = 3;
  Provenance provenance = 4;
  string error_message = 5;    // set when status == ERROR
}

message EntityFinding {
  string entity_type = 1;             // e.g. "OnlineAccount"
  string canonical_key = 2;           // stable id for merge matching
  string display_label = 3;
  map<string, string> attributes = 4;
}

message RelationshipFinding {
  string from_canonical_key = 1;
  string to_canonical_key = 2;
  string relationship_type = 3;       // e.g. "HasAccount"
}

message Provenance {
  string source_url = 1;
  string retrieval_method = 2;        // e.g. "HTTP GET"
  string raw_response_sha256 = 3;
  int64 collected_at_unix_ms = 4;
  string plugin_name = 5;
  string plugin_version = 6;
}
```

The full `.proto` file (the single source of truth both sides compile
against) is `crates/eumeaus-plugin-protocol/plugin.proto`.

### 3.3 `ConfidenceStatus` — get this right, it's the whole trust model

This is the most important semantic decision your plugin makes per
result, and the one most worth getting right:

| Status | Means | Example |
|---|---|---|
| `FOUND` | Positively identified something. Populate `entities`/`relationships`. | The username exists on this site. |
| `NOT_FOUND` | Positively confirmed absence. | The username returned a clean 404. |
| `UNCERTAIN` | Could not determine either way — **not the same as an error.** | The site rate-limited you (HTTP 429) or showed a CAPTCHA. |
| `ERROR` | Your plugin itself failed. Populate `error_message`. | A network/DNS failure, an unexpected response shape. |

The `UNCERTAIN` vs. `ERROR` distinction exists because a rate-limited
lookup is not the same claim as "this doesn't exist," and conflating
them would make Eumeaus's whole provenance model dishonest. Absence in
the resulting case graph *is* the negative/uncertain result — don't
manufacture an entity just to represent "not found."

Your plugin should never panic or let a transport error propagate
uncaught — catch it and return `ERROR` with a real `error_message`
instead. One bad result should never crash your whole `check()` call.

## 4. The manifest (`plugin.toml`)

Every plugin ships a `plugin.toml` next to its binary. This is what the
engine reads to discover, validate, and decide whether to trust your
plugin — read it in full before writing one, every field is checked.

```toml
[plugin]
name = "email-lookup"
version = "0.1.0"
description = "Checks whether an email address is associated with known accounts/breaches"
author = "Your Name or Org"
# signature = "..."   # base64 detached signature — see §7, Signing

[compatibility]
engine_min = "0.1.0"
engine_max = "0.x"        # major-version wildcard, or a plain semver like "0.2.0"
protocol_version = "1"

[contract]
input_entity_types = ["Email"]
output_entity_types = ["OnlineAccount"]
output_relationship_types = ["HasAccount"]

[permissions]
network = true
requested_credentials = []   # e.g. ["hibp_api_key"] — see §6, Credentials

[execution]
entrypoint = "./bin/email-lookup-plugin"
default_rate_limit_per_sec = 5
default_timeout_ms = 8000
```

| Section | Field | Notes |
|---|---|---|
| `[plugin]` | `name`, `version` | Required. `name` is the identifier used everywhere (`--plugin <name>`, directory name under `plugins/`). |
| | `description`, `author` | Optional, shown in `plugin list` and the GUI's Plugins screen. |
| | `signature` | Optional (omitted = unsigned). See §7. |
| `[compatibility]` | `engine_min`/`engine_max` | Checked against the running engine's version. `engine_max` accepts either a plain semver or a `"N.x"` major-version wildcard. A manifest declaring itself incompatible is refused at discovery time with a clear message — it doesn't fail the whole scan or crash discovery of other plugins. |
| | `protocol_version` | Must currently be `"1"` — the only version `plugin.proto` defines today. |
| `[contract]` | `input_entity_types` | Which entity types (e.g. `"Email"`) this plugin can be run against. Drives `scan run`'s "run every plugin compatible with `--target-type`" default. |
| | `output_entity_types`/`output_relationship_types` | Declarative documentation of what your plugin can produce — not currently enforced against your actual `CheckResult`s, but keep it accurate; the GUI's Plugins screen surfaces it to investigators deciding what to run. |
| `[permissions]` | `network` | Documents that your plugin makes network calls. Not currently sandboxed/enforced, but a real signal to an investigator deciding whether to trust and run your plugin. |
| | `requested_credentials` | Logical credential names your plugin needs (see §6). |
| `[execution]` | `entrypoint` | Path to your binary, relative to the manifest's own directory. |
| | `default_rate_limit_per_sec` | Your plugin's own recommended pace — advisory; the caller of `scan run` can override it. |
| | `default_timeout_ms` | How long the host waits for your `Check` call before giving up and marking your run `ERROR`. Get this realistic: too short and slow-but-legitimate lookups get killed; too long and a genuinely hung plugin blocks a scan slot for a while. |

An invalid or incompatible manifest is **skipped with a warning**, not a
hard failure — other valid plugins in the same directory still load.
Same philosophy applies to a malformed sidecar config file of your own
(see §8): degrade, don't abort.

## 5. Finding your own files at runtime

The host sets two environment variables on every spawned plugin process.
Neither is part of the gRPC contract — both are filesystem conveniences
you can ignore entirely if you don't need them:

- **`EUMEAUS_PLUGIN_DIR`** — a fresh, per-invocation scratch directory.
  Wiped after the call. Don't rely on anything you put here surviving.
- **`EUMEAUS_PLUGIN_MANIFEST_DIR`** — the stable, canonicalized directory
  containing your `plugin.toml`. This is how you find *your own* sibling
  files — a config file, a data file — without the engine needing to know
  anything about them.

## 6. Credentials

If your plugin needs an API key (an email-breach API, a paid lookup
service, whatever), declare its logical name in
`permissions.requested_credentials`, e.g. `["hibp_api_key"]`. The
investigator provisions it once via:

```console
$ eumeaus credential set hibp_api_key
Value for credential "hibp_api_key":
```

— stored in the OS-native credential store (keychain/Credential
Manager/Secret Service), **never** in the case file. At scan time, the
host resolves every name your manifest declares and injects it into
`CheckRequest.resolved_credentials` (a `map<string, string>`, keyed by
the same logical name) — **only there**, never as a subprocess argv
or environment variable, both of which are visible to other processes
on the same machine.

```rust
async fn check(&self, request: &CheckRequest) -> Vec<CheckResult> {
    let api_key = request.resolved_credentials.get("hibp_api_key");
    // ...
}
```

If a declared credential isn't set, *that plugin's run* is marked
`ERROR` — it doesn't abort the whole scan, and other plugins that don't
need it are unaffected.

## 7. Signing & trust

Plugins load **unsigned by default**. An investigator can instead pass
`--trusted-key <hex>` or `--trust <name>` to `scan run`/`plugin verify`,
which refuses to load any plugin whose `plugin.toml` `signature` field
doesn't verify against that key. There's no baked-in "official" key —
trust is the investigator's decision, one `eumeaus trust add` at a time.

What gets signed is deliberately narrow: `"{name}\n{version}\n
{sha256(entrypoint binary)}"` — not the whole manifest TOML text (which
isn't guaranteed to re-serialize byte-identically), and not any sidecar
config file (see §8) — so editing your plugin's user-facing config
doesn't invalidate its signature. Signing attests to **what code runs**,
under what declared name/version — nothing about its current
configuration.

Sign your installed plugin with the CLI directly:

```console
$ eumeaus plugin sign my-plugin --plugins-dir plugins --signing-key-file signing-key.hex
<base64 signature>
public key: <64 hex chars>
```

`--signing-key-file` points at a file containing a hex-encoded 32-byte
Ed25519 private key (same format/convention as `case export
--sign-key-file`) — bring your own key, generated with whatever
standard Ed25519 tool you already trust; this command never generates
or stores one for you. It writes the resulting signature straight into
your plugin's `plugin.toml` on disk and prints the signer's public
key — hand that to whoever installs your plugin so they can
`eumeaus trust add <name> <that hex key>` or pass it directly via
`--trusted-key`. Because the entrypoint *path* isn't part of what's
signed, a signature computed once stays valid even if the binary gets
moved afterward.

Doing this programmatically instead (e.g. from your own build/release
tooling) means calling the same function the CLI command wraps:
`eumeaus_plugin_host::sign(&signing_key, &manifest)` — see
`eumeaus-plugin-host/src/signature.rs` for the exact payload it signs.

## 8. Externalizing configuration (optional, recommended for larger plugins)

If your plugin has data that should be user-editable without a rebuild
— a site list, an endpoint list, whatever — look for a sidecar file
under `EUMEAUS_PLUGIN_MANIFEST_DIR` first, fall back to a sensible
compiled-in default if it's absent or fails to parse (with a warning on
stderr, not a hard failure). This is exactly the pattern
`eumeaus-username-search-plugin`'s `sites.toml` uses — read
`crates/eumeaus-username-search-plugin/src/lib.rs`'s `load_sites` for
the full worked implementation, including a test-only environment
variable override for isolating test runs from a user's real config.
Remember: this file is **not** covered by your plugin's signature (§7).

## 9. Testing your plugin

Nothing here requires the real Eumeaus engine to be running — a plugin
is just a subprocess that speaks gRPC, so you can and should test it in
isolation:

- **Unit test your `check()` logic directly**, mocking the HTTP layer
  (or whatever I/O you do) rather than hitting real endpoints in CI —
  see `eumeaus-username-search-plugin/tests/check.rs` for the pattern
  (an env-var override redirects requests to a local mock server without
  touching any request/response-handling code).
- **Integration-test against the real host** by installing your plugin
  into a scratch `plugins/` directory and running a real scan against it:

  ```console
  $ eumeaus plugin install ./my-plugin-dist --plugins-dir ./plugins
  email-lookup 0.1.0
  $ eumeaus plugin list --plugins-dir ./plugins
  email-lookup	0.1.0	unsigned	/path/to/email-lookup-plugin
  $ eumeaus --case test.eum entity add --type Email --key someone@example.com
  <entity-id>
  $ eumeaus --case test.eum scan run \
      --plugins-dir ./plugins --target-type Email --target-value someone@example.com
  <scan-id>
  $ eumeaus --case test.eum scan status <scan-id>
  COMPLETED
  $ eumeaus --case test.eum entity show <entity-id>
  ```

  If your plugin crashed, hung, or errored, `scan status` reports it —
  check `entity show`/`audit show` and your plugin's stderr (inherited
  by the host, so it shows up directly in your terminal) to debug.
- **Verify signing separately** with `eumeaus plugin verify <name>
  --trusted-key <hex>` before wiring signature verification into a real
  scan — it runs the exact same check `scan run` would, standalone.

## 10. Full worked reference

`crates/eumeaus-username-search-plugin/` in this repository is a
complete, real, shipped plugin — not a toy example — covering every
piece of this guide:

- `plugin.toml` — a real manifest, including the intentionally-blank
  `signature` field with a comment explaining why (it can only be
  computed against the actual built binary, at packaging time).
- `src/lib.rs` — the `PluginRuntime` implementation, HTTP-based
  `FOUND`/`NOT_FOUND`/`UNCERTAIN`/`ERROR` detection logic, provenance
  construction, and the `sites.toml` externalized-config pattern (§8).
- `src/main.rs` — the minimal `main` that hands off to
  `eumeaus_plugin_sdk::serve`.
- `tests/check.rs` — the mock-server testing pattern (§9).

Read it top to bottom before writing your own plugin from scratch —
almost everything you need is one working example away.

## 11. Where to look for more

- [`SPEC.md`](./SPEC.md) §2.2–2.4, §3.2–3.3 — full architectural
  rationale for the protocol/manifest design.
- [`CLI.md`](./CLI.md)'s `plugin`, `scan`, `credential`, and `trust`
  sections — the investigator-facing side of everything this guide
  covers from the plugin-author side.
- `crates/eumeaus-plugin-protocol/plugin.proto` — the single source of
  truth for the wire contract; if this guide and the `.proto` ever
  disagree, the `.proto` wins.
- `crates/eumeaus-plugin-sdk/src/lib.rs` — if you're writing in Rust,
  read this before reimplementing the handshake/transport yourself;
  it's small and well-commented, and handles the Unix-socket/Windows-
  named-pipe split for you.
