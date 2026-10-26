//! GNOME Shell search provider (`org.gnome.Shell.SearchProvider2`): Cloud PCs, Azure
//! Virtual Desktop resources and Remote PCs appear in the Activities search and connect
//! when activated. Results come from the local [`SearchIndex`], so searching never
//! signs in or touches the network.
//!
//! GNOME Shell finds the provider through
//! `/usr/share/gnome-shell/search-providers/com.microsoft.windowsapp.linux.search-provider.ini`
//! and starts the app with `--gapplication-service` through its D-Bus service file.

use crate::config::APP_ID;
use crate::launcher::{LaunchEntry, LaunchTarget, SearchIndex};
use adw::prelude::*;
use gtk4::{gio, glib};
use std::collections::HashMap;

pub const OBJECT_PATH: &str = "/com/microsoft/windowsapp/linux/SearchProvider";

const INTERFACE_XML: &str = r#"
<node>
  <interface name="org.gnome.Shell.SearchProvider2">
    <method name="GetInitialResultSet">
      <arg type="as" name="terms" direction="in"/>
      <arg type="as" name="results" direction="out"/>
    </method>
    <method name="GetSubsearchResultSet">
      <arg type="as" name="previous_results" direction="in"/>
      <arg type="as" name="terms" direction="in"/>
      <arg type="as" name="results" direction="out"/>
    </method>
    <method name="GetResultMetas">
      <arg type="as" name="identifiers" direction="in"/>
      <arg type="aa{sv}" name="metas" direction="out"/>
    </method>
    <method name="ActivateResult">
      <arg type="s" name="identifier" direction="in"/>
      <arg type="as" name="terms" direction="in"/>
      <arg type="u" name="timestamp" direction="in"/>
    </method>
    <method name="LaunchSearch">
      <arg type="as" name="terms" direction="in"/>
      <arg type="u" name="timestamp" direction="in"/>
    </method>
  </interface>
</node>
"#;

/// Most results GNOME Shell shows per provider anyway.
const MAX_RESULTS: usize = 10;

/// Exports the provider on the application's bus connection. `activate` is called with
/// the chosen connection, or `None` to just show the main window.
pub fn register(
    app: &adw::Application,
    activate: impl Fn(Option<LaunchTarget>) + 'static,
) -> Result<(), glib::Error> {
    let Some(connection) = app.dbus_connection() else {
        return Ok(());
    };
    let node = gio::DBusNodeInfo::for_xml(INTERFACE_XML)?;
    let interface = node
        .lookup_interface("org.gnome.Shell.SearchProvider2")
        .ok_or_else(|| glib::Error::new(gio::IOErrorEnum::Failed, "interface missing"))?;
    let weak_app = app.downgrade();
    connection
        .register_object(OBJECT_PATH, &interface)
        .method_call(move |_, _, _, _, method, params, invocation| {
            // Each search restarts the idle timeout of a D-Bus activated instance.
            if let Some(app) = weak_app.upgrade() {
                let hold = app.hold();
                glib::timeout_add_local_once(std::time::Duration::from_millis(100), move || {
                    drop(hold);
                });
            }
            let index = SearchIndex::default();
            let reply = match method {
                "GetInitialResultSet" => params
                    .get::<(Vec<String>,)>()
                    .map(|(terms,)| (ids(index.search(&terms)),).to_variant()),
                "GetSubsearchResultSet" => {
                    params
                        .get::<(Vec<String>, Vec<String>)>()
                        .map(|(previous, terms)| {
                            let matches = index.search(&terms);
                            let narrowed: Vec<LaunchEntry> = matches
                                .into_iter()
                                .filter(|e| previous.contains(&e.target.to_string()))
                                .collect();
                            (ids(narrowed),).to_variant()
                        })
                }
                "GetResultMetas" => params
                    .get::<(Vec<String>,)>()
                    .map(|(identifiers,)| (metas(&index, &identifiers),).to_variant()),
                "ActivateResult" => params.get::<(String, Vec<String>, u32)>().map(|(id, ..)| {
                    activate(LaunchTarget::parse(&id));
                    ().to_variant()
                }),
                "LaunchSearch" => params.get::<(Vec<String>, u32)>().map(|_| {
                    activate(None);
                    ().to_variant()
                }),
                _ => None,
            };
            match reply {
                Some(value) => invocation.return_value(Some(&value)),
                None => invocation.return_error(
                    gio::DBusError::InvalidArgs,
                    &format!("Invalid call to {method}"),
                ),
            }
        })
        .build()?;
    Ok(())
}

fn ids(entries: Vec<LaunchEntry>) -> Vec<String> {
    entries
        .into_iter()
        .take(MAX_RESULTS)
        .map(|e| e.target.to_string())
        .collect()
}

fn metas(index: &SearchIndex, identifiers: &[String]) -> Vec<HashMap<String, glib::Variant>> {
    let entries = index.load();
    identifiers
        .iter()
        .filter_map(|id| entries.iter().find(|e| e.target.to_string() == *id))
        .map(|entry| {
            let icon = match entry.target {
                LaunchTarget::CloudPc(_) => APP_ID,
                LaunchTarget::RemotePc(_) => "preferences-desktop-remote-desktop",
                LaunchTarget::Avd(_) => "computer",
            };
            HashMap::from([
                ("id".to_string(), entry.target.to_string().to_variant()),
                ("name".to_string(), entry.name.to_variant()),
                ("description".to_string(), entry.description.to_variant()),
                ("gicon".to_string(), icon.to_variant()),
            ])
        })
        .collect()
}
