# eumeaus-phone-lookup-plugin-python

A proof-of-concept Eumeaus plugin written **entirely in Python, with no
Rust and no `eumeaus-plugin-sdk`** — implementing the wire protocol
(handshake + gRPC service) directly, exactly the way
[`plugin-developer-guide.md`](../plugin-developer-guide.md) §3 describes
for a non-Rust plugin author. It exists to prove the protocol really is
language-agnostic, not just document it.

It lives outside `crates/` on purpose: `crates/` is this repository's
Cargo workspace, and this isn't a Cargo crate — there's nothing here for
`cargo build --workspace`/`cargo test --workspace` to pick up, and that's
intentional.

## What it does

Takes a `PhoneNumber` entity and looks it up against
[`phonenumbers`](https://pypi.org/project/phonenumbers/) (a pure-Python
port of Google's libphonenumber — the same reference data Android and
Chromium ship with), **entirely offline, no network call, no API key**:

- Validity (a well-formed-but-impossible number comes back `NOT_FOUND`;
  garbage input comes back `ERROR`, not a crash).
- Self-enrichment onto the scanned entity itself: E.164 form, line type
  (mobile/fixed line/VOIP/...), region code — the same
  same-`canonical_key`-as-input self-merge pattern
  `eumeaus-crypto-wallet-plugin` uses.
- A related `Location` entity (region + human-readable description +
  timezone(s)), linked with a `LocatedAt` relationship.
- A related `Organization` entity when libphonenumber's bundled data can
  attribute the number to a carrier, linked with an `AssociatedWith`
  relationship — the same "one call, multiple entity types" pattern
  `eumeaus-ip-lookup-plugin` uses.

`plugin.toml` sets `permissions.network = false`, honestly — this plugin
never makes an HTTP request. That's a deliberate choice for a
proof-of-concept: it makes every result 100% reproducible and testable
with zero flakiness, while still doing genuine, useful OSINT enrichment
(this is the same data source real phone-OSINT tools query offline for
exactly this reason).

**Platform:** Unix domain sockets only (Linux/macOS). Windows named-pipe
support (SPEC.md §2.2's other transport) is out of scope for this PoC —
see `plugin.py`'s `main()`, which refuses to start on `os.name != "posix"`
rather than silently misbehaving.

## Requirements

- Python 3.9+ (tested against 3.14).
- `pip` (to install the two runtime dependencies below into a venv).
- The `eumeaus` CLI itself, built from this repository
  (`cargo build -p eumeaus-cli`), to actually run a scan against it.

Runtime dependencies (`requirements.txt`): `grpcio`, `protobuf`,
`phonenumbers`. Nothing else — in particular, **not** `grpcio-tools`
(that's a one-time codegen dependency, see "Regenerating the protobuf
code" below, not something the running plugin needs).

## Setup

```console
$ cd eumeaus-phone-lookup-plugin-python
$ python3 -m venv .venv
$ .venv/bin/pip install -r requirements.txt
```

That's it — `pb/plugin_pb2.py` and `pb/plugin_pb2_grpc.py` (generated
from `../crates/eumeaus-plugin-protocol/plugin.proto`) are already
checked in, so no `protoc`/`grpcio-tools` is needed just to run the
plugin.

## Running it for real, against the actual engine

`eumeaus-plugin-host` spawns whatever `[execution] entrypoint` names in
`plugin.toml` — here, `./run`, a tiny shell wrapper (not `plugin.py`
directly) that `exec`s this directory's own `.venv/bin/python3` by
absolute path. That matters: the host inherits its own process's `PATH`,
which has no guaranteed relationship to whichever `python3` has this
plugin's dependencies installed, and a `#!/usr/bin/env python3` shebang
on `plugin.py` itself would silently pick up the wrong one. Build the
venv *before* installing (see below) or the wrapper has nothing to exec.

`eumeaus-plugin-host`'s discovery (`PluginHost::discover`) does **not**
follow symlinks (`DirEntry::file_type()` doesn't traverse them) — a
`plugins/phone-lookup` symlinked at this directory silently shows up as
zero plugins found, not an error. Use `eumeaus plugin install` (a real
copy) instead:

```console
$ cargo build -p eumeaus-cli   # from the repo root
$ EUM=./target/debug/eumeaus

# 1. Install (copies this directory, minus anything gitignored, into
#    ./plugins/phone-lookup):
$ $EUM plugin list --plugins-dir ./plugins   # empty at first
$ $EUM plugin install ./eumeaus-phone-lookup-plugin-python --plugins-dir ./plugins
phone-lookup 0.1.0

# 2. Build the venv INSIDE the installed copy (not before installing —
#    that would just copy a large .venv/ pointlessly; do it after):
$ (cd ./plugins/phone-lookup && python3 -m venv .venv && ./.venv/bin/pip install -r requirements.txt)

$ $EUM plugin list --plugins-dir ./plugins
phone-lookup    0.1.0   unsigned  ./plugins/phone-lookup/./run

# 3. Create a scratch case, add a target, and scan it:
$ $EUM case create poc --path /tmp
$ CASE=/tmp/poc.eum
$ $EUM --case "$CASE" entity add --type PhoneNumber --key "+14155552671"
<entity-id>
$ $EUM --case "$CASE" scan run --plugins-dir ./plugins \
    --target-type PhoneNumber --target-value "+14155552671"
<scan-id>
$ $EUM --case "$CASE" scan status <scan-id>
COMPLETED
$ $EUM --case "$CASE" entity list
<entity-id>     PhoneNumber     +14155552671    +14155552671
<entity-id-2>   Location        us              San Francisco, CA
$ $EUM --case "$CASE" entity show <entity-id>
```

This exact sequence was run against a real build of this repository's
`eumeaus-cli` while writing this plugin — real handshake, real Unix
socket, real gRPC `Check` call, real merge into the case graph. If your
plugin ever hangs, crashed, or errored, `scan status`/`entity show`
reports it, and the plugin's stderr is inherited by the host (visible
directly in your terminal), same as any other plugin —
`plugin-developer-guide.md` §9 covers this in more depth.

## Testing without the engine at all

`check()` — the actual collection logic — is a plain function taking a
`CheckRequest` and returning a list of `CheckResult`s, independent of the
gRPC server plumbing around it. Test it directly, no subprocess, no
socket, no `eumeaus-plugin-host`:

```console
$ .venv/bin/pip install -r requirements-dev.txt   # adds pytest
$ .venv/bin/python -m pytest tests/ -v
```

Covers: a valid number (self-merge attributes + `Location`), a number
libphonenumber's data attributes to a named carrier (`Organization`), a
well-formed-but-impossible number (`NOT_FOUND`), unparseable input
(`ERROR`, not a crash), and that every result carries `Provenance`.

You can also exercise the handshake/subprocess layer directly, exactly
like `eumeaus-plugin-host` does, without the CLI at all:

```console
$ WORKDIR=$(mktemp -d)
$ EUMEAUS_PLUGIN_DIR="$WORKDIR" ./run &
$ # stdout prints: EUMEAUS-PLUGIN|1|unix|<WORKDIR>/plugin.sock|grpc
$ ls "$WORKDIR"   # plugin.sock should exist
$ kill %1
```

## Regenerating the protobuf code

Only needed after `../crates/eumeaus-plugin-protocol/plugin.proto`
itself changes upstream — `pb/plugin_pb2.py`/`pb/plugin_pb2_grpc.py` are
checked in precisely so a normal install never needs `protoc` at all.

```console
$ .venv/bin/pip install grpcio-tools   # not in requirements*.txt — see below
$ ./regenerate_proto.sh
```

`grpcio-tools` bundles its own vendored `protoc` (same reasoning as this
repo's Rust side using `protoc-bin-vendored`, per CLAUDE.md's Gotchas)
and is deliberately excluded from `requirements.txt`/`requirements-dev.txt`
— it's a one-off maintenance action, not something every install or test
run needs to pull in.

## Layout

| Path | What |
|---|---|
| `plugin.toml` | The manifest — see `plugin-developer-guide.md` §4 for every field. |
| `run` | The `[execution] entrypoint` — resolves its own real path and `exec`s `.venv/bin/python3 plugin.py`. |
| `plugin.py` | Everything: handshake, gRPC server, `check()` collection logic. Deliberately kept in one file — this is a PoC, not a template for a larger plugin. |
| `pb/` | Generated from `plugin.proto` — checked in, not hand-written. |
| `requirements.txt` / `requirements-dev.txt` | Runtime deps / runtime+test deps. |
| `regenerate_proto.sh` | Maintenance-only: regenerates `pb/` from the canonical `.proto`. |
| `tests/test_plugin.py` | Unit tests for `check()`, no running server needed. |

## Why not use the Rust SDK?

Because the entire point of this plugin is to prove the alternative
works: `eumeaus-plugin-sdk` exists to make the *Rust* path convenient,
but SPEC.md §2.4 and `plugin-developer-guide.md` §3 both promise the
protocol itself doesn't require Rust at all. This plugin backs that
promise with a real, working, tested example instead of leaving it as an
unverified claim in a doc.
