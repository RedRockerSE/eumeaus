# Eumeaus

A local-first, plugin-extensible OSINT case management tool for investigators.
See [`SPEC.md`](./SPEC.md) for the full design.

**Status:** M0–M4 done. Case lifecycle over real SQLCipher (M1); manual
entity/relationship CRUD, merge/split, and audit trail via the CLI (M2);
plugin manifest validation and real subprocess/gRPC spawn, handshake,
invoke, and timeout handling (M3); and scan orchestration — worker pool,
rate limiting, crash-safe resumability, result auto-merge — wired end to
end through `scan run`/`status`/`resume` (M4). Credentials are still a stub
returning `NotImplemented` — see `SPEC.md` §7 for the milestone order.

## Workspace layout

- `crates/eumeaus-engine` — case lifecycle, entity/relationship/provenance data model, scan orchestration.
- `crates/eumeaus-plugin-host` — plugin discovery, manifest validation, subprocess/gRPC lifecycle.
- `crates/eumeaus-plugin-protocol` — the engine↔plugin wire contract (`plugin.proto`).
- `crates/eumeaus-plugin-sdk` — helper library for plugin authors.
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
