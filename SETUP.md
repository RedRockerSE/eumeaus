# Dev environment setup

Building Eumeaus from source (not just installing the shipped binary —
see `README.md`'s Installation section for that). Ubuntu/Debian commands
below; adjust package names for other distros. Windows dev is supported
too (CI builds it) but isn't covered here — the CLI cross-compiles fine
with just `rustup`, and the GUI needs Visual Studio's C++ build tools for
Tauri's own Windows toolchain (see Tauri's own prerequisites docs).

## 1. Rust

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

No pinned toolchain version — CI uses `stable`, so `rustup default stable`
is enough. `rustup component add rustfmt clippy` if they're not already
in by default.

## 2. Node.js (only needed for the GUI, `crates/eumeaus-gui`)

Node 20+ (matches CI's `setup-node`). Any install method works —
[nvm](https://github.com/nvm-sh/nvm), your distro's package, whatever.

## 3. GUI system dependencies (Linux only)

`crates/eumeaus-gui`'s Rust side (`src-tauri`) won't even `cargo check`
without these — this is the host's own native linking, not something a
target flag or vendored crate can work around:

```sh
sudo apt-get update
sudo apt-get install -y libwebkit2gtk-4.1-dev libsoup-3.0-dev \
  libjavascriptcoregtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev libxdo-dev
```

(`libwebkit2gtk-4.1-dev` pulls in most of the rest via apt, but not all —
see CLAUDE.md's Gotchas for the full story.)

## 4. OS keychain / Secret Service

Any test that calls `Case::create`/`Case::open` stores or reads a real
encryption key in the OS keychain (`cargo test --workspace` will hit
this). A normal desktop session (GNOME, KDE, etc.) already has one
running. Headless/SSH with no keyring daemon: `cargo test` will hang or
fail with `EngineError::Keychain` — CI works around this by starting
`gnome-keyring-daemon` explicitly (see `.github/workflows/ci.yml`), and a
headless dev box would need the same.

## 5. `gh` (GitHub CLI)

Only needed for PRs/releases, not for building. Install per
[cli.github.com](https://cli.github.com/), then `gh auth login`.

## 6. Build and test

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Pre-commit hooks for the last two: see README.md's Commands section.

## 7. Run the GUI in dev mode

```sh
cd crates/eumeaus-gui
npm install
npm run tauri dev
```

First `npm install` may warn about `esbuild`'s postinstall script needing
approval (`npm warn allow-scripts`) — harmless, just an npm supply-chain
guard; `npm run tauri dev` still works without acting on it.

## 8. (Optional) Reproducing the release build locally

The CLI's release workflow targets `x86_64-unknown-linux-musl` for a
fully static binary. Only needed if you're touching `.github/workflows/
release.yml` itself:

```sh
rustup target add x86_64-unknown-linux-musl
sudo apt-get install -y musl-tools
cargo build --release --target x86_64-unknown-linux-musl --bin eumeaus
```
