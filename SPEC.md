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
- **No built-in legal-compliance guarantees.** The tool provides mechanisms (rate limiting, proxying, audit logs) but does not itself determine whether a given collection activity is lawful or ToS-compliant in a given jurisdiction — that remains the investigator's responsibility.
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
5. **Multi-case concurrency.** Can one engine instance operate on multiple cases at once (relevant once the GUI supports tabs/windows)? Affects whether the worker pool is scoped per-case or globally.
6. **Evidentiary export/report format.** Is handing off the raw encrypted case file (plus a verification tool) sufficient, or is a human-readable signed report (PDF/JSON bundle) needed for court/legal use?
7. **Legal/ToS posture.** Beyond rate limiting, should the tool take any default stance on robots.txt or per-site ToS (e.g. warn, block by default, leave entirely to the investigator)?
8. **App/engine update mechanism.** Out of scope for v1 itself, but the Tauri GUI milestone will need an auto-update story (affects code-signing setup, which is easier to establish early).
