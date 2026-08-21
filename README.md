# Eumeaus

A local-first, plugin-extensible OSINT case management tool for investigators.
See [`SPEC.md`](./SPEC.md) for the full design, [`CLI.md`](./CLI.md) for
command reference and usage examples, and
[`plugin-developer-guide.md`](./plugin-developer-guide.md) if you want to
write your own plugin.

**Status:** v1 complete — all of `SPEC.md` §7's milestones (M0–M6) are
done, including the full v1 proof (`SPEC.md` §6). Case lifecycle over real
SQLCipher (M1); manual entity/relationship CRUD, merge/split, and audit
trail via the CLI (M2); plugin manifest validation and real subprocess/gRPC
spawn, handshake, invoke, and timeout handling (M3); scan orchestration —
worker pool, rate limiting, crash-safe resumability, result auto-merge —
wired end to end through `scan run`/`status`/`resume` (M4); a real
Sherlock-equivalent proof-of-concept plugin
(`eumeaus-username-search-plugin`), signed, checking real sites over real
HTTP (M5); and OS-keychain-backed credential storage, injected into a
plugin's request only — never the case file, subprocess argv, or
environment (M6).

## Installation

Prebuilt releases (Linux x86_64, Windows x86_64) ship via GitHub Releases.
Install with one line:

```sh
# Linux
curl -fsSL https://raw.githubusercontent.com/RedRockerSE/eumeaus/main/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/RedRockerSE/eumeaus/main/install.ps1 | iex
```

Both scripts verify a SHA-256 checksum against the release before
installing, and also install the bundled `username-search`,
`email-lookup`, `ip-lookup`, and `domain-lookup` plugins ready to use
with `scan run --plugins-dir`. See
[`install.sh`](./install.sh)/[`install.ps1`](./install.ps1) for exactly
what they do — nothing hidden, no piping to a shell you haven't read.

No release yet for your platform, or you'd rather build it yourself? See
"Commands" below — `cargo build --release` produces the same binary the
release workflow ships.

## Workspace layout

- `crates/eumeaus-engine` — case lifecycle, entity/relationship/provenance data model, scan orchestration.
- `crates/eumeaus-plugin-host` — plugin discovery, manifest validation, subprocess/gRPC lifecycle.
- `crates/eumeaus-plugin-protocol` — the engine↔plugin wire contract (`plugin.proto`).
- `crates/eumeaus-plugin-sdk` — helper library for plugin authors.
- `crates/eumeaus-username-search-plugin` — the real v1 proof-of-concept
  plugin: a small Sherlock-equivalent username checker.
- `crates/eumeaus-email-lookup-plugin` — a second real plugin: checks
  whether an email address has a registered Gravatar/Libravatar avatar.
- `crates/eumeaus-ip-lookup-plugin` — a third real plugin: geolocates an
  IP address (city/region/country, ISP/org) via ip-api.com.
- `crates/eumeaus-domain-lookup-plugin` — a fourth real plugin: looks up
  a domain's registration data (registrar, dates, nameservers) via RDAP.
- `crates/eumeaus-cli` — the v1 user-facing CLI, and the end-to-end test surface.

## Commands

```sh
cargo build --workspace              # build everything
cargo run -p eumeaus-cli -- --help   # run the CLI
cargo test --workspace               # unit + e2e tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Pre-commit hooks (`cargo fmt --check`, `cargo clippy`) are configured via
[pre-commit](https://pre-commit.com/): `pip install pre-commit && pre-commit install`.

## License

Licensed under either of [Apache License, Version 2.0](./LICENSE-APACHE) or
[MIT license](./LICENSE-MIT) at your option.
