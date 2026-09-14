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
