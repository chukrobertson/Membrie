# Installation and data safety

Membrie installs entirely inside the current Ubuntu user account. The installer does
not need administrator access, does not open a network port, and does not add a system
service.

## Install

From a Membrie source checkout:

```bash
sudo apt install build-essential cargo pkg-config libgtk-4-dev libadwaita-1-dev libglib2.0-dev gnome-shell
./scripts/install.sh
```

The first command installs Ubuntu development packages. The second command compiles
and installs Membrie for the current user. Log out and back in once if GNOME says the
clipboard bridge is queued rather than active. Membrie will then appear in the app
grid and its local background services will start at login.

The installer places files in:

- `~/.local/bin` and `~/.local/libexec/membrie` for the application programs;
- `~/.local/share/applications` and `~/.local/share/icons` for the app-grid entry;
- `~/.config/systemd/user` for the two user services;
- the normal per-user GNOME Shell extension directory for the clipboard bridge.

No service runs as root. The database and local socket are readable only by the user.

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

The Privacy & Capture screen reports the daemon and clipboard helper in ordinary
language. For troubleshooting from a terminal:

```bash
systemctl --user status membried.service membrie-capture.service
```

The clipboard helper intentionally restarts if it comes up before the GNOME bridge is
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
