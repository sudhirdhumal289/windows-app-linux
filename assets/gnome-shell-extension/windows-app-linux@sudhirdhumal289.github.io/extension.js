// Reports and changes the workspace of an app's own windows over D-Bus, which GNOME
// on Wayland offers no client protocol for. Windows are looked up by title among those
// owned by the calling process, so a client cannot see or move other apps' windows.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const OBJECT_PATH = '/com/microsoft/windowsapp/linux/WindowPlacement';
const INTERFACE_XML = `
<node>
  <interface name="com.microsoft.windowsapp.linux.WindowPlacement">
    <method name="GetWorkspace">
      <arg type="s" name="title" direction="in"/>
      <arg type="i" name="workspace" direction="out"/>
    </method>
    <method name="MoveToWorkspace">
      <arg type="s" name="title" direction="in"/>
      <arg type="u" name="workspace" direction="in"/>
      <arg type="b" name="moved" direction="out"/>
    </method>
  </interface>
</node>`;

function withCallerPid(invocation, callback) {
    invocation.get_connection().call(
        'org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
        'GetConnectionUnixProcessID', new GLib.Variant('(s)', [invocation.get_sender()]),
        new GLib.VariantType('(u)'), Gio.DBusCallFlags.NONE, -1, null,
        (connection, result) => {
            let pid;
            try {
                [pid] = connection.call_finish(result).deepUnpack();
            } catch (e) {
                invocation.return_gerror(e);
                return;
            }
            callback(pid);
        });
}

function findWindow(pid, title) {
    return global.display.list_all_windows()
        .find(window => window.get_pid() === pid && window.get_title() === title) ?? null;
}

class WindowPlacement {
    GetWorkspaceAsync([title], invocation) {
        withCallerPid(invocation, pid => {
            const window = findWindow(pid, title);
            let index = -1;
            if (window && !window.is_on_all_workspaces())
                index = window.get_workspace()?.index() ?? -1;
            invocation.return_value(new GLib.Variant('(i)', [index]));
        });
    }

    MoveToWorkspaceAsync([title, index], invocation) {
        withCallerPid(invocation, pid => {
            const window = findWindow(pid, title);
            if (!window || window.is_on_all_workspaces()) {
                invocation.return_value(new GLib.Variant('(b)', [false]));
                return;
            }
            const manager = global.workspace_manager;
            const workspace = manager.get_workspace_by_index(
                Math.min(index, manager.get_n_workspaces() - 1));
            window.change_workspace(workspace);
            workspace.activate_with_focus(window, global.get_current_time());
            invocation.return_value(new GLib.Variant('(b)', [true]));
        });
    }
}

export default class WindowPlacementExtension extends Extension {
    enable() {
        this._dbus = Gio.DBusExportedObject.wrapJSObject(INTERFACE_XML, new WindowPlacement());
        this._dbus.export(Gio.DBus.session, OBJECT_PATH);
    }

    disable() {
        this._dbus?.unexport();
        this._dbus = null;
    }
}
