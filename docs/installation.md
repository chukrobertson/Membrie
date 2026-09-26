# Installation and data safety

Membrie installs entirely inside the current Ubuntu user account. The installer does
not need administrator access, does not open a network port, and does not add a system
service.

## Install

From a Membrie source checkout:

```bash
sudo apt install build-essential cargo pkg-config libgtk-4-dev libadwaita-1-dev libglib2.0-dev gnome-shell gir1.2-ecal-2.0
./scripts/install.sh
```

The first command installs Ubuntu development packages. The second command compiles
and installs Membrie for the current user. Log out and back in once if GNOME says the
desktop bridge is queued rather than active. Membrie will then appear in the app
grid and its local background services will start at login.

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
local preview, and use **Show pairing token** to pair that browser. The token file is private to the
current Ubuntu user and is retained with the rest of Membrie's data during uninstall.

The default locked-PC rule permits paired devices to create notes but denies Recall, Brie, and both
clipboard directions. **Allow recall while this PC is locked** is a separate explicit choice. The
local preview intentionally does not configure Tailscale; tailnet exposure is a later, auditable
step after this boundary has been tested on the PC.

## Backups

Membrie creates a verified SQLite snapshot when the daemon starts if the latest backup
is more than 24 hours old. It checks again hourly while running and keeps the newest 14
snapshots. The Privacy & Capture screen also has a **Create backup now** button.

The live database is normally:

```text
~/.local/share/membrie/membrie.db
```

Backups are normally:

```text
~/.local/share/membrie/backups/membrie-<timestamp>.db
```

If `XDG_DATA_HOME` is set, both locations move beneath that directory instead. Each
snapshot is created with SQLite's online backup mechanism, checked with SQLite's
integrity check, synced to disk, and made private to the current user before it is
counted as a successful backup.

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
