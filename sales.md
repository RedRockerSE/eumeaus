# Eumeaus

### A trustworthy, extensible case management platform for professional OSINT work

*Pitch deck / one-pager draft — for buyer and investor conversations.*

---

## The one-line pitch

Eumeaus is the first OSINT case management tool built like evidence software, not
scraper software: local-first, provenance-tracked, cryptographically signed, and
extensible by design — so investigators can trust what it collects, and defend
it later.

## The problem

Every professional investigator — law enforcement digital forensics units,
private investigators, investigative journalists, corporate fraud and threat
intelligence teams, litigation support — eventually needs to collect and
structure open-source intelligence: people, usernames, domains, phone numbers,
accounts, and the relationships between them.

Today's options are a bad trade-off:

- **Free scrapers (Sherlock, Recon-ng, SpiderFoot, and friends)** are collection
  scripts, not case management. No persistent case model, no provenance, no
  audit trail, nothing that survives contact with opposing counsel or an
  internal-affairs review.
- **Heavyweight enterprise platforms (Maltego, i2 Analyst's Notebook, Palantir
  Gotham)** are built for large intelligence organizations — expensive,
  server-centric, and closed. A solo investigator or a small firm can't
  extend them, audit them, or afford them.
- **Nothing in between** treats *findings integrity* as a first-class design
  constraint. If a finding can't be traced back to what was collected, when,
  how, and by which tool version, it's a liability the moment it matters.

## The solution

Eumeaus is a local-first, plugin-extensible case management tool purpose-built
for OSINT work, with three design pillars that follow directly from what
professional use actually requires:

**Extensible.** New collection techniques ship as plugins — isolated
subprocesses talking a versioned gRPC contract, not code bolted onto the
core. A plugin can be written in any language. The core ships with a real,
working username-enumeration plugin (a Sherlock-equivalent) today, proving
the model end to end rather than as a diagram.

**Trustworthy.** Every fact carries provenance (source, retrieval method,
a SHA-256 of the raw response, a timestamp) and facts are append-only —
corrections are new facts, never silent overwrites, so the record never
loses history. Plugins are Ed25519-signed and verified before they run.
Case files are SQLCipher-encrypted at rest. Exported reports can be
signed and independently verified, supporting a real chain of custody.

**Operationally safe.** Rate limiting, proxy support, and a resumable
scan engine mean the tool doesn't recklessly hammer third-party sites or
lose partial work if it's interrupted — and it never silently guesses:
a rate-limited or blocked lookup is recorded as *uncertain*, distinct
from *not found*, so the investigator never mistakes "the site blocked us"
for "the answer is no."

None of this is a roadmap slide. It's shipped: a public v1 CLI release
(installable in one line on Linux and Windows), a desktop GUI with the
same guarantees, and a real proof-of-concept plugin doing real HTTP
lookups against real sites — see **Traction**, below.

## Why now

OSINT has gone from a niche investigative skill to a standard part of
digital forensics, journalism, fraud investigation, and litigation
support — and the tooling hasn't caught up. Evidentiary standards
(chain of custody, tamper-evidence, defensible provenance) that have
existed in traditional forensics for decades are still an afterthought
in most OSINT tooling. A tool that treats them as the foundation, not a
feature, is a genuine gap in the market rather than an incremental
improvement on an existing category.

## Product architecture, in brief

| Layer | What it does |
|---|---|
| **Engine** (Rust, embeddable library) | Case lifecycle, entity/relationship/provenance data model, entity resolution, scan orchestration. The "brain" — everything else is a client of it. |
| **Plugin protocol** (gRPC, versioned) | Language-agnostic wire contract between the engine and plugin subprocesses — a plugin runs isolated, sandboxed by the OS process boundary, not trusted code linked into the core. |
| **CLI** | The v1 user-facing surface — every capability of the engine, scriptable, automatable. |
| **Desktop GUI** (Tauri, Windows + Linux) | The same engine, a polished investigator-facing UI: case overview, entity/graph browsing, live scan monitoring, plugin/credential management, signed report export. |

No client-server architecture, no cloud dependency, no central point of
compromise or subpoena — case data never leaves the investigator's
machine unless they choose to export it.

## Differentiation

| | Sherlock-style scripts | Maltego / i2 / Gotham | **Eumeaus** |
|---|---|---|---|
| Persistent, structured case model | ✗ | ✓ | ✓ |
| Provenance on every finding | ✗ | partial | ✓ (source, method, hash, timestamp) |
| Signed, verifiable plugins | ✗ | ✗ (closed) | ✓ (Ed25519, local trust store) |
| Append-only fact history, audit trail | ✗ | partial | ✓ |
| Extensible by third parties | scripts only | limited/paid SDKs | ✓ (open protocol, any language) |
| Local-first, no server/cloud | ✓ | ✗ (usually) | ✓ |
| Price point accessible to solo/small teams | free (but not a product) | enterprise-only | open core |

## Buyers

- **Law enforcement digital forensics and cybercrime units** — need
  defensible, auditable collection with a real chain of custody, at a
  price point below enterprise intelligence platforms.
- **Private investigation and corporate security firms** — need to
  scale investigator output without scaling headcount, via automation
  (scans, plugins) that doesn't sacrifice rigor.
- **Investigative journalism newsrooms** — need provenance for editorial
  and legal defensibility, on a budget that rules out enterprise tooling.
- **Litigation support / eDiscovery teams** — need exportable, signed
  reports that hold up to opposing scrutiny.

## Business model

The core (engine, CLI, plugin protocol/SDK, one reference plugin) is
open source (MIT/Apache-2.0 dual-licensed) — deliberately, to build
trust and a plugin ecosystem the way successful developer-facing
platforms do. Monetization is open-core, layered on top:

- **Professional/enterprise desktop tier** — the signed GUI build,
  priority support, training, and onboarding for firms and agencies.
- **Verified plugin marketplace** (planned, currently a deliberate
  v1 non-goal) — a curated, signed catalog of collection plugins beyond
  the single reference implementation, with revenue share for third-party
  plugin authors.
- **Custom plugin development & integration services** — for agencies
  with proprietary data sources or internal systems to connect.
- **Compliance/evidentiary packages** — signed export + verification
  tooling tailored to specific jurisdictions' evidentiary requirements.

*(Placeholder for a real pitch: pricing tiers, unit economics, and a
funding ask belong here once there are real numbers to put behind them —
intentionally left out rather than fabricated.)*

## Traction

Everything below is a real, independently verifiable engineering
milestone, not a projection:

- **v1 CLI shipped and publicly released** (`v0.1.0`) — installable in
  one line on Linux and Windows, with checksum-verified installers.
  Full command surface: case lifecycle, entity/relationship CRUD,
  scan orchestration, plugin management, credential storage, audit
  trail, signed report export/verify.
- **A real proof-of-concept plugin, not a mock** — a Sherlock-equivalent
  username checker doing genuine HTTP lookups against live sites
  (GitHub, GitLab), proving the subprocess/gRPC/signing pipeline
  end to end, not just on paper.
- **Desktop GUI shipped** (`gui-v0.1.1`), Windows + Linux, built on
  the identical engine — case overview, entity/relationship graph,
  live scan monitoring, plugin/credential/trust management, and
  signed report export, all wired to real data with zero mock UI.
- **Auto-update mechanism verified live, end to end** — not just wired
  up: a real running instance detected a new release, downloaded it,
  cryptographically verified the update artifact's signature, installed
  it, and relaunched into the new version.
- **Security model implemented, not aspirational**: SQLCipher-encrypted
  case files, OS-keychain-backed credential storage (never touching
  disk, subprocess argv, or environment variables in plaintext),
  Ed25519 plugin signing and a local trust store, append-only fact
  history with a hard audit trail for merges/splits.
- **A second real plugin shipped** — email lookup (Gravatar/Libravatar
  avatar checks), live-verified against production servers, proving the
  plugin model generalizes beyond username enumeration on the first try.

## Roadmap

1. **Third-party plugin ecosystem** — publish the developer guide,
   recruit early plugin authors, stand up a signed-plugin catalog.
2. **Windows code signing** — remove the SmartScreen warning path for
   enterprise buyers who require signed installers.
3. **macOS support** — currently deferred, revisit once there's
   committed buyer demand.
4. **Enterprise/compliance packages** — jurisdiction-specific
   evidentiary export tooling, sold alongside the open-core product.

## The ask

*(To be filled in for a specific conversation: seed capital to fund
plugin-ecosystem growth and enterprise go-to-market, a design partner
relationship with a law enforcement or PI firm for a paid pilot, or a
direct enterprise license — whichever fits the audience this document
is actually being used with.)*

## Risks, named honestly

- **Pre-revenue, single-plugin ecosystem today.** The core product is
  proven; the ecosystem that makes it valuable at scale is not built
  yet. This is the current focus, not a hidden gap.
- **Legal/compliance responsibility stays with the investigator.**
  Eumeaus provides mechanisms (rate limiting, audit trail, signed
  provenance) but doesn't itself determine whether a given collection
  activity is lawful in a given jurisdiction — a deliberate scope
  boundary, not an oversight, and one every credible buyer will expect.
- **Windows code signing is unresolved** — a real Authenticode
  certificate has a real ongoing cost, not yet acquired. Doesn't block
  adoption today; does affect first impression for security-conscious
  enterprise buyers until resolved.
