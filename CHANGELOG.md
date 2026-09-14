# Changelog

Notable changes to eumeaus's released binaries. The CLI and GUI ship on
separate version tracks — `v*` tags for the CLI (bundled with its
plugins) and `gui-v*` tags for the GUI — see [GitHub
Releases](https://github.com/RedRockerSE/eumeaus/releases) for downloads
and full asset lists.

## CLI (`eumeaus`)

### [v0.1.8] - 2026-09-14

#### Fixed
- `case create`/`case open` failed on Windows with `database error: disk
  I/O error`. `Case` held two independent locks on the same file — a
  manual `std::fs::File::try_lock()` and SQLCipher's own internal file
  locking — which collided under Windows' mandatory `LockFileEx`
  semantics (harmless on Linux, where the two locking APIs never
  interact). Replaced the manual lock with SQLite's own `PRAGMA
  locking_mode = EXCLUSIVE`, so there's only ever one lock on the file.

### [v0.1.7] - 2026-09-12

#### Added
- `email-accounts` plugin (#24): checks whether an email address has a
  registered account on Twitter, Spotify, Duolingo, and other sites, via
  each site's own signup/password-reset validation endpoint.
- `subdomain-lookup` plugin (#25): enumerates a domain's subdomains via
  Certificate Transparency logs (crt.sh), with automatic retry on
  transient lookup failures.
- Python proof-of-concept plugin
  (`eumeaus-phone-lookup-plugin-python`), demonstrating the plugin
  protocol doesn't require Rust.

#### Changed
- Redesigned the HTML case report as a proper dossier document (#18).
- Entity images now have EXIF metadata (GPS coordinates, camera info,
  timestamp) automatically extracted on upload (#21).

#### Fixed
- UTC datestamp and entity-name rendering in the HTML report export.

### [v0.1.6] - 2026-08-24

#### Added
- Entity document upload (#10): attach arbitrary files to an entity, not
  just images.
- Entity hide/unhide (#9).

### [v0.1.5] - 2026-08-22

#### Added
- Entity image upload (phase one): manual attachment via the GUI.
- `entity split` lets the caller choose the new entity's type and
  canonical key, both in the CLI (`--type`) and the GUI (#2 — it
  previously always inherited the source entity's own type, so
  splitting a username fact off a `Person` produced another `Person`
  instead of a `Username`).

#### Fixed
- `username-search` plugin: bounded concurrent site checks (an
  unbounded fan-out could open far more sockets at once than intended
  on a large `sites.toml`).

### [v0.1.4] - 2026-08-21

#### Added
- `crypto-wallet` plugin: Blockstream balance/transaction-count lookup,
  self-merging onto the scanned wallet entity.
- `domain-lookup` plugin: RDAP domain registration lookup.

### [v0.1.3] - 2026-08-21

#### Fixed
- Explicitly-named plugins (`scan run --plugin <name>`) now still get
  compatibility-checked against the target entity type, instead of
  skipping the check that every other, auto-discovered plugin gets.
- Overview stats (GUI) went stale after adding a fact or relationship
  without a full screen refresh.
- Settings > Updates' "Installed version" now reads the real running
  version instead of a placeholder.

### [v0.1.2] - 2026-08-20

#### Added
- `ip-lookup` plugin: geolocates an `IPAddress` via ip-api.com, emitting
  both a `Location` and an `Organization` from one lookup.

### [v0.1.1] - 2026-08-20

#### Added
- `email-lookup` plugin: checks Gravatar/Libravatar for a registered
  avatar (MD5-of-email lookup, no API key) — the second real plugin
  after `username-search`.
- `eumeaus plugin sign`, used to sign both shipped plugins.

#### Fixed
- An empty `plugin.toml` `signature` field is now correctly treated as
  unsigned, rather than as an invalid signature.

### [v0.1.0] - 2026-08-19

Initial public release. SPEC.md's v1 milestones (M0–M6) complete:
case lifecycle over SQLCipher-encrypted SQLite, the entity/relationship/
provenance data model with merge/split and an append-only audit trail,
the plugin protocol/host/SDK (subprocess + gRPC, Unix socket and
Windows named pipe transports), scan orchestration, the
`username-search` proof-of-concept plugin, and OS-keychain-backed
credential management. Also resolves SPEC.md §8.1–8.7 (passphrase-based
case export/import, a local trust store for plugin signing keys, the
`CryptoWallet`/`Url` entity types, true-deletion fact redaction,
multi-case concurrency, signed evidentiary HTML reports, and the
project's legal/ToS posture) and ships one-line installers
(`install.sh`/`install.ps1`).

## GUI (`eumeaus-gui`)

### [gui-v0.1.6] - 2026-09-12

#### Added
- Map screen (#22): every entity or fact with a location now shows as a
  pin on an offline, bundled map.
- Auto-scan on entity add (#16): adding an entity can automatically kick
  off a scan with every compatible plugin. Off by default; a pulsing
  footer indicator shows while a scan runs, and the Entities screen
  live-refreshes on completion.
- Entity images now have EXIF metadata (GPS coordinates, camera info,
  timestamp) automatically extracted on upload (#21).

#### Changed
- The Graph screen now renders the new `HasSubdomain` relationship
  directionally, same as `HasAccount`/`Owns`.

### [gui-v0.1.5] - 2026-08-24

#### Added
- Entity document upload (#10).
- Entity hide/unhide (#9).

#### Fixed
- The custom titlebar wasn't actually draggable.

### [gui-v0.1.4] - 2026-08-22

#### Added
- Draggable graph nodes, with dragged positions persisted between
  sessions.
- Graph relationship lines now trim to the node edge instead of the
  center point, with directional arrowheads.
- Entity image upload (phase one): manual attachment via the GUI.
- `entity split`'s type/key picker (see the CLI's v0.1.5 entry, #2).

### [gui-v0.1.3] - 2026-08-21

#### Fixed
- Fixes from a user exploratory-test pass (`usertests/`): the Overview
  screen's Export card path field was still text-only (no native file
  picker), Overview stats went stale after adding a fact or
  relationship, and Settings > Updates' "Installed version" read a
  placeholder instead of the real running version.

### [gui-v0.1.2] - 2026-08-20

Fixes and additions from a user exploratory-test pass (`usertests/`).

#### Added
- Native file/directory pickers on every path input that used to be
  type-only (case open/create/browse, plugin install/directory, Scans'
  plugins directory).
- A persisted default plugins directory (Settings > General).
- A searchable entity combobox, replacing free-text entity id/value
  fields in the Entities screen's relationship target and the Scans
  screen's scan target — plus a relationship-type taxonomy dropdown
  with a "Custom..." escape hatch.
- Entity-detail "Add fact" button.

#### Fixed
- The Scans screen's target-type dropdown was a stale 4-entry hardcoded
  list missing `IPAddress` (and everything added since) — this is what
  let the new `ip-lookup` plugin actually be scanned from the GUI at
  all.
- `entityStyle.ts` used the key `IpAddress` where `EntityType::IpAddress`
  actually serializes as `IPAddress`, silently routing GUI-created IP
  address entities through the generic `Custom(...)` styling instead of
  their real type.

### [gui-v0.1.1] - 2026-08-20

#### Added
- The Claude Design UX redesign: replaced G0–G6's flat forms with real
  sidebar screens (Overview/Entities/Graph/Scans/Plugins/Settings).
- Signed report export, with signature verification.

### [gui-v0.1.0] - 2026-08-20

Initial GUI release: Tauri 2.x + React/TS, Linux and Windows only.
Case lifecycle (create/open/close/list), read-only entity/fact
browsing, scan run with live per-plugin progress, the entity/
relationship write path, plugin/credential/trust management, and
packaging + auto-update (G0–G6).
