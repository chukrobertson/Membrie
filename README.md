# Membrie

Membrie is a private, local-first memory companion for Ubuntu. It captures useful
moments as **Remembries**, makes them searchable, and will use the local assistant
**Brie** to help recall them with exact citations.

The project is intentionally local-first and self-hosted:

- one SQLite database is the source of truth;
- the database daemon listens on a user-only Unix socket, never a network port;
- the desktop app and companion gateway talk to that private daemon socket;
- the optional companion gateway initially listens on loopback only and requires its own pairing token;
- no telemetry, remote APIs, or cloud services are included.

## What works today

- Native GTK 4/libadwaita shell with Timeline, Search, Brie, and Privacy views
- Local daemon with a small typed JSON protocol over a Unix socket
- SQLite schema for Remembries, content, relationships, embeddings, and processing jobs
- Manual Remembrie capture
- File Remembries from the desktop, plus a Mobile Companion paste inbox for mixed text, links, images, audio, and files
- Content-addressed attachment storage with byte-level deduplication and retained original evidence
- Local image description and visible-text extraction through the selected Ollama vision model
- Direct Companion voice recording, local playback, and English transcription through Ubuntu's `whisper.cpp`, with timestamped searchable evidence
- Transcript correction stored separately from the preserved machine interpretation, per-recording retry, and explicit attachment or Remembrie deletion
- Day, week, and month activity maps with stable application colors and day-by-day chronology
- Color-grouped application time ribbon with idle gaps, stable app colors, and exact evidence
- Timeline filters for observed activity, scheduled events, and manual notes, including upcoming days
- Evidence inspection that states each source's provenance and what it can—and cannot—prove
- Evidence-backed repeated-context, resume-after-break, and clustered-notification patterns without productivity scoring
- Clickable pattern evidence in the Timeline and deterministic pattern context for Brie when asked
- Exact full-text search powered by SQLite FTS5/BM25
- Persistent pause state
- Safe, explicitly enabled clipboard capture through a local GNOME Shell bridge
- Optional focused-application and window-title activity sessions with idle boundaries
- Opt-in Semantic Context capture using bounded, read-only GNOME accessibility data
- First-class LibreOffice Writer, Calc, Impress, Draw, Base, and Math context with focused-document identity and bounded text excerpts
- First-class Thunderbird mailbox, message, and compose context with truthful Sent, Drafts, Outbox, and delivery boundaries
- Off-by-default Notification Memory with per-application controls, hard password-manager and authentication-code protection, and transient-progress suppression
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
- Daily integrity-checked local backups of the database and referenced attachments, with a rolling 14-backup history
- Capture-service health reporting in the Privacy screen
- Rootless Ubuntu installation, app launcher, and automatic user services
- Restart-safe local summary and embedding jobs
- Hybrid exact-text and semantic search with `embeddinggemma`
- Brie answers powered by local `gemma4:12b`, with exact Remembrie citations
- Evidence-aware Brie reasoning that separates scheduled plans, observed activity, copied text, and notes
- Brie citations open the matching Timeline day, highlight the source, and show its exact evidence
- Quick Brie compact window for local recall and note capture without leaving the current workflow
- Opt-in read-only GNOME Calendar integration with recurring-event expansion, local reconciliation, and per-calendar Allow/Ignore controls
- Responsive Mobile Companion for universal paste capture, multi-file selection, direct voice notes, explicit text/image clipboard transfers, Brie, and compact recall
- Independent Mobile Companion pairing plus a fail-closed locked-PC privacy boundary
- One-use five-minute QR or 8-digit pairing invitations, with the reusable token retained as fallback
- Truthful `Membrie Companion` provenance for phone notes and source-aware recent-note recall in Brie
- Coarse, local-only Companion usage totals for the last day, week, and month
- Local model selection and visible indexing health

Broader file-format understanding, meeting transcripts, individual device revocation,
and richer cross-source corroboration are the next implementation milestones.

## Install on Ubuntu

Install the native build requirements once:

```bash
sudo apt install build-essential cargo pkg-config libgtk-4-dev libadwaita-1-dev libglib2.0-dev gnome-shell gir1.2-ecal-2.0 ffmpeg whisper.cpp
```

Then run the local installer from this checkout:

```bash
./scripts/install.sh
```

The installer never uses `sudo`. It builds Membrie, installs it only for the current
user, adds it to the Ubuntu app grid, installs the GNOME desktop bridge, and starts
the local daemon, capture helper, and loopback Mobile Companion automatically at login. A new GNOME extension may
need one log out and back in before desktop context and clipboard capture become available.
The same bridge registers `Super+Shift+B` for Quick Brie. The compact window receives only the
previous application's name and window title as optional search context. Drag its header to move
it; press `Esc`, its close button, or the shortcut again while it has focus to close it.

Brie requires a local Ollama installation and two downloaded models. The recommended
defaults are:

```bash
ollama pull gemma4:12b
ollama pull embeddinggemma
ollama pull gemma4:e2b
```

The first two models power Brie and hybrid recall. The optional third model powers
Screen Memory and image-attachment understanding. Image understanding runs only after a deliberate
attachment; it does not require automatic Screen Memory to be enabled. Other installed vision-capable
Ollama models can be selected in Privacy & Capture.

Voice-note understanding uses Ubuntu's native `whisper.cpp` package rather than an audio upload or
remote API. Install Membrie's small, quantized English model once:

```bash
./scripts/install-speech-model.sh
```

That script makes one verified 57 MiB model download. Afterward, M4A and other common audio
attachments up to 30 minutes are decoded and transcribed entirely on this PC. Timestamped text is
explicitly labeled as an unverified machine transcription; the original recording remains the
canonical evidence. Audio with no clear speech is labeled that way instead of silently treated as a
voice note. Companion can record and preview a voice note directly. Recall can play the retained
original, show its timestamped transcript, queue another local transcription, and store a user's
correction alongside—never over—the machine interpretation.

Calendar access is separately opt-in. Membrie reads enabled calendars from Ubuntu's local
Evolution Data Server, never receives calendar-account credentials, and exposes no calendar write
or refresh operation. A provider configured in GNOME may continue its own normal synchronization;
Membrie only reads the resulting local calendar view. After the first discovery sync, each calendar
can be allowed or ignored independently. An ignored calendar is not read during later syncs; its
already remembered events remain available unless the user explicitly deletes them.

Membrie connects only to Ollama's fixed loopback address (`127.0.0.1`). Cloud-backed
Ollama model names are deliberately rejected.

Mobile Companion is separately opt-in in **Privacy & Capture**. It always listens only on the
Ubuntu PC at `http://127.0.0.1:47381` and requires a private pairing token generated on that PC.
An explicitly configured Tailscale Serve proxy can publish that loopback service to the exact
MagicDNS hostname over private tailnet HTTPS; Membrie still opens no LAN or public listener.
While the PC is locked, paired devices can add notes but cannot read Remembries or clipboard text
unless the user explicitly relaxes that boundary.

The Mobile Companion accepts every file representation that iOS exposes through an explicit paste
gesture or file picker, with up to eight attachments and 100 MiB combined in one Remembrie. Text and
links enter the searchable note directly. PNG, JPEG, GIF, and WebP receive retained previews plus
local understanding. Common audio formats receive local speech transcription when the optional
speech model is installed. Other formats, including HEIC when no local conversion is available, are
retained exactly but labeled unsupported for machine analysis. Originals remain canonical; Brie
sees only clearly labeled, fallible text produced by local models.

Clipboard ferrying is always explicit. Text that resembles a credential or payment card is blocked.
On GNOME Wayland, **Fetch** can also bring a current PNG, JPEG, or WebP clipboard image to Companion
for a temporary preview; the user then chooses whether to remember it, save it, or invoke the phone's
share sheet. The bridge accepts both byte representations used across supported GNOME/GJS releases
and reports a specific problem when the clipboard is busy, times out, or contains no image. Binary
clipboard content is never captured or published automatically.

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
text through GNOME's accessibility interface. In a focused LibreOffice window it recognizes
Writer, Calc, Impress, Draw, Base, and Math, records the document identity from the active window,
and reads at most a small bounded excerpt exposed by read-only text interfaces, normally around
the caret. It never opens another document, reads files from disk, or calls LibreOffice editing
actions. Focused Thunderbird windows similarly expose only bounded accessibility context from the
mailbox, message, or compose view; Membrie never receives Gmail credentials or reads Thunderbird's
profile database. Compose, Drafts, Outbox, Inbox, and Sent are labeled with distinct evidence limits
so Brie does not confuse drafting with sending or a Sent copy with delivery. Separate, off-by-default
**Notification Memory** can remember eligible local notifications that GNOME receives, including
Thunderbird arrivals. It blocks password managers and authentication-code notifications, suppresses
transient progress chatter, and provides a control for each observed application. Those records prove
only that a notification appeared—not that it was seen, opened, or acted upon. No mail account
credentials or profile database access is involved. Rich semantic observations replace a
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
crates/membrie-daemon  Database owner, policy engine, calendar reader, and Unix-socket service
crates/membrie-capture Local D-Bus client for the GNOME desktop bridge
crates/membrie-a11y    Bounded, read-only AT-SPI semantic-context reader
crates/membrie-app     Native GTK 4/libadwaita application
crates/membrie-mobile  Paired, loopback-first responsive companion gateway
gnome-shell-extension  Local clipboard and desktop-context bridge for GNOME Wayland
docs/                  Product and architecture decisions
```

## License

Membrie is free software licensed under the
[GNU Affero General Public License v3.0 or later](LICENSE). You may use, study,
modify, and share it; if you distribute a modified version or provide one for
others to use over a network, you must make the corresponding source available
under the same license.
