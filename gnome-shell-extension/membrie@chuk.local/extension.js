// SPDX-License-Identifier: AGPL-3.0-or-later

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const DBUS_NAME = 'com.chuk.Membrie.Clipboard';
const DBUS_PATH = '/com/chuk/Membrie/Clipboard';
const DBUS_INTERFACE = 'com.chuk.Membrie.Clipboard';
const MAX_TEXT_BYTES = 256 * 1024;

const DBUS_XML = `
<node>
  <interface name="${DBUS_INTERFACE}">
    <method name="GetText">
      <arg direction="out" type="s" name="text"/>
    </method>
    <method name="RequestText"/>
    <method name="GetActivityState">
      <arg direction="out" type="s" name="app_id"/>
      <arg direction="out" type="s" name="app_name"/>
      <arg direction="out" type="s" name="window_title"/>
      <arg direction="out" type="u" name="idle_ms"/>
      <arg direction="out" type="b" name="locked"/>
    </method>
    <signal name="OwnerChanged">
      <arg type="as" name="mimetypes"/>
    </signal>
    <signal name="TextReady"/>
    <signal name="ActivityChanged"/>
  </interface>
</node>
`;

class ClipboardBridge {
    constructor() {
        this._reading = false;
        this._readGeneration = 0;
        this._readTimeoutId = 0;
        this._cachedText = '';
        this._activitySignalId = 0;
        this._focusWindow = null;
        this._titleChangedId = 0;
        this._selection = global.display.get_selection();
        this._clipboard = St.Clipboard.get_default();
        this._windowTracker = Shell.WindowTracker.get_default();
        this._idleMonitor = global.backend.get_core_idle_monitor();
        this._ownerChangedId = this._selection.connect(
            'owner-changed',
            this._onOwnerChanged.bind(this)
        );

        this._dbusObject = Gio.DBusExportedObject.wrapJSObject(DBUS_XML, this);
        this._dbusObject.export(Gio.DBus.session, DBUS_PATH);
        this._nameId = Gio.DBus.session.own_name(
            DBUS_NAME,
            Gio.BusNameOwnerFlags.NONE,
            null,
            this._onNameLost.bind(this)
        );
        this._focusChangedId = global.display.connect(
            'notify::focus-window',
            () => {
                this._trackFocusedWindow();
                this._queueActivityChanged();
            }
        );
        this._sessionUpdatedId = Main.sessionMode.connect(
            'updated',
            () => this._queueActivityChanged()
        );
        this._trackFocusedWindow();
    }

    _onOwnerChanged(_selection, type) {
        if (type !== Meta.SelectionType.SELECTION_CLIPBOARD || this._reading)
            return;

        GLib.idle_add(GLib.PRIORITY_DEFAULT_IDLE, () => {
            try {
                const mimetypes = this._selection.get_mimetypes(
                    Meta.SelectionType.SELECTION_CLIPBOARD
                );
                this._dbusObject.emit_signal(
                    'OwnerChanged',
                    new GLib.Variant('(as)', [mimetypes])
                );
            } catch (error) {
                logError(error, 'Membrie could not inspect clipboard formats');
            }
            return GLib.SOURCE_REMOVE;
        });
    }

    _onNameLost() {
        console.error('Membrie Desktop Bridge lost its local session-bus name');
        this._nameId = 0;
    }

    GetText() {
        const text = this._cachedText;
        this._cachedText = '';
        return text;
    }

    RequestText() {
        if (this._reading)
            return;

        this._reading = true;
        const generation = ++this._readGeneration;
        this._readTimeoutId = GLib.timeout_add(
            GLib.PRIORITY_DEFAULT,
            5000,
            () => {
                if (generation === this._readGeneration) {
                    this._cachedText = '';
                    this._reading = false;
                    console.warn('Membrie clipboard read timed out');
                }
                this._readTimeoutId = 0;
                return GLib.SOURCE_REMOVE;
            }
        );

        this._clipboard.get_text(
            St.ClipboardType.CLIPBOARD,
            (_clipboard, text) => {
                if (generation !== this._readGeneration)
                    return;

                if (this._readTimeoutId > 0)
                    GLib.source_remove(this._readTimeoutId);
                this._readTimeoutId = 0;

                if (text !== null &&
                    new TextEncoder().encode(text).length <= MAX_TEXT_BYTES)
                    this._cachedText = text;
                else
                    this._cachedText = '';

                this._dbusObject.emit_signal('TextReady', null);
                this._reading = false;
            }
        );
    }

    GetActivityState() {
        const window = global.display.focus_window;
        let appId = '';
        let appName = '';
        let windowTitle = '';
        if (window !== null) {
            const app = this._windowTracker.get_window_app(window);
            appId = app?.get_id() ?? window.get_wm_class() ?? '';
            appName = app?.get_name() ?? window.get_wm_class() ?? '';
            windowTitle = window.get_title() ?? '';
        }
        const idleTime = Math.max(0, Math.trunc(this._idleMonitor.get_idletime()));
        const idleMs = Math.min(idleTime, 0xffffffff);
        return [appId, appName, windowTitle, idleMs, Boolean(Main.sessionMode.isLocked)];
    }

    _trackFocusedWindow() {
        if (this._focusWindow && this._titleChangedId > 0)
            this._focusWindow.disconnect(this._titleChangedId);
        this._focusWindow = global.display.focus_window;
        this._titleChangedId = 0;
        if (this._focusWindow) {
            this._titleChangedId = this._focusWindow.connect(
                'notify::title',
                () => this._queueActivityChanged()
            );
        }
    }

    _queueActivityChanged() {
        if (this._activitySignalId > 0)
            return;
        this._activitySignalId = GLib.idle_add(GLib.PRIORITY_DEFAULT_IDLE, () => {
            this._activitySignalId = 0;
            try {
                this._dbusObject.emit_signal('ActivityChanged', null);
            } catch (error) {
                logError(error, 'Membrie could not report a desktop activity change');
            }
            return GLib.SOURCE_REMOVE;
        });
    }

    destroy() {
        if (this._selection && this._ownerChangedId > 0)
            this._selection.disconnect(this._ownerChangedId);
        this._ownerChangedId = 0;
        this._selection = null;
        this._clipboard = null;
        this._cachedText = '';

        if (this._focusWindow && this._titleChangedId > 0)
            this._focusWindow.disconnect(this._titleChangedId);
        this._titleChangedId = 0;
        this._focusWindow = null;
        if (this._focusChangedId > 0)
            global.display.disconnect(this._focusChangedId);
        this._focusChangedId = 0;
        if (this._sessionUpdatedId > 0)
            Main.sessionMode.disconnect(this._sessionUpdatedId);
        this._sessionUpdatedId = 0;
        if (this._activitySignalId > 0)
            GLib.source_remove(this._activitySignalId);
        this._activitySignalId = 0;
        this._windowTracker = null;
        this._idleMonitor = null;

        if (this._readTimeoutId > 0)
            GLib.source_remove(this._readTimeoutId);
        this._readTimeoutId = 0;

        if (this._nameId > 0)
            Gio.DBus.session.unown_name(this._nameId);
        this._nameId = 0;

        try {
            this._dbusObject.unexport();
        } catch (_error) {
            // The bridge may not have finished exporting during Shell shutdown.
        }
        this._dbusObject.run_dispose();
        this._dbusObject = null;
    }
}

export default class MembrieClipboardExtension extends Extension {
    enable() {
        this._bridge = new ClipboardBridge();
    }

    disable() {
        this._bridge?.destroy();
        this._bridge = null;
    }
}
