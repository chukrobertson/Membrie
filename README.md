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

- Native GTK 4/libadwaita shell with Timeline, Search, Brie, and Privacy views
- Local daemon with a small typed JSON protocol over a Unix socket
- SQLite schema for Remembries, content, relationships, embeddings, and processing jobs
- Manual Remembrie capture
- Day, week, and month activity maps with stable application colors and day-by-day chronology
- Color-grouped application time ribbon with idle gaps, stable app colors, and exact evidence
- Evidence-backed repeated-context observations without productivity scoring
- Exact full-text search powered by SQLite FTS5/BM25
- Persistent pause state
- Safe, explicitly enabled clipboard capture through a local GNOME Shell bridge
- Optional focused-application and window-title activity sessions with idle boundaries
- Opt-in Semantic Context capture using bounded, read-only GNOME accessibility data
- Adaptive semantic-first capture: rich application context avoids redundant screenshots,
  while sparse or unavailable context falls back to Screen Memory when it is enabled
- A consent-gated, store-nothing Semantic Context compatibility test
- Opt-in active-window Screen Memory with local visual-change filtering
- A five-second “Remember this screen” control for deliberate one-off capture
- Temporary PNGs removed from disk before their pixels are analyzed in memory by a selected loopback Ollama vision model
- Machine-described screen context is explicitly labeled as fallible evidence for Brie
- In-progress screen analysis survives an activity-session boundary and joins that Remembrie before local indexing
- Activity sessions become searchable, cited Remembries without recording keyboard or pointer input
- Pre-storage secret detection and duplicate suppression
- Built-in password-manager and private-window exclusions
- User-managed application exclusion rules
- Temporary capture pauses and deletion by recent time range
- Daily integrity-checked local backups with a rolling 14-backup history
- Capture-service health reporting in the Privacy screen
- Rootless Ubuntu installation, app launcher, and automatic user services
- Restart-safe local summary and embedding jobs
- Hybrid exact-text and semantic search with `embeddinggemma`
- Brie answers powered by local `gemma4:12b`, with exact Remembrie citations
- Brie citations open the matching Timeline day, highlight the source, and show its exact evidence
- Quick Brie compact window for local recall and note capture without leaving the current workflow
- Local model selection and visible indexing health

Dedicated OCR, optional retained evidence controls, and deeper pattern detection are the next
implementation milestones.

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
user, adds it to the Ubuntu app grid, installs the GNOME desktop bridge, and starts
the local daemon and capture helper automatically at login. A new GNOME extension may
need one log out and back in before desktop context and clipboard capture become available.
The same bridge registers `Super+Shift+B` for Quick Brie. The compact window receives only the
previous application's name and window title as optional search context; press `Esc` to close it.

Brie requires a local Ollama installation and two downloaded models. The recommended
defaults are:

```bash
ollama pull gemma4:12b
ollama pull embeddinggemma
ollama pull gemma4:e2b
```

The first two models power Brie and hybrid recall. The optional third model powers
Screen Memory and is not used unless that source is explicitly enabled. Other installed
vision-capable Ollama models can be selected in Privacy & Capture.

Membrie connects only to Ollama's fixed loopback address (`127.0.0.1`). Cloud-backed
Ollama model names are deliberately rejected.

To remove the installed application while keeping every Remembrie and backup:

```bash
./scripts/uninstall.sh
```

See [Installation and data safety](docs/installation.md) for the installed locations,
service checks, and backup details.

## Run the development build

The native development libraries for GTK 4 and libadwaita are required. Install the
small local GNOME Shell bridge once so Wayland can deliver clipboard and desktop-context
changes while
Membrie is in the background:

```bash
./scripts/install-gnome-extension.sh
```

The extension exports only a local session-bus interface and has no network code.
Activity Context, Semantic Context, and Screen Memory stay off until enabled in Membrie's
Privacy & Capture screen. Semantic Context reads bounded, visible application labels and
text through GNOME's accessibility interface. Rich semantic observations replace a
redundant screenshot; partial or unavailable observations allow Screen Memory to fill the
gap when that separate source is enabled. Screen Memory takes one-shot samples of the
active window, filters unchanged frames locally, removes each PNG from disk before local
Ollama analysis, and never records keyboard or pointer input. GNOME may require one log
out and back in before it can load a newly installed local extension; the installer queues
it to enable on that next login. After the bridge is enabled, start Membrie:

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
crates/membrie-capture Local D-Bus client for the GNOME desktop bridge
crates/membrie-a11y    Bounded, read-only AT-SPI semantic-context reader
crates/membrie-app     Native GTK 4/libadwaita application
gnome-shell-extension  Local clipboard and desktop-context bridge for GNOME Wayland
docs/                  Product and architecture decisions
```

## License

Membrie is free software licensed under the
[GNU Affero General Public License v3.0 or later](LICENSE). You may use, study,
modify, and share it; if you distribute a modified version or provide one for
others to use over a network, you must make the corresponding source available
under the same license.
