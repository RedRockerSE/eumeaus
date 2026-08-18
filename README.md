# Eumeaus

A local-first, plugin-extensible OSINT case management tool for investigators.
See [`SPEC.md`](./SPEC.md) for the full design.

**Status:** M0–M2 done. Case lifecycle over real SQLCipher (M1), plus manual
entity/relationship CRUD, merge/split, and audit trail via the CLI (M2).
Plugins and scans are still stubs returning `NotImplemented` — see
`SPEC.md` §7 for the milestone order.

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
