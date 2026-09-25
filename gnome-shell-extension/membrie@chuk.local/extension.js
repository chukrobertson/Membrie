// SPDX-License-Identifier: AGPL-3.0-or-later

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

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
    <signal name="OwnerChanged">
      <arg type="as" name="mimetypes"/>
    </signal>
    <signal name="TextReady"/>
  </interface>
</node>
`;

class ClipboardBridge {
    constructor() {
        this._reading = false;
        this._readGeneration = 0;
        this._readTimeoutId = 0;
        this._cachedText = '';
        this._selection = global.display.get_selection();
        this._clipboard = St.Clipboard.get_default();
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
        console.error('Membrie Clipboard Bridge lost its local session-bus name');
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

    destroy() {
        if (this._selection && this._ownerChangedId > 0)
            this._selection.disconnect(this._ownerChangedId);
        this._ownerChangedId = 0;
        this._selection = null;
        this._clipboard = null;
        this._cachedText = '';

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
