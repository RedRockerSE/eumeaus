# Eumeaus CLI

Command reference and usage examples for `eumeaus`, the v1 user interface
described in [`SPEC.md`](./SPEC.md) §3.4. For build/dev commands see
[`README.md`](./README.md); for internals see [`CLAUDE.md`](./CLAUDE.md).

Every example below was run against the real binary while writing this
document.

## Building and running

```sh
cargo build --workspace
./target/debug/eumeaus --help
```

(`cargo build --release` for `./target/release/eumeaus` instead.)

## Concepts

- **Case file** — a single `.eum` file: a SQLCipher-encrypted SQLite
  database. Its encryption key lives in your OS keychain, not in the file
  or anywhere on disk in plaintext, so `case open` (implicit in every
  command below) only works on the machine — and OS user account — that
  created it.
- **`--case <path>`** is a global flag: it can go before or after the
  subcommand (`eumeaus --case demo.eum entity list` and `eumeaus entity
  list --case demo.eum` are equivalent) and is required by any command that
  touches case data. Commands that don't (`case create`, `credential *`)
  don't need it.
- **Entities and relationships** have no privileged "root" type — a
  person, a username, a domain are all just entities, connected by typed
  relationships. See [Entity and relationship types](#entity-and-relationship-types)
  below for the taxonomy.
- **Plugins** are discovered from a directory you point `scan run` at
  (`--plugins-dir`, default `./plugins`), one subdirectory per plugin, each
  containing a `plugin.toml` manifest (SPEC.md §3.3) and its executable.

## Command reference

### `case`

```
eumeaus case create <name> [--path <dir>]
eumeaus case open <path>
eumeaus case list [--path <dir>]
eumeaus case export <path> --out <file> [--format sqlite|report]
```

`create` makes `<dir>/<name>.eum` (default `--path .`) and prints nothing
on success. `open` just verifies the case can be decrypted — most of the
time you don't call it directly, since every other command that needs a
case opens (and closes) it for that one invocation.

`list` scans a directory (default `.`) for `.eum` files and prints
`id  name  path` for each — it never opens or decrypts any of them (the
name comes from the filename, the id from the plaintext `.eum.meta`
sidecar), so it works even without keychain access.

`export --format sqlite` produces a **plaintext, unencrypted** SQLite copy
of the whole case via SQLCipher's own `sqlcipher_export()` — readable by
plain `sqlite3`, with no key. This is not a portability/handoff encryption
scheme (SPEC.md §8's open question 1 is still open) — treat the output
file as sensitive, same as you would the decrypted data itself.
`--format report` instead writes a human-readable JSON dump of every
entity and relationship, their attributes, and their audit trail — a
minimal stand-in for §8's open question 6 (evidentiary report format), not
a signed PDF/JSON bundle. Both refuse to overwrite an existing `--out`.

```console
$ eumeaus case list --path .
471698ea-b4a2-4b6d-98ef-76631dba8a75	acme-investigation	./acme-investigation.eum
$ eumeaus case export acme-investigation.eum --out acme.sqlite --format sqlite
$ python3 -c "import sqlite3; print(list(sqlite3.connect('acme.sqlite').execute('select entity_type, canonical_key from entities')))"
[('Username', 'jdoe123'), ('Person', 'john doe'), ('Person', None)]
$ eumeaus case export acme-investigation.eum --out acme-report.json --format report
```

### `entity`

```
eumeaus entity add --type <Type> [--key <value>] [--attr k=v ...]
eumeaus entity list [--type <Type>]
eumeaus entity show <id>
eumeaus entity merge <id1> <id2>
eumeaus entity split <id> --facts <fact-id,...>
```

(`entity list` also accepts `--filter <expr>` — SPEC.md §3.4 shows it, but
it's currently a no-op; only `--type` actually filters.)

- `add` prints the new (or matched-and-merged-into) entity's id.
  `--key`, normalized (trimmed, lowercased), is what dedupes: adding the
  same `--type`/`--key` again doesn't create a duplicate, it appends a new
  fact to the existing entity — repeat the example below with
  `--key JDOE123` and it merges into the same `jdoe123` entity.
- `list` prints one tab-separated row per entity: `id  type  canonical_key  display_label`.
- `show` prints the entity plus every attribute fact recorded on it, each
  tagged with the fact id that produced it, `*` marking the current value
  per key and flagging conflicting ones (SPEC.md §4.4) — see
  [Attribute conflicts](#attribute-conflicts).
- `merge id1 id2` absorbs `id2` into `id1` and prints the survivor's id
  (always `id1`). Recorded as an audit event — see `audit show`.
- `split` needs fact ids — get them from `entity show`'s `(fact: ...)`
  column.

```console
$ eumeaus case create acme-investigation --path .
$ eumeaus --case acme-investigation.eum entity add --type Username --key jdoe123
299a704e-bc40-40c5-be5c-99f91f110f58
$ eumeaus --case acme-investigation.eum entity add --type Person --key "John Doe" \
    --attr full_name="John Doe" --attr nationality=US
f8bac901-2693-45ed-ad4e-f1cf8028b195
$ eumeaus --case acme-investigation.eum entity list
299a704e-bc40-40c5-be5c-99f91f110f58   Username  jdoe123    jdoe123
f8bac901-2693-45ed-ad4e-f1cf8028b195   Person    john doe   John Doe
```

A second, separate `entity add` on the same key (e.g. `nationality` added
in a later session) creates its own fact, independent of the first —
`entity show` lists each attribute with the fact id that produced it,
which `entity split --facts` then consumes directly:

```console
$ eumeaus --case acme-investigation.eum entity add --type Person --key "John Doe" --attr nationality=US
f8bac901-2693-45ed-ad4e-f1cf8028b195
$ eumeaus --case acme-investigation.eum entity show f8bac901-2693-45ed-ad4e-f1cf8028b195
id:            f8bac901-2693-45ed-ad4e-f1cf8028b195
type:          Person
canonical_key: john doe
label:         John Doe
attributes:
  * full_name = John Doe (fact: f883cb77-08cf-4db6-872a-ca164bb930c7, source: user, collected_at: 1787114078998)
  * nationality = US (fact: 52bd0def-fe7a-45d9-8bcf-372da4e8b7a1, source: user, collected_at: 1787114079057)
$ eumeaus --case acme-investigation.eum entity split f8bac901-2693-45ed-ad4e-f1cf8028b195 \
    --facts 52bd0def-fe7a-45d9-8bcf-372da4e8b7a1
b20ff1f2-d961-4839-88bf-4990059f3c4e
```

(Attributes added together in the *same* `entity add` call share one fact,
so splitting by fact id moves them as a group — split works at fact
granularity, not per-attribute.)

### `relationship`

```
eumeaus relationship add --from <id> --to <id> --type <Type> [--attr k=v ...]
```

Prints the new relationship's id.

```console
$ eumeaus --case acme-investigation.eum relationship add \
    --from f8bac901-2693-45ed-ad4e-f1cf8028b195 \
    --to   299a704e-bc40-40c5-be5c-99f91f110f58 \
    --type HasAccount --attr verified=true
9870690d-cbf4-4f84-8645-b919f8ccdfc6
```

### `scan`

```
eumeaus scan run --target-type <Type> --target-value <value>
                  [--plugin <name>] [--plugins-dir <dir>]
                  [--rate-limit N] [--proxy <url>] [--worker-pool N]
                  [--trusted-key <hex>]
eumeaus scan status <scan-id>
eumeaus scan resume <scan-id>
eumeaus scan list
```

`--target-type`/`--target-value` name an **existing** entity by its
canonical key — add it with `entity add` first, or `scan run` refuses with
a clear error. `--plugin` runs one named plugin; omit it to run every
discovered plugin compatible with `--target-type`, up to `--worker-pool`
of them concurrently (default 4).

`run` prints the scan id immediately (before the scan itself, which can
take a while, runs) and again from `status`/`resume`. If the process gets
killed mid-scan, `scan resume <id>` continues it — only plugins that
hadn't finished re-run; already-succeeded ones aren't repeated.

A **plugin directory** (`--plugins-dir`, default `./plugins`) holds one
subdirectory per plugin:

```
plugins/
  username-search/
    plugin.toml
```

```toml
[plugin]
name = "username-search"
version = "0.1.0"
description = "Checks username existence across social platforms"
author = "Eumeaus Core Team"
# signature = "..."   # see --trusted-key below

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
requested_credentials = []          # see `credential`, below

[execution]
entrypoint = "/absolute/or/./relative/path/to/eumeaus-username-search-plugin"
default_rate_limit_per_sec = 5
default_timeout_ms = 8000
```

`eumeaus-username-search-plugin`, this project's real plugin, is built
right alongside the CLI (`cargo build --workspace` produces
`target/debug/eumeaus-username-search-plugin`).

**Configuring which sites `username-search` checks.** The site list isn't
hardcoded — drop a `sites.toml` next to `username-search`'s `plugin.toml`
to add or remove checks without a rebuild:

```
plugins/
  username-search/
    plugin.toml
    sites.toml   # optional — falls back to a built-in github/gitlab
                 # default list if absent or invalid
```

```toml
[[sites]]
slug = "github"
display_name = "GitHub"
base_url = "https://github.com"
path_template = "/{username}"
detection = "status_code"      # 200 = found, 404 = not found

[[sites]]
slug = "some-forum"
display_name = "Some Forum"
base_url = "https://forum.example.com"
path_template = "/u/{username}"
detection = "body_marker"      # always 200; page text decides found/not
not_found_marker = "Profile not found"
```

A malformed `sites.toml` doesn't fail the scan — it prints a warning and
falls back to the built-in default list:

```console
$ eumeaus --case demo.eum scan run --plugins-dir plugins --target-type Username --target-value torvalds
warning: /path/to/plugins/username-search/sites.toml is invalid (invalid TOML in ...); falling back to the built-in site list
4b3229c7-da49-4ad5-89e8-40b3936a50a4
```

**Trust note:** a plugin's `--trusted-key` signature covers its
name/version/binary hash only, never `sites.toml` — editing this file
doesn't invalidate an otherwise-signed plugin's signature. That's by
design (it's meant to be freely user-editable), just worth knowing when
reasoning about what a signature actually attests to.

```console
$ eumeaus --case acme-investigation.eum entity add --type Username --key torvalds
e13fee0c-7518-4a32-9b90-c0da8e753c4e
$ eumeaus --case acme-investigation.eum scan run \
    --plugins-dir ./plugins --target-type Username --target-value torvalds
eb3b3abd-edf2-4f51-863b-ac4692e7d58b
$ eumeaus --case acme-investigation.eum scan status eb3b3abd-edf2-4f51-863b-ac4692e7d58b
COMPLETED
$ eumeaus --case acme-investigation.eum entity list
...
e13fee0c-7518-4a32-9b90-c0da8e753c4e   Username       torvalds          torvalds
47e448b1-2975-4071-baf8-da512b5e5d77   OnlineAccount  github:torvalds   torvalds on GitHub
53f615e4-5740-4336-b237-16680f95ebe8   OnlineAccount  gitlab:torvalds   torvalds on GitLab
```

(This is a real, live lookup against github.com/gitlab.com — it needs
network access, and `torvalds` is chosen because those accounts really
exist. A username that comes back "not found" or rate-limited on a given
site produces no `OnlineAccount` row for that site at all, by design:
absence in the graph *is* the negative/uncertain result.)

`list` prints one tab-separated row per scan in the case:
`scan_id  status  target_entity_id  started_at  completed_at` (unix ms;
`-` if not yet set):

```console
$ eumeaus --case acme-investigation.eum scan list
85a374c7-1aae-455d-80ac-94bbd9ec1b1a	COMPLETED	2f5b2c01-5007-49eb-a36c-eebe3b144103	1787113952865	1787113954061
```

**Signed plugins.** By default every plugin loads unsigned. Pass
`--trusted-key <hex-encoded-32-byte-Ed25519-public-key>` to require every
plugin in that scan to carry a valid signature against it instead (refused
otherwise). There's currently no `eumeaus plugin sign` command — computing
a manifest's `signature` field means calling `eumeaus_plugin_host::sign`
programmatically (see `eumeaus-plugin-host/src/signature.rs` and any
test's `write_signed_manifest` helper for the exact pattern); wiring that
up as a CLI subcommand is a natural next step, not yet done.

### `credential`

```
eumeaus credential set <name>      # prompts interactively (real TTY required)
eumeaus credential list
eumeaus credential remove <name>
```

Global to your OS user account, not scoped to any case — no `--case`
needed. A plugin manifest's `permissions.requested_credentials` names
which of these it needs; `scan run` looks each one up and injects it into
that plugin's request. The value never touches the case file, the plugin's
command-line arguments, or its environment variables — see SPEC.md §4.5.

`set` reads the value with echo disabled, from `/dev/tty` directly (it
refuses a plain pipe on purpose, so `echo value | eumeaus credential set x`
won't work — that's intentional, not a bug).

```console
$ eumeaus credential set shodan_api_key
Value for credential "shodan_api_key":
$ eumeaus credential list
shodan_api_key
$ eumeaus credential remove shodan_api_key
$ eumeaus credential list
$
```

### `audit`

```
eumeaus audit show --entity <id> | --relationship <id> | --scan <id>
```

Exactly one of the three flags. Prints one tab-separated row per event:
`occurred_at_unix_ms  event_type  actor  event_id  description`. Currently
only entity merges/splits are recorded here — plugin-sourced facts (like
the GitHub/GitLab rows above) carry their own provenance (`entity show`)
but aren't separately audit-logged, and `--scan` isn't wired up yet.

```console
$ eumeaus --case acme-investigation.eum entity add --type Person --key "J. Doe" --attr alias=true
e41ec36e-0d03-4288-ac63-89af222589d2
$ eumeaus --case acme-investigation.eum entity merge \
    f8bac901-2693-45ed-ad4e-f1cf8028b195 e41ec36e-0d03-4288-ac63-89af222589d2
f8bac901-2693-45ed-ad4e-f1cf8028b195
$ eumeaus --case acme-investigation.eum audit show --entity f8bac901-2693-45ed-ad4e-f1cf8028b195
1787056461803  merge  user  0142f53c-1019-46b8-ad2c-b11a06cc1fcd  merged entity e41ec36e-0d03-4288-ac63-89af222589d2 into f8bac901-2693-45ed-ad4e-f1cf8028b195
```

### `plugin`

```
eumeaus plugin list [--plugins-dir <dir>] [--installed|--available]
eumeaus plugin install <path> [--plugins-dir <dir>]
eumeaus plugin verify <name> --trusted-key <hex> [--plugins-dir <dir>]
```

`--plugins-dir` defaults to `./plugins` for all three, same as `scan run`.
`--installed`/`--available` are accepted but currently no-ops — there's no
separate installed-vs-available registry (v1 non-goal: no plugin
marketplace, SPEC.md §1), so `--plugins-dir` itself *is* what's installed.

`list` prints one tab-separated row per discovered manifest:
`name  version  signed|unsigned  entrypoint`.

```console
$ eumeaus plugin list --plugins-dir plugins
username-search	0.1.0	unsigned	/home/magnus/Desktop/dev/eumeaus/target/debug/eumeaus-username-search-plugin
```

`install <path>` copies a plugin directory (containing `plugin.toml` and
its entrypoint) into `<plugins-dir>/<plugin-name>/`, and prints the
installed `name version`. Refuses to overwrite an already-installed plugin
of the same name — remove its directory first if you want to replace it.

```console
$ eumeaus plugin install ./install-src --plugins-dir plugins2
username-search 0.1.0
$ eumeaus plugin install ./install-src --plugins-dir plugins2
error: plugin "username-search" is already installed (remove its directory under --plugins-dir first)
```

`verify <name>` checks engine/protocol compatibility and, against
`--trusted-key`, the manifest's signature — same check `scan run
--trusted-key` performs before loading a plugin, run standalone. Prints
`valid` on success; refuses (same errors as `scan run`) if the plugin is
unsigned, the signature doesn't match, or `name` isn't discovered in
`--plugins-dir`. There's no `eumeaus plugin sign` command yet — computing a
manifest's `signature` field means calling `eumeaus_plugin_host::sign`
programmatically (see `eumeaus-plugin-host/src/signature.rs` and any
test's `write_signed_manifest`/`signed_manifest` helper for the exact
pattern); wiring that up as a CLI subcommand is a natural next step, not
yet done.

```console
$ eumeaus plugin verify username-search --plugins-dir plugins --trusted-key 0000000000000000000000000000000000000000000000000000000000000000
error: plugin host error: plugin username-search is unsigned; refusing to load (pass --allow-unsigned for local dev)
```

## Entity and relationship types

Starter taxonomy (SPEC.md §4.3) — any other string is accepted too (an
escape hatch, not an error):

- **Entity**: `Person`, `Username`, `Email`, `PhoneNumber`, `Domain`,
  `IPAddress`, `OnlineAccount`, `Organization`, `Location`, `Document`,
  `Image`, `Vehicle`.
- **Relationship**: `HasAccount`, `Owns`, `AssociatedWith`, `LocatedAt`,
  `MemberOf`, `ResolvesTo`, `Mentions`, `RelatedTo`.

## Attribute conflicts

`entity show` marks the most-recently-collected value per attribute key
with `*`; if two facts disagree on the same key, every value for it is
still shown, flagged `* (conflict, other values exist)` rather than
silently picking one (SPEC.md §4.4) — nothing is ever hidden.

## Not yet implemented

Every command in the [reference](#command-reference) above now has a real
implementation. Two flags remain accepted-but-no-op rather than fully
wired:

| Flag | Behavior |
|---|---|
| `entity list --filter <expr>` | parsed but ignored; only `--type` actually filters |
| `plugin list --installed` / `--available` | both no-ops — see the `plugin` section above |

There's also no `eumeaus plugin sign` command (see the `plugin verify`
section above) and no `case delete`/`case close` CLI command (an open
case's OS-keychain key has no way to be removed except by hand).

## Exit codes

`0` on success. Any failure — a bad argument, an unknown id, an engine
error — prints `error: <message>` to stderr and exits `1`.
