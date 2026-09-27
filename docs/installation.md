# Installation and data safety

Membrie installs entirely inside the current Ubuntu user account. The installer does
not need administrator access, does not open a network port, and does not add a system
service.

## Install

From a Membrie source checkout:

```bash
sudo apt install build-essential cargo pkg-config libgtk-4-dev libadwaita-1-dev libglib2.0-dev gnome-shell gir1.2-ecal-2.0 ffmpeg whisper.cpp
./scripts/install.sh
./scripts/install-speech-model.sh
```

The first command installs Ubuntu development packages. The second command compiles
and installs Membrie for the current user. Log out and back in once if GNOME says the
desktop bridge is queued rather than active. Membrie will then appear in the app
grid and its local background services will start at login.

The third command makes one verified 57 MiB model download and enables English voice-note
transcription. It is optional: recordings are always retained even when the speech runtime or model
is absent. Once installed, decoding and transcription happen entirely on this PC.

After the GNOME bridge has loaded, press `Super+Shift+B` to open Quick Brie over the current
workflow. GNOME launches a normal compact GTK window with a valid activation token rather than an
unsupported always-on-top surface. Its header is draggable. Press `Esc`, its close button, or the
shortcut again while Quick Brie has focus to close it; when another window covers it, the shortcut
brings it forward. Quick Brie can also be opened from the Membrie launcher's context menu.

The installer places files in:

- `~/.local/bin` and `~/.local/libexec/membrie` for the application programs;
- `~/.local/share/applications` and `~/.local/share/icons` for the app-grid entry;
- `~/.config/systemd/user` for the three user services;
- the normal per-user GNOME Shell extension directory for the desktop bridge.

No service runs as root. The database and local socket are readable only by the user.

Screen Memory additionally uses a private directory beneath `$XDG_RUNTIME_DIR` for
short-lived active-window PNGs. It removes each image before local Ollama analysis begins;
only labeled text context is stored in the database. Screen Memory is off by default and
requires a local vision model such as `gemma4:e2b`.

Semantic Context is off by default. When it is first enabled or tested, Membrie may ask to
enable GNOME accessibility. The confirmation explains that this makes application-provided UI
structure available to Membrie and other local accessibility tools. Membrie's reader is bounded
and read-only, skips password fields, and never invokes application actions. Automatic capture
still passes through Membrie's pause, exclusion, sensitive-content, and duplicate policies before
storage. The separate five-second compatibility test never writes its result to the database.

When both Semantic Context and Screen Memory are enabled, rich application-provided context avoids
a redundant screenshot. Partial or unavailable semantic context lets Screen Memory provide a local
visual fallback. Both sources remain dependent on Activity Context, and all processing stays local.
Some already-open applications may need to be restarted after accessibility is enabled.

Calendar access is also off by default. When enabled, Membrie reads the calendars already exposed by
Ubuntu's Evolution Data Server every 15 minutes. It imports a bounded one-year history and one-year
look-ahead, expands recurring events, and makes them locally searchable by Brie. The connector does
not receive account credentials and contains no calendar write or forced-refresh operation. GNOME
calendar providers may continue their own configured synchronization independently of Membrie.
The `gir1.2-ecal-2.0` package in the install command supplies the small runtime description needed
to call the calendar library; the installer reports an ordinary-language reminder when it is absent.

Mobile Companion is off by default. Its service listens only on `127.0.0.1:47381`, so installing it
does not make Membrie reachable from the LAN or tailnet. Enable it in **Privacy & Capture**, open the
local preview, and use **Pair another device** to pair that browser. The token file is private to the
current Ubuntu user and is retained with the rest of Membrie's data during uninstall.

**Pair another device** creates a QR code and temporary 8-digit code that expire after five minutes
and one successful use. Scanning the QR pairs a normal browser directly; the short code is convenient
when an iPhone home-screen installation has its own separate storage. **Copy fallback token** remains
available for recovery. Pairing invitations and tokens are never sent to a cloud service.

For tailnet access, first record this PC's exact MagicDNS name with the installed companion binary,
then point Tailscale Serve at the loopback listener:

```bash
~/.local/libexec/membrie/membrie-mobile --trust-tailnet-host exact-name.example.ts.net
systemctl --user restart membrie-mobile.service
sudo tailscale serve --bg --yes http://127.0.0.1:47381
```

Use the exact hostname reported by `tailscale status --json`; do not copy the example. Tailscale
Serve provides private HTTPS to tailnet members and persists its configuration. Membrie validates
that exact Host and HTTPS Origin, still requires the pairing token, and continues to reject direct
network peers. Do not use Tailscale Funnel, which is intended for public internet exposure.

The default locked-PC rule permits paired devices to create notes but denies Recall, Brie, and both
clipboard directions. **Allow recall while this PC is locked** is a separate explicit choice. The
Tailscale access does not weaken this rule: tailnet membership and pairing are required before the
locked-session boundary is evaluated.

The Timeline composer can create an image Remembrie from a file picker, pasted image, or dropped file.
Mobile Companion adds a deliberate paste inbox and multi-file picker: text and links join the note,
while every binary item iOS exposes is previewed before submission and retained on the PC. One mobile
Remembrie accepts up to eight attachments and 100 MiB combined. PNG, JPEG, GIF, and WebP are interpreted
by the selected local vision model; common audio formats are transcribed by the optional local speech
model for voice-note recall. No item is sent to a remote API. Other formats are retained exactly but
labeled unsupported for local understanding. Companion usage shown in its status sheet is only a local
opening count and visible-time total, not telemetry.

## Backups

Membrie creates a verified complete backup when the daemon starts if the latest backup
is more than 24 hours old. It checks again hourly while running and keeps the newest 14
backups. The Privacy & Capture screen also has a **Create backup now** button.

The live database is normally:

```text
~/.local/share/membrie/membrie.db
```

New complete backups are normally directories:

```text
~/.local/share/membrie/backups/membrie-<timestamp>.backup/
  membrie.db
  manifest.txt
  blobs/sha256/...
```

If `XDG_DATA_HOME` is set, both locations move beneath that directory instead. Each database is
created with SQLite's online backup mechanism and checked with SQLite's integrity check. Every
referenced attachment is included, a manifest records its hash and size, and the complete private
directory is synced before it is counted as successful. Older `membrie-<timestamp>.db` snapshots
remain recognized, but they predate attachment-inclusive backups.

These backups protect against database corruption and accidental local changes. They
are still on the same physical disk. For protection against disk loss or another
migration accident, copy the entire `membrie` data directory to encrypted storage you
control while Membrie is closed. Membrie itself never uploads it.

## Check the background services

The Privacy & Capture screen reports the daemon and desktop capture helper in ordinary
language. For troubleshooting from a terminal:

```bash
systemctl --user status membried.service membrie-capture.service membrie-mobile.service
```

The desktop helper intentionally restarts if it comes up before the GNOME bridge is
ready. It is attached to GNOME's graphical-session lifecycle, so logging out stops it
cleanly and the next graphical login starts it again. The database daemon remains
available as a separate per-user service.

## Uninstall

```bash
./scripts/uninstall.sh
```

This removes the application, launcher, services, and GNOME bridge. It deliberately
keeps the database and every backup. The final terminal message prints the retained
data directory.
