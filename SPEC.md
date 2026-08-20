# Eumeaus — v1 Specification

## 1. Problem Statement and Non-Goals

### Problem

Investigators (law enforcement, attorneys, journalists, and other OSINT practitioners) need a professional, commercial-grade desktop tool to collect, structure, and visualize open-source intelligence — entities (people, usernames, domains, phone numbers, etc.) and the relationships between them — in a way that is:

- **Extensible**: new collection techniques (e.g. a Sherlock-style username enumerator) can be added as plugins without modifying the core application.
- **Trustworthy**: findings carry provenance and are tamper-evident enough to support evidentiary/investigative use, not just casual research.
- **Operationally safe**: the tool doesn't recklessly hammer third-party sites or leak the investigator's own operational details by default.

### Non-Goals (v1 and beyond, per constraints given)

- **No client-server architecture.** This is a local desktop tool. No multi-user sync, no central server, no cloud storage of case data.
- **No GUI in v1.** The definition of done for v1 is the engine, plugin system, and persistence layer, driven through a CLI. The Tauri-based GUI is a later milestone.
- **No plugin marketplace/registry in v1.** Plugins are locally installed from disk; a distribution/registry system is future scope.
- **No built-in legal-compliance guarantees.** The tool provides mechanisms (rate limiting, proxying, audit logs) but does not itself determine whether a given collection activity is lawful or ToS-compliant in a given jurisdiction — that remains the investigator's responsibility. This explicitly covers robots.txt and per-site ToS too (§8 open question 7): the tool doesn't fetch, parse, warn on, or block against either — a deliberate stance, not a gap.
- **No automatic entity resolution beyond exact-key matching.** Fuzzy/probabilistic identity resolution ("is this the same John Smith?") is explicitly left to human judgment via manual merge/split, not automated in v1.

---

## 2. Architecture

Five modules, split across a Cargo workspace:

### 2.1 `eumeaus-engine` (library crate) — the core

Owns: case lifecycle, the entity/relationship/provenance data model, entity resolution (auto-merge/manual merge/split), scan orchestration (worker pool, resumability), and the public Rust API that both the CLI and (later) the Tauri GUI link against or drive. This is the "brain" — everything else is a client of it or a service it calls out to.

### 2.2 `eumeaus-plugin-host` (library crate, used by the engine)

Owns: plugin discovery (scanning a plugins directory for manifests), manifest parsing/validation (semver compatibility, permission grants), signature verification, subprocess lifecycle (spawn, gRPC handshake, health/timeout monitoring, teardown), and per-plugin rate-limit/proxy configuration enforcement. Modeled on the HashiCorp `go-plugin` pattern: the engine spawns a plugin binary, the plugin starts a local gRPC server on a Unix domain socket (named pipe on Windows) and writes the connection info to stdout as a handshake; the engine then connects as a gRPC client.

### 2.3 `eumeaus-plugin-protocol` (shared library crate, generated from `.proto`)

Owns: the wire contract between engine and plugins — the single source of truth both sides compile against. No logic lives here beyond generated types and any thin serialization helpers.

### 2.4 `eumeaus-plugin-sdk` (library crate, for plugin authors)

Owns: a thin Rust helper library that implements the boilerplate side of the plugin protocol (handshake, gRPC server bootstrap, manifest embedding) so a plugin author writes only the actual collection logic. Plugins are not required to use this SDK or be written in Rust — the protocol is language-agnostic — but this is the first-party ergonomic path, and it's what the v1 PoC plugin uses.

### 2.5 `eumeaus-cli` (binary crate)

Owns: the v1 user-facing interface. A thin wrapper translating CLI subcommands into `eumeaus-engine` API calls, and formatting results for terminal output. This is also the end-to-end test surface for v1.

```
                    ┌─────────────────┐
                    │   eumeaus-cli    │   (v1 interface; GUI is a later,
                    └────────┬─────────┘    separate consumer of the engine)
                             │
                    ┌────────▼─────────┐
                    │  eumeaus-engine   │──── owns SQLite/SQLCipher case file
                    │ (case, entities,  │
                    │  scans, merge)    │
                    └────────┬─────────┘
                             │ uses
                    ┌────────▼─────────┐
                    │ eumeaus-plugin-   │──── spawns plugin subprocesses,
                    │      host         │     gRPC over local socket
                    └────────┬─────────┘
                             │ contract defined by
                    ┌────────▼─────────┐
                    │ eumeaus-plugin-   │
                    │    protocol       │
                    └───────────────────┘
                             ▲
                             │ implemented against (optionally via SDK)
                    ┌────────┴─────────┐
                    │  plugin binaries  │  e.g. username-search (PoC)
                    └───────────────────┘
```

---

## 3. Public Interfaces

### 3.1 Engine API (illustrative signatures, `eumeaus-engine`)

```rust
pub struct Case { /* opaque handle over an open, decrypted case DB connection + file lock */ }

impl Case {
    pub fn create(path: &Path, name: &str) -> Result<Case, CaseError>;
    pub fn open(path: &Path) -> Result<Case, CaseError>;   // acquires exclusive lock
    pub fn close(self) -> Result<(), CaseError>;
    pub fn export(&self, dest: &Path, format: ExportFormat) -> Result<(), CaseError>;

    pub fn add_entity(&mut self, entity_type: EntityType, key: Option<String>,
        attrs: Vec<Attribute>, provenance: Provenance) -> Result<EntityId, EngineError>;
    pub fn merge_entities(&mut self, a: EntityId, b: EntityId, actor: Actor)
        -> Result<EntityId, EngineError>;
    pub fn split_entity(&mut self, id: EntityId, fact_ids: Vec<FactId>, actor: Actor)
        -> Result<EntityId, EngineError>;
    pub fn add_relationship(&mut self, from: EntityId, to: EntityId,
        rel_type: RelationshipType, attrs: Vec<Attribute>, provenance: Provenance)
        -> Result<RelationshipId, EngineError>;
    pub fn list_entities(&self, filter: EntityFilter) -> Result<Vec<Entity>, EngineError>;
    pub fn audit_trail(&self, target: AuditTarget) -> Result<Vec<AuditEvent>, EngineError>;

    pub fn start_scan(&mut self, plugin: PluginRef, target: TargetEntity,
        config: ScanConfig) -> Result<ScanId, EngineError>;
    pub fn resume_scan(&mut self, scan_id: ScanId) -> Result<(), EngineError>;
    pub fn scan_status(&self, scan_id: ScanId) -> Result<ScanStatus, EngineError>;
}

pub struct PluginHost { /* manages plugin subprocess lifecycles */ }

impl PluginHost {
    pub fn discover(plugins_dir: &Path) -> Result<Vec<PluginManifest>, PluginError>;
    pub fn load(&mut self, manifest: &PluginManifest, trust_policy: TrustPolicy)
        -> Result<PluginHandle, PluginError>;
    pub fn invoke(&self, handle: &PluginHandle, request: CheckRequest)
        -> Result<CheckResultStream, PluginError>;  // streaming results
    pub fn shutdown(&mut self, handle: PluginHandle) -> Result<(), PluginError>;
}

pub enum ConfidenceStatus { Found, NotFound, Uncertain, Error(String) }
```

### 3.2 Plugin Protocol (`eumeaus-plugin-protocol/plugin.proto`, illustrative)

```protobuf
service PluginRuntime {
  rpc Describe(DescribeRequest) returns (DescribeResponse);
  rpc Check(CheckRequest) returns (stream CheckResult);   // streamed: one plugin
                                                            // invocation may check
                                                            // many sources (e.g.
                                                            // one result per site
                                                            // for username-search)
}

message CheckRequest {
  string scan_id = 1;
  string input_entity_type = 2;
  string input_value = 3;
  map<string, string> resolved_credentials = 4; // populated from OS keychain by host
  RateLimitConfig rate_limit = 5;
}

message CheckResult {
  ConfidenceStatus status = 1;      // FOUND | NOT_FOUND | UNCERTAIN | ERROR
  repeated EntityFinding entities = 2;
  repeated RelationshipFinding relationships = 3;
  Provenance provenance = 4;
  string error_message = 5;         // set when status == ERROR
}

message Provenance {
  string source_url = 1;
  string retrieval_method = 2;      // e.g. "HTTP GET"
  string raw_response_sha256 = 3;
  int64 collected_at_unix_ms = 4;
  string plugin_name = 5;
  string plugin_version = 6;
}
```

### 3.3 Plugin Manifest (`plugin.toml`, illustrative)

```toml
[plugin]
name = "username-search"
version = "0.1.0"
description = "Checks username existence across social platforms (Sherlock-equivalent)"
author = "Eumeaus Core Team"
signature = "base64-detached-signature-over-manifest-and-binary-hash"

[compatibility]
engine_min = "0.1.0"
engine_max = "0.x"
protocol_version = "1"

[contract]
input_entity_types = ["Username"]
output_entity_types = ["OnlineAccount"]
output_relationship_types = ["HasAccount"]

[permissions]
network = true
requested_credentials = []   # e.g. ["twitter_api_key"] for API-backed plugins

[execution]
entrypoint = "./bin/username-search-plugin"
default_rate_limit_per_sec = 5
default_timeout_ms = 8000
```

### 3.4 CLI Surface (`eumeaus-cli`)

```
eumeaus case create <name> [--path <dir>]
eumeaus case open <path>
eumeaus case list
eumeaus case export <path> --out <file> [--format sqlite|report]

eumeaus entity add --type <Type> [--key <value>] [--attr k=v ...]
eumeaus entity list [--type <Type>] [--filter <expr>]
eumeaus entity show <id>
eumeaus entity merge <id1> <id2>
eumeaus entity split <id> --facts <fact-id,...>

eumeaus relationship add --from <id> --to <id> --type <Type> [--attr k=v ...]

eumeaus plugin list [--installed|--available]
eumeaus plugin install <path>
eumeaus plugin verify <name>

eumeaus scan run --case <path> --plugin <name> --target-type <Type> \
  --target-value <value> [--rate-limit N] [--proxy <url>] [--worker-pool N]
eumeaus scan status <scan-id>
eumeaus scan resume <scan-id>
eumeaus scan list

eumeaus credential set <name>      # prompts interactively, stores via OS keychain
eumeaus credential list
eumeaus credential remove <name>

eumeaus audit show --entity <id> | --relationship <id> | --scan <id>
```

---

## 4. Data Model / On-Disk Formats

### 4.1 Case File

A case is a single **SQLCipher-encrypted SQLite file** (`.eum` extension). The encryption key is generated at case creation and stored in the OS-native credential store (Keychain / Credential Manager / Secret Service), referenced by the case's UUID. Opening a case requires the host OS user account that created it (or one with access to the same keychain entry) — see Open Question 1 regarding portability/handoff.

Opening a case acquires an **exclusive lock** (OS file lock) for the duration; a second attempt to open the same case file fails fast with a clear "case already open" error rather than risking concurrent-write corruption.

### 4.2 Core Schema (peer entity/relationship model)

No entity type is privileged as a root — any type can be an investigation's starting point.

**`entities`**
| column | type | notes |
|---|---|---|
| id | UUID PK | |
| entity_type | TEXT | see starter taxonomy below |
| canonical_key | TEXT, nullable | normalized value used for exact-match auto-merge (e.g. lowercased username) |
| display_label | TEXT | |
| created_at, updated_at | INTEGER (unix ms) | |

**`entity_attributes`** — freeform key/value bag per entity, each tied to the fact that produced it (see below), so attributes are never "just there" without provenance.

**`relationships`**
| column | type | notes |
|---|---|---|
| id | UUID PK | |
| from_entity_id, to_entity_id | UUID FK | |
| relationship_type | TEXT | |
| created_at | INTEGER | |

**`facts`** — the append-only provenance/audit log. Every attribute, every relationship, and every manual edit is backed by a fact row. Facts are **never updated or deleted**; corrections happen by inserting new facts and, for merges/splits, an explicit audit event.

| column | type | notes |
|---|---|---|
| id | UUID PK | |
| entity_id / relationship_id | UUID FK, one set | nullable pair, exactly one populated |
| scan_id | UUID FK, nullable | null for manually-entered facts |
| source | TEXT | plugin name, or `"user"` for manual entry |
| source_version | TEXT | plugin semver, or app version for manual entry |
| confidence_status | TEXT | FOUND / NOT_FOUND / UNCERTAIN / ERROR |
| source_url | TEXT, nullable | |
| retrieval_method | TEXT, nullable | |
| raw_response_sha256 | TEXT, nullable | |
| collected_at | INTEGER | |

**`audit_events`** — records merges, splits, and other structural edits (who/what/when/why), distinct from `facts` (which record collected data). Append-only.

**`scans`** / **`scan_plugin_runs`** — scan orchestration and resumability state.

| `scans` | | | `scan_plugin_runs` | |
|---|---|---|---|---|
| id | UUID PK | | scan_id | UUID FK |
| target_entity_id | UUID FK | | plugin_name, plugin_version | TEXT |
| config_snapshot | JSON | worker pool size, rate limits, proxy used | status | PENDING / RUNNING / SUCCESS / TIMEOUT / ERROR / SKIPPED |
| status | PENDING / RUNNING / COMPLETED / PARTIALLY_FAILED / ABORTED | | started_at, completed_at | INTEGER |
| started_at, completed_at | INTEGER | | error_message | TEXT, nullable |

On case open, any `scan_plugin_runs` row left in `RUNNING` state with no live process backing it (i.e. the app crashed mid-scan) is reconciled to `PENDING`, making it eligible for `scan resume` rather than being falsely reported as complete or lost.

### 4.3 Starter Entity/Relationship Taxonomy (confirmed, see Open Question 3)

Entity types: `Person`, `Username`, `Email`, `PhoneNumber`, `Domain`, `IPAddress`, `OnlineAccount`, `Organization`, `Location`, `Document`, `Image`, `Vehicle`, `CryptoWallet`, `Url`, `Custom` (escape hatch).

Relationship types: `HasAccount`, `Owns`, `AssociatedWith`, `LocatedAt`, `MemberOf`, `ResolvesTo`, `Mentions`, `RelatedTo` (generic catch-all) — `RelationshipType` also carries a `Custom` escape hatch in the implementation, even though none is listed here (see CLAUDE.md's documented deviations).

### 4.4 Entity Resolution

On fact ingestion, if a new finding's `(entity_type, canonical_key)` exactly matches an existing entity, it is automatically merged into that entity (new fact appended, no new entity row created). Anything short of an exact key match (fuzzy name similarity, partial overlap) is **never** auto-merged — it produces a separate entity that a human can merge explicitly via `entity merge`, which itself is recorded as an `audit_event`. Conflicting attribute values from different facts on the same entity are never silently resolved — both facts remain visible with full provenance; the "current" value shown is the most recent by `collected_at`, clearly flagged as one of several disagreeing sources when a conflict exists.

### 4.5 Credentials

Never stored in the case file. Stored in the OS-native credential store under an app-scoped namespace, referenced by logical name (e.g. `"twitter_api_key"`). A plugin's manifest declares which named credentials it needs; the plugin host resolves and injects them into the gRPC request at invocation time — they are never passed via subprocess argv or environment variables (both are visible to other processes on the same machine).

---

## 5. Error Handling and Failure Modes

| Failure | Handling |
|---|---|
| Plugin crashes / hangs | Per-plugin timeout (manifest-declared default, overridable); on timeout or crash, that plugin's `scan_plugin_runs` row is marked ERROR with the failure reason, the scan continues for all other plugins. One bad plugin never aborts a scan. |
| Target site blocks/rate-limits (429, CAPTCHA wall) | Distinguished from a hard error: tagged `UNCERTAIN`, not `ERROR` — the plugin could not determine presence/absence, which is different from the plugin itself failing. |
| Plugin manifest invalid or engine-incompatible | Plugin is refused at discovery/load time with a clear message; other valid plugins still load normally. |
| Plugin unsigned / signature invalid | Refused to load by default. An explicit `--allow-unsigned` CLI flag permits loading with a loud warning, for local development only. |
| Corrupt or tampered case file | SQLCipher decryption failure or an integrity-hash mismatch during open causes a hard failure with a specific error — never a silent partial load. |
| Case already open elsewhere | `case open` fails fast with "case already open" rather than risking concurrent writers. |
| App crash mid-scan | On next open, orphaned `RUNNING` `scan_plugin_runs` rows (no live process) are reconciled to `PENDING`; `scan resume` picks up only the incomplete work, not already-succeeded plugin runs. |
| Disk full / write failure mid-scan | The failing write is contained to a single transaction/fact insert; that fact or plugin run is marked ERROR, other already-committed data is untouched (SQLite transactional guarantees). |
| Conflicting facts on merge | Never auto-resolved; both retained with provenance, surfaced to the investigator rather than picking a silent winner. |
| Network/DNS errors inside a plugin | Surfaced as `ERROR` status with `error_message` populated; caught at the plugin-host boundary via timeout if the plugin itself doesn't handle it gracefully. |

---

## 6. Test Strategy

- **Unit tests** across all crates for schema logic, manifest parsing, merge/split semantics, and scan state-machine transitions.
- **Contract tests** for the plugin protocol: a set of intentionally misbehaving stub plugins (hangs, crashes, returns malformed data, exceeds rate limit) verify the plugin host's isolation and timeout guarantees.
- **Recorded HTTP fixtures (cassettes)** for the username-search PoC plugin: captured real responses per site replayed deterministically in CI. A separate, manually-triggered **live test suite** runs the plugin against real sites periodically to catch site-format drift (Sherlock's own chronic problem) — never part of the default CI run.
- **End-to-end test (the v1 proof)**: drives the actual CLI, not internal APIs directly:
  1. `case create` a fresh encrypted case.
  2. `scan run` the username-search plugin against a known test username with a mix of known-existing and known-nonexistent target sites (via cassette fixtures).
  3. Kill the process mid-scan (simulated crash); `scan resume` and confirm only incomplete plugin runs re-execute.
  4. Assert the resulting entity graph (`OnlineAccount` entities, `HasAccount` relationships) matches expected fixture-derived truth, with correct `FOUND`/`NOT_FOUND`/`UNCERTAIN` tagging.
  5. `case close`, then `case open` again; assert the graph and full audit trail (facts, provenance hashes, scan history) are byte-for-byte intact.
  6. Attempt to open the case file directly with plain `sqlite3` and confirm it is unreadable without the key (proves encryption-at-rest is real, not decorative).

This single test exercises every module — engine, plugin host, protocol, persistence, resumability, and provenance — together, which is the actual point of the v1 milestone.

---

## 7. Milestones (ordered, each independently verifiable)

- **M0 — Workspace scaffolding.** Cargo workspace with all five crates wired together. *Verify:* `cargo build` succeeds; `eumeaus-cli --help` runs.
- **M1 — Case lifecycle & encrypted persistence.** Create/open/close a case; core schema migrations in place. *Verify:* `case create`/`case open` round-trip; case file is confirmed unreadable by plain `sqlite3` without the OS-keychain key.
- **M2 — Manual entity/relationship graph via CLI.** No plugins yet — CRUD entities/relationships by hand, with provenance recorded as `"user"`-sourced facts, plus merge/split and their audit events. *Verify:* add entities/relationships, merge two, confirm audit trail shows the merge.
- **M3 — Plugin protocol & host.** `.proto` contract, subprocess spawn + gRPC handshake, signature verification, timeout handling. *Verify:* a trivial stub plugin is discovered/invoked successfully; a deliberately-hanging stub plugin is correctly timed out without crashing the host.
- **M4 — Scan orchestration.** Worker pool, rate limiting, scan-state persistence and resumability, result ingestion with auto-merge. *Verify:* a scan against several stub plugins respects the worker-pool bound; killing and resuming mid-scan completes only the remaining work.
- **M5 — Username-search PoC plugin (the v1 definition-of-done milestone).** Real Sherlock-equivalent plugin, signed manifest, cassette test suite. *Verify:* the full end-to-end test in Section 6 passes.
- **M6 — Credential management.** OS-keychain-backed credential storage and injection for plugins that declare `requested_credentials`. *Verify:* a test plugin receives an injected credential correctly; confirm it never appears in the case file, subprocess argv, or environment.

---

## 8. Open Questions

1. **RESOLVED — Case portability vs. keychain-tied encryption.** `case export --format portable` produces a SQLCipher copy re-keyed with a user passphrase (SQLCipher's own passphrase mode, not a hand-rolled KDF) instead of the local keychain key; `case import` decrypts it with that passphrase and turns it into a brand-new, fully ordinary local case (fresh UUID, fresh keychain key) on the receiving machine — see CLI.md's `case` section. `Case::open` itself was deliberately left untouched (still keychain-only): rather than teaching it to understand two key sources, export/import round-trips through a completely separate case identity, which also sidesteps any question of what it would mean for two machines to both hold a case under the same UUID.
2. **RESOLVED — Plugin signing authority.** v1 has no baked-in "official" key — inventing one for a project with no real key-custody process would be security theater, and §1 already rules out a marketplace/registry for v1. Instead, the investigator *is* the signing authority: `eumeaus trust add/list/remove` maintains a local, plain-file store of named public keys they've explicitly chosen to trust, and `scan run --trust <name>` / `plugin verify --trust <name>` reference one by name instead of retyping `--trusted-key <hex>` every time (still available too) — see CLI.md's `trust` section. The eventual third-party-plugin trust process remains genuinely open — there's no third-party plugin ecosystem yet for it to serve.
3. **RESOLVED — Confirm the starter entity/relationship taxonomy** (Section 4.3). Validated through six milestones of real use (case CRUD, the real username-search plugin, every CLI.md example) without needing a workaround — and both `EntityType`/`RelationshipType` already carry a `Custom(String)` escape hatch (§4.4), so "incomplete" was never actually a blocker; any string works today. Added `CryptoWallet` and `Url` as first-class types — common OSINT targets that don't map cleanly onto anything existing (a wallet isn't an `OnlineAccount`; a URL of interest isn't necessarily a profile page). Caveat carried over from the *existing*, unchanged auto-merge normalization (§4.4: canonical keys are trimmed and lowercased): a case-sensitive wallet address or URL path gets lowercased for matching purposes like every other type's key — not a new issue introduced by adding these two, but worth knowing before relying on exact-case wallet/URL matching.
4. **RESOLVED — Data retention/redaction.** `eumeaus fact redact <fact-id> --reason <text>` (`Case::redact_fact`) implements **true deletion, not crypto-shredding**: the targeted fact row and its `entity_attributes`/`relationship_attributes` row(s) are actually removed from the database (verified: the redacted value doesn't survive anywhere in the file, including a fresh `case export --format sqlite`). Crypto-shredding — encrypting each fact separately and destroying just its key — was rejected as real complexity (a whole per-value encryption/key-management subsystem on top of SQLCipher's existing case-level encryption) for a niche benefit (preserving unreadable ciphertext bytes) that a legal takedown doesn't actually need. What preserves the append-only design's tamper-evidence goal instead: a permanent `audit_events` row (`event_type = "redact"`) recording that a fact existed and was removed — its id, source, and collection time, plus the investigator's stated reason — but never repeating the redacted value itself. Deliberately fact-level only, not full entity erasure: an entity's own `canonical_key`/`display_label` live on the entity row, not per-fact, and are untouched by redacting the facts that originally produced them.
5. **RESOLVED — Multi-case concurrency.** Yes, already supported by the existing architecture, with no new mechanism needed: `Case` is fully self-contained (own `Connection`, own OS file lock, own ephemeral `tokio::runtime::Runtime` spun up per scan and torn down after) — nothing engine-wide is shared, so a future caller (the GUI, per this question's own framing) can hold several `Case`s open in one process simultaneously. `worker_pool` is already scoped *per scan invocation* — narrower than even "per-case." Proven, not just asserted: `eumeaus-engine/src/scan.rs`'s `two_different_cases_scan_concurrently_in_one_process` test opens two different cases on two threads and times both scans running genuinely concurrently, not serialized by any hidden global lock. Explicitly deferred: a *cross-case* concurrency ceiling (e.g. "never more than N concurrent HTTP requests system-wide, regardless of how many tabs are scanning") doesn't exist — it only matters once a real multi-tab GUI exists to create that scenario, which is out of v1 scope.
6. **RESOLVED — Evidentiary export/report format.** Both, for different purposes — and both are now real: the raw case file (already tamper-evident via SQLCipher's own per-page HMAC, and portable via §8.1's `--format portable`) is the full-fidelity forensic artifact; `case export --format report`/`--format html` (JSON and, new, a self-contained HTML document — headers/tables, no external CSS/JS, print-to-PDF-able from any browser) are human/court-facing distillations of the same underlying facts. What was missing was "signed": JSON/HTML exports are plaintext with no tamper-evidence of their own (unlike the SQLCipher formats), so `case export --sign-key-file <path>` now writes a detached Ed25519 signature (`<out>.sig`) alongside any export, and `eumeaus report verify <report> --sig <file> (--trusted-key <hex> | --trust <name>)` checks it — reusing §8.2's trust store. Deliberately not built: PDF generation (a real rendering library for a benefit a browser's own print-to-PDF already covers) or any investigator-identity/key-generation system — `--sign-key-file` takes a hex private key the investigator generates and manages entirely with their own tools, same "bring your own key" philosophy as §8.2's trust store.
7. **RESOLVED — Legal/ToS posture.** Leave entirely to the investigator — the third option this question itself named, and the same position §1's "no built-in legal-compliance guarantees" non-goal already took for lawfulness/ToS-compliance generally, now made explicit for robots.txt specifically. No code changes: the tool doesn't fetch, parse, warn on, or block against robots.txt or a site's ToS today, and that's a deliberate stance rather than an oversight — building any warn/block mechanism would be a real product/values decision (how much the tool should second-guess an investigator's own judgment call on a given site) that a v1 CLI tool for a single investigator's own use doesn't need to make on their behalf.
8. **App/engine update mechanism.** Out of scope for v1 itself, but the Tauri GUI milestone will need an auto-update story (affects code-signing setup, which is easier to establish early). Still genuinely open — no Tauri project, GUI code, or binary-distribution pipeline exists yet, so there's nothing to update and nothing to decide against real constraints. Pointer for whoever starts that milestone: evaluate Tauri's own updater plugin first (it's the framework-native answer and likely needs the least custom code), which will still need per-platform code signing sorted out early — Apple Developer ID + notarization for macOS, an Authenticode certificate for Windows — since both are slow, identity/business-verification-gated processes worth starting well before the first GUI release is otherwise ready.

---

## 9. GUI (v2, Tauri) — Design

Everything below is design for a *second* client of `eumeaus-engine`, alongside — not replacing — `eumeaus-cli`. Nothing in §1–§8 changes: this is additive scope, worked out before writing GUI code per this milestone's own process (a design pass first, mirroring how §1–§8 governed v1 before M0 started).

### 9.1 Framework choice

Tauri 2.x, per §8.8's existing pointer. Rationale, made explicit here: `eumeaus-engine` is a Rust library today (§2.1) — Tauri embeds it directly as the backend process with no FFI/IPC boundary of its own invention, unlike Electron (which would need a Node/Rust bridge) or a client-server rewrite (ruled out entirely by §1's non-goals). It also brings a framework-native updater (`tauri-plugin-updater`, §9.4) rather than a hand-rolled one. The alternative considered and rejected: a pure-Rust immediate-mode toolkit (egui/iced) avoids a web frontend entirely, but this app's real UI surface — entity/relationship tables, fact timelines, forms-heavy CRUD, a case-wide search — is exactly the kind of content-dense, text-heavy UI that HTML/CSS handles better than immediate-mode widget layout; Tauri keeps that option (an HTML/CSS/JS frontend) without giving up a Rust backend.

Frontend framework: **React + TypeScript**, confirmed — largest Tauri-adjacent ecosystem and plugin compatibility, at the cost of a heavier toolchain than Svelte/vanilla.

### 9.2 Architecture — IPC boundary

The GUI's Rust backend is `crates/eumeaus-gui/src-tauri` (§9.7's resolved workspace-placement decision), a new crate that depends on `eumeaus-engine` directly — in-process, not spawned, unlike a plugin (§2.2's subprocess isolation exists for *untrusted third-party* plugin code; the GUI's own backend is first-party, same trust level as `eumeaus-cli`).

Each `eumeaus-cli` subcommand (§3.4) has a corresponding `#[tauri::command]` function — the command layer is `eumeaus-gui`'s equivalent of `eumeaus-cli`'s thin-wrapper role, calling the same `Case`/engine API, not a new one. `invoke()` from the frontend is the IPC boundary.

State: `Case` owns a `rusqlite::Connection`, which is `!Sync` (`eumeaus-engine/src/scan.rs`'s own module doc explains why scans bridge this with an owned `tokio::runtime::Runtime` and `block_on`). Tauri's managed state (`tauri::State`) holds one open `Case` behind a plain `std::sync::Mutex` (not `tokio::sync::Mutex` — every command's actual `Case` call runs inside `tauri::async_runtime::spawn_blocking`, a blocking-pool thread, where a blocking mutex is the right tool, not an async one); concurrent command invocations serialize on it rather than each opening a second connection to the same file — consistent with §8.5's finding that a `Case` is already safe to hold open per-instance, but says nothing about two connections to *one* case file, which SQLCipher's exclusive OS file lock (CLAUDE.md) already forbids regardless of GUI or CLI.

`scan run` is long-lived (worker-pool-bounded, potentially many plugin invocations) — a blocking command/response doesn't fit, and turned out to need one genuine new engine API, not just a wrapper (correcting this section's original assumption that G3 wouldn't need one): `resume_scan` holds `Case`'s one connection for the *entire* scan, so nothing else — including a same-process poller sharing that `Case` — could observe `scan_plugin_runs` mid-flight the way a second CLI invocation of `scan status` could. `Case::resume_scan_with_progress` (additive; existing `resume_scan`/its callers are untouched) takes a `tokio::sync::mpsc::UnboundedSender<ScanProgressEvent>` and sends one synchronously at each status transition, from inside the same orchestrating call `resume_scan` already makes them from — no second DB connection, no polling. `scan_run` returns as soon as `create_scan` succeeds (same split `eumeaus-cli`'s own `scan run` already uses), then a background task drives `resume_scan_with_progress` and forwards each event to the frontend via a Tauri event (`emit`/`listen`) as it arrives.

### 9.3 Screens (full eventual surface, not all in one milestone — see §9.6)

The actual visual/interaction design for these screens was handed over from Claude Design (claude.ai/design) after G0–G6 shipped functionally-plain versions, and implemented (merged to `main`): a dark, information-dense desktop shell with a custom titlebar (`decorations: false`) and sidebar navigation across Overview/Entities/Graph/Scans/Plugins/Settings. Every screen below wires to real backend commands — the design handover's own preview necessarily used fixture data, none of which survived translation into the app (`crates/eumeaus-gui/src/api.ts` is the full real command surface). Two adaptations where the design assumed something the backend doesn't have: Graph uses a real circular layout (the design's hand-placed node coordinates don't exist for arbitrary case data, and a force-directed-layout dependency isn't in this project); the launcher's "recent cases" is a real directory browser (`case_list`) rather than invented history, since no MRU-case-tracking exists in the backend.

- Case list / create / open / close (`case list/create/open/export`)
- Entity list and detail (facts, attributes, relationships) (`entity show`, `fact`)
- Relationship view — implemented as a real graph visualization (circular layout, see above), not just the table originally planned as the first cut
- Scan run/monitor/resume, live-updating via §9.2's event stream (`scan run/list/resume`)
- Plugin management: list/install/verify/trust (`plugin`, `trust`)
- Credential management (`credential set/list/remove`) — GUI's own analog of CLI's `rpassword` TTY prompt is a normal password `<input>`, not a gotcha here the way it was for CLI headless testing
- Audit log viewer (`audit`)
- Report export + signature verify (`case export --format report/html`, `report verify`) — implemented as an Export/Verify card pair on the Overview screen (`report_state.rs`'s `case_export`/`report_verify`, wrapping the same `Case::export`/`report::sign_export`/`verify_report` calls as the CLI); the one screen from this list the design handover didn't cover, added after a post-merge dogfooding pass surfaced the gap

### 9.4 Packaging, signing, update (resolves §8.8)

Implemented as `.github/workflows/release-gui.yml` — deliberately a *separate* workflow from the CLI's `release.yml` (§7), not new matrix legs on it, triggered by its own `gui-v*` tag namespace rather than the CLI's `v*`: the GUI is a different, still-mid-milestone product surface, and coupling its release cadence to the CLI's (or risking the CLI's already-working pipeline while iterating here) wasn't worth it. Uses `tauri-apps/tauri-action@v0` to produce platform installers — NSIS `.exe`/WiX `.msi` on Windows, `.AppImage`/`.deb` on Linux — with `releaseDraft: true`, same human-review-before-publish stance as the CLI's release. macOS is out of scope for now (§9.7).

Windows code signing (Authenticode, needed to avoid a SmartScreen warning) turned out to still be unresolved when G6 was actually implemented — a real certificate is identity-verification-gated and a real ongoing cost, genuinely outside what could be done inside this build. **Resolution: ship unsigned for now, sign later** — nothing in the pipeline blocks on it; a certificate can be wired into `release-gui.yml`'s existing steps whenever one exists, without restructuring anything.

Update mechanism: `tauri-plugin-updater` + `tauri-plugin-process` (for `relaunch()` after install), the framework-native answer §8.8 already pointed at. Its signing keypair (Tauri's own minisign-based scheme — a third distinct signing scheme in this project alongside §8.2's plugin-manifest Ed25519 signing and §8.6's report detached-signature Ed25519 signing, deliberately not unified, since they verify different things for different audiences) was generated for real during G6 (`tauri signer generate`) — this one genuinely could be done inside the build, unlike the Authenticode certificate. The public half is committed in `tauri.conf.json`'s `plugins.updater.pubkey`; the private half is a `TAURI_SIGNING_PRIVATE_KEY` GitHub Actions secret, never committed. `release-gui.yml` signs update artifacts and publishes `latest.json` automatically (`includeUpdaterJson: true`); the endpoint (`plugins.updater.endpoints`) points at `.../releases/latest/download/latest.json`, the same GitHub-Releases-as-CDN pattern `install.sh`/`install.ps1` already use for the CLI. Verified live: the frontend's "Check for updates" button reaches this endpoint and surfaces a clean error (no release published yet, so a 404) rather than crashing — the wiring is correct even though no real update exists yet to actually apply. That last step — publish a real `gui-v0.1.0` release, then a `gui-v0.1.1` to test an actual detected-and-applied update — needs a deliberate decision to cut a real release, not something to do unprompted.

### 9.5 Relationship to the CLI

The GUI does not deprecate or replace `eumeaus-cli` — both are first-party clients of `eumeaus-engine`, same standing §2.1 already gives the CLI. A case created by one opens fine in the other; nothing about a case's on-disk format (§4) or keychain entry is GUI- or CLI-specific.

What's *not* safe, and isn't new: §4's exclusive OS file lock means the GUI and CLI (or two GUI instances) can't both hold the same `.eum` file open concurrently — an existing constraint (true since M1) that only becomes something a user might actually hit once a second program exists to collide with the first.

### 9.6 Milestones (ordered, each independently verifiable — same convention as §7)

- **G0 — Scaffold.** Tauri project builds and runs on Linux and Windows (matching §7's existing release matrix); one trivial command round-trips into `eumeaus-engine` (e.g. reporting its version). *Verify:* `tauri dev` opens a window; the round-trip command's result renders on screen.
- **G1 — Case lifecycle.** Create/open/close a case from the GUI, backed by the real keychain + SQLCipher path (§4), not a mock. *Verify:* a case created in the GUI opens correctly via `eumeaus-cli case open` and vice versa.
- **G2 — Entity/fact browsing (read-only).** List/detail views wrapping `entity show` and fact history. *Verify:* entities/facts created via CLI render correctly in the GUI with no data-shape translation bugs.
- **G3 — Scan run + live progress.** Wraps `scan run`, using §9.2's event-stream design. *Verify:* a scan started in the GUI against the real `username-search` plugin (§M5) shows live per-plugin progress and completes with the same result set `scan list` would report via CLI.
- **G4 — Entity/relationship editing, merge/split.** Write path, not just browsing. *Verify:* a GUI-initiated merge produces the same `audit_events` row shape §8's merge/split tests already check for CLI-initiated merges.
- **G5 — Plugin/credential/trust management.** Screens for `plugin`, `credential`, `trust`. *Verify:* a plugin installed/trusted via GUI is usable by a CLI-initiated scan and vice versa.
- **G6 — Packaging, signing, updater.** §9.4 in full; resolves §8.8. *Verify:* a signed installer builds in CI for each target platform; a running GUI instance detects and applies an update from a real (test) release.

### 9.7 Open questions

- **RESOLVED — Frontend framework.** React + TypeScript (§9.1).
- **RESOLVED — macOS.** Dropped for now — the GUI inherits the CLI's existing Linux+Windows-only release matrix (§7) rather than adding a third platform. Revisit if demand shows up; `eumeaus-engine`'s `bundled-sqlcipher-vendored-openssl` build has never been tried on macOS at all, so this also sidesteps an unknown.
- **RESOLVED — Workspace placement.** Inside the existing `crates/` workspace, not a separate `apps/` directory — `crates/eumeaus-gui/` holds the frontend (React+TS, Vite) at its root and the Tauri Rust backend in `crates/eumeaus-gui/src-tauri/` (a normal workspace member), so `cargo build --workspace` keeps covering it like every other crate.
