# Getting Started with Eumeaus (Desktop GUI)

This guide walks a new user through installing the Eumeaus desktop app,
opening a first case, and running a first scan — no command line
required. If you'd rather use the CLI, see [`CLI.md`](./CLI.md) instead.

Screenshots are marked as placeholders below (`Screenshot: ...`) — drop
the real images in as they're captured.

## 1. What Eumeaus is

Eumeaus is a local-first case management tool for OSINT (open-source
intelligence) work. You create a case, add the people/usernames/emails/
domains/etc. you're looking into as **entities**, and run **scans**
against them — small, isolated plugins that look things up (a username
across sites, an IP's location, a domain's registration, and so on) and
merge what they find back into your case as new entities and
relationships, each one tagged with where it came from.

Everything lives in one encrypted file on your own machine. Nothing is
uploaded anywhere unless you explicitly export and share it yourself.

## 2. Install the app

Download the Windows installer (`.exe` or `.msi`) from the [latest GUI
release](https://github.com/RedRockerSE/eumeaus/releases/latest) and
run it. Linux users can grab the `.AppImage` or `.deb`/`.rpm` package
from the same page.

> **Screenshot:** the GitHub Releases page, with the Windows installer
> asset highlighted.

> **Screenshot:** the Windows installer's welcome/install screen.

Launch **Eumeaus** from the Start menu (Windows) or your applications
list (Linux) once it's installed.

## 3. Install the plugins

The desktop app is the case-management shell — the actual lookups
(username search, IP geolocation, domain lookup, and so on) are separate
plugin programs the app calls out to. The GUI installer doesn't bundle
them, so this is a one-time separate step.

Run the CLI installer for your platform — it fetches the plugin bundle
and puts it somewhere the GUI can point at:

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/RedRockerSE/eumeaus/main/install.ps1 | iex
```

```sh
# Linux
curl -fsSL https://raw.githubusercontent.com/RedRockerSE/eumeaus/main/install.sh | sh
```

This installs a small CLI tool (which you don't need to use directly)
and a `eumeaus-plugins` folder alongside it — by default:
- Windows: `%LOCALAPPDATA%\eumeaus\eumeaus-plugins`
- Linux: `~/.local/bin/eumeaus-plugins`

Keep that folder's path handy — you'll point Settings at it in the next
step.

## 4. Create your first case

On first launch you'll see the **Open a case** screen.

> **Screenshot:** the Launcher screen, showing the "Open a case" and
> "New case" forms.

Under **New case**, click **Browse…** to pick a folder for your case
file, type a name (e.g. `first-case`), and click **New case…**. This
creates one encrypted `first-case.eum` file in that folder and opens it
— you're now in the main window.

To come back to a case later, use **Open a case** and browse to the
`.eum` file, or **Browse a directory** to see every case file in a
folder at once.

## 5. A quick tour of the main window

The sidebar on the left switches between:

| Screen | What it's for |
|---|---|
| **Overview** | Case summary — entity/relationship counts, recent activity |
| **Entities** | Add, search, and edit the things you're investigating |
| **Graph** | A visual map of entities and how they're connected |
| **Map** | Every entity/fact with a location, shown as a pin |
| **Scans** | Run a plugin (or several) against an entity |
| **Plugins** | Install and manage plugins |
| **Settings** | Default plugins folder, auto-scan, and other preferences |

> **Screenshot:** the main window with the sidebar visible, on the
> Overview screen.

## 6. Point Eumeaus at your plugins

Go to **Settings > General** and paste (or **Browse…** to) the
`eumeaus-plugins` folder path from step 3, then **Save**. This becomes
the default plugins folder everywhere else in the app, so you won't
need to re-enter it on the Scans or Plugins screens.

> **Screenshot:** Settings > General, with the plugins directory field
> filled in.

## 7. Add your first entity

Go to **Entities** and click **Add entity**. Pick a type (e.g.
`Username`), type in the value you're investigating as the **Canonical
key** (e.g. a handle), and click **Add entity**.

> **Screenshot:** the Entities screen with the "New entity" form open
> and filled in.

Your new entity now appears in the list.

## 8. Run a scan

Go to **Scans**. Under **Target**, pick the same entity type and the
entity you just added. Confirm the **Plugins directory** matches what
you set in Settings (it should already be filled in) and click **List**
to load the available plugins. Leave the plugin checklist blank to run
every plugin compatible with that entity type, or check specific ones.
Click **Run scan**.

> **Screenshot:** the Scans screen mid-run, showing live per-plugin
> progress.

Results — new entities and relationships the plugins found — merge into
your case automatically as the scan runs. If you close the app or lose
power mid-scan, reopening the case and starting the same scan again
picks up only the plugins that hadn't finished.

## 9. Explore what was found

- **Entities** now lists everything the scan discovered, each tagged
  with which plugin found it.
- **Graph** draws every entity as a node and every relationship as a
  connecting line — drag nodes around to untangle a busy case.
- **Map** plots anything with a location (an IP's geolocation, GPS data
  from an uploaded image, etc.) as a pin.

> **Screenshot:** the Graph screen showing a small case with a few
> connected entities.

> **Screenshot:** the Map screen showing at least one pin.

## 10. Optional: turn on auto-scan

Normally, adding an entity and scanning it are two separate steps. If
you'd rather have every new entity scanned automatically the moment
it's added, turn on **Auto-scan on add** in **Settings > General**. It's
off by default on purpose — flip it on once you're comfortable with
which plugins you're running and what they contact.

## 11. Export a report

From **Overview**, use **Export** to save a signed, self-contained HTML
report of the whole case — entities, relationships, and where each fact
came from — that opens in any browser, independent of Eumeaus itself.

> **Screenshot:** the Overview screen's Export card.

## Where to go next

- [`CLI.md`](./CLI.md) — the full command-line reference, if you want
  to script or automate anything the GUI covers by hand.
- [`plugin-developer-guide.md`](./plugin-developer-guide.md) — write
  your own plugin, in any language.
- [GitHub Issues](https://github.com/RedRockerSE/eumeaus/issues) —
  report a bug or request a feature.
