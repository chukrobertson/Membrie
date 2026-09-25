# Membrie

Membrie is a private, local-first memory companion for Ubuntu. It captures useful
moments as **Remembries**, makes them searchable, and will use the local assistant
**Brie** to help recall them with exact citations.

The project is intentionally local-only:

- one SQLite database is the source of truth;
- the daemon listens on a user-only Unix socket, never a network port;
- the desktop app talks only to that socket;
- no telemetry, remote APIs, or cloud services are included.

## What works today

- Native GTK 4/libadwaita shell with Timeline, Search, Brie, and Constellation views
- Local daemon with a small typed JSON protocol over a Unix socket
- SQLite schema for Remembries, content, relationships, embeddings, and processing jobs
- Manual Remembrie capture
- Recent timeline
- Exact full-text search powered by SQLite FTS5/BM25
- Persistent pause state
- Safe, explicitly enabled clipboard capture through a local GNOME Shell bridge
- Pre-storage secret detection and duplicate suppression
- Built-in password-manager and private-window exclusions
- User-managed application exclusion rules
- Temporary capture pauses and deletion by recent time range
- Daily integrity-checked local backups with a rolling 14-backup history
- Capture-service health reporting in the Privacy screen
- Rootless Ubuntu installation, app launcher, and automatic user services

Brie, OCR, embeddings, application-context adapters, and the interactive constellation
are the next implementation milestones. Their UI surfaces are present but clearly
marked as in progress.

## Install on Ubuntu

Install the native build requirements once:

```bash
sudo apt install build-essential cargo pkg-config libgtk-4-dev libadwaita-1-dev libglib2.0-dev gnome-shell
```

Then run the local installer from this checkout:

```bash
./scripts/install.sh
```

The installer never uses `sudo`. It builds Membrie, installs it only for the current
user, adds it to the Ubuntu app grid, installs the GNOME clipboard bridge, and starts
the local daemon and capture helper automatically at login. A new GNOME extension may
need one log out and back in before clipboard capture becomes available.

To remove the installed application while keeping every Remembrie and backup:

```bash
./scripts/uninstall.sh
```

See [Installation and data safety](docs/installation.md) for the installed locations,
service checks, and backup details.

## Run the development build

The native development libraries for GTK 4 and libadwaita are required. Install the
small local GNOME Shell bridge once so Wayland can deliver clipboard changes while
Membrie is in the background:

```bash
./scripts/install-gnome-extension.sh
```

The extension declares its clipboard access in GNOME, exports only a local session-bus
interface, and has no network code. GNOME may require one log out and back in before it
can load a newly installed local extension; the installer queues it to enable on that
next login. After the bridge is enabled, start Membrie:

```bash
./scripts/dev.sh
```

The development script builds the workspace, starts `membried`, and then opens the
native application. Press `Ctrl+C` in the terminal after closing the app if needed.

By default, data is written to:

```text
$XDG_DATA_HOME/membrie/membrie.db
```

or `~/.local/share/membrie/membrie.db` when `XDG_DATA_HOME` is unset. The socket is
placed under `$XDG_RUNTIME_DIR` when available.

Verified backups are written to `~/.local/share/membrie/backups` (or the equivalent
directory beneath `$XDG_DATA_HOME`). Nothing is uploaded or transmitted.

For an isolated development database:

```bash
MEMBRIE_DATA_DIR=/tmp/membrie-dev ./scripts/dev.sh
```

## Workspace

```text
crates/membrie-core    Canonical model, SQLite repository, paths, and IPC client
crates/membrie-daemon  Database owner, policy engine, and Unix-socket service
crates/membrie-capture Local D-Bus client for the GNOME clipboard bridge
crates/membrie-app     Native GTK 4/libadwaita application
gnome-shell-extension  Local clipboard bridge required by GNOME Wayland
docs/                  Product and architecture decisions
```

## License

Membrie is free software licensed under the
[GNU Affero General Public License v3.0 or later](LICENSE). You may use, study,
modify, and share it; if you distribute a modified version or provide one for
others to use over a network, you must make the corresponding source available
under the same license.
