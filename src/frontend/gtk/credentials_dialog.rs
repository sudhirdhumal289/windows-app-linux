//! Sign-in for direct connections: passwords from the system keyring when remembered,
//! otherwise a prompt (which can store them for next time).

use crate::connections::SavedConnection;
use crate::platform::secrets::{self, SecretKey};
use crate::session::Credentials;
use adw::prelude::*;
use gtk4::{gio, glib, ListBox};

/// Result of a successful sign-in: the connection with the username and "remember"
/// choice entered, and the passwords.
pub struct SignIn {
    pub connection: SavedConnection,
    pub credentials: Credentials,
}

/// Obtains credentials for `connection` and passes them to `on_ready`; nothing happens
/// when the user cancels. `can_remember`: whether the connection is saved (only saved
/// connections can keep passwords).
pub fn obtain(
    parent: &impl IsA<gtk4::Widget>,
    connection: SavedConnection,
    can_remember: bool,
    on_ready: impl FnOnce(SignIn) + 'static,
) {
    if !(can_remember && connection.remember_password) {
        prompt(
            parent,
            connection,
            can_remember,
            Credentials::default(),
            on_ready,
        );
        return;
    }
    let parent = parent.clone().upcast::<gtk4::Widget>();
    glib::MainContext::default().spawn_local(async move {
        let id = connection.id.clone();
        let needs_gateway = connection.gateway_needs_password();
        let stored = gio::spawn_blocking(move || -> Result<Credentials, secrets::SecretError> {
            Ok(Credentials {
                password: secrets::load_password(&SecretKey::password(&id))?,
                gateway_password: if needs_gateway {
                    secrets::load_password(&SecretKey::gateway(&id))?
                } else {
                    None
                },
            })
        })
        .await;
        let stored = match stored {
            Ok(Ok(credentials)) => credentials,
            Ok(Err(e)) => {
                log::warn!("Could not read the saved password: {e}");
                Credentials::default()
            }
            Err(_) => Credentials::default(),
        };
        let complete =
            stored.password.is_some() && (!needs_gateway || stored.gateway_password.is_some());
        if complete {
            on_ready(SignIn {
                connection,
                credentials: stored,
            });
        } else {
            prompt(&parent, connection, true, stored, on_ready);
        }
    });
}

/// Asks for the username and passwords, pre-filled with `known`.
fn prompt(
    parent: &impl IsA<gtk4::Widget>,
    connection: SavedConnection,
    can_remember: bool,
    known: Credentials,
    on_ready: impl FnOnce(SignIn) + 'static,
) {
    let username_row = adw::EntryRow::builder()
        .title("Username")
        .text(&connection.username)
        .activates_default(true)
        .build();
    let password_row = adw::PasswordEntryRow::builder()
        .title("Password")
        .text(known.password.as_deref().unwrap_or_default())
        .activates_default(true)
        .build();
    let gateway_row = adw::PasswordEntryRow::builder()
        .title("Gateway password")
        .text(known.gateway_password.as_deref().unwrap_or_default())
        .activates_default(true)
        .visible(connection.gateway_needs_password())
        .build();
    let remember_row = adw::SwitchRow::builder()
        .title("Remember Password")
        .active(connection.remember_password)
        .visible(can_remember)
        .build();
    let fields = ListBox::new();
    fields.add_css_class("boxed-list");
    fields.set_selection_mode(gtk4::SelectionMode::None);
    fields.append(&username_row);
    fields.append(&password_row);
    fields.append(&gateway_row);
    fields.append(&remember_row);

    let body = if can_remember {
        format!("Sign in to {}.", connection.address)
    } else {
        format!(
            "Sign in to {}. The password is used for this session only.",
            connection.address
        )
    };
    let dialog = adw::AlertDialog::builder()
        .heading(format!("Connect to {}", connection.display_name()))
        .body(body)
        .extra_child(&fields)
        .default_response("connect")
        .close_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("connect", "Connect")]);
    dialog.set_response_appearance("connect", adw::ResponseAppearance::Suggested);
    if connection.username.is_empty() {
        dialog.set_focus(Some(&username_row));
    } else {
        dialog.set_focus(Some(&password_row));
    }

    let on_ready = std::cell::Cell::new(Some(on_ready));
    dialog.connect_response(Some("connect"), move |_, _| {
        let Some(on_ready) = on_ready.take() else {
            return;
        };
        let mut connection = connection.clone();
        connection.username = username_row.text().trim().to_string();
        let was_remembered = connection.remember_password;
        connection.remember_password = can_remember && remember_row.is_active();
        let non_empty = |text: glib::GString| Some(text.to_string()).filter(|t| !t.is_empty());
        let credentials = Credentials {
            password: non_empty(password_row.text()),
            gateway_password: non_empty(gateway_row.text())
                .filter(|_| connection.gateway_needs_password()),
        };
        password_row.set_text("");
        gateway_row.set_text("");
        if connection.remember_password {
            store(&connection.id, &credentials);
        } else if was_remembered {
            forget(&connection.id);
        }
        on_ready(SignIn {
            connection,
            credentials,
        });
    });
    dialog.present(Some(parent));
}

/// Saves the passwords in the keyring (in the background).
fn store(id: &str, credentials: &Credentials) {
    let id = id.to_string();
    let credentials = credentials.clone();
    gio::spawn_blocking(move || {
        let pairs = [
            (SecretKey::password(&id), credentials.password),
            (SecretKey::gateway(&id), credentials.gateway_password),
        ];
        for (key, password) in pairs {
            let result = match password {
                Some(password) => secrets::save_password(&key, &password),
                None => secrets::delete_password(&key),
            };
            if let Err(e) = result {
                log::warn!("Could not save password {key}: {e}");
            }
        }
    });
}

/// Removes the saved passwords of connection `id` (in the background).
pub fn forget(id: &str) {
    let id = id.to_string();
    gio::spawn_blocking(move || secrets::delete_connection_secrets(&id));
}
