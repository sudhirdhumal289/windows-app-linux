//! Connecting: Cloud PCs, Azure Virtual Desktop resources and Remote PCs (with the
//! credentials prompt), quick connect from the search field and opened `.rdp` files.

use super::app::App;
use super::forms::Form;
use super::util::{self, ns};
use crate::api::AvdResource;
use crate::connections::SavedConnection;
use crate::frontend::common::launch::{self, DisplayInfo, Launch};
use crate::frontend::common::library::{Item, SearchFilter};
use crate::frontend::common::text;
use crate::launcher::LaunchTarget;
use crate::models::CloudPc;
use crate::overrides::ConnectionOverrides;
use crate::platform::secrets::{self, SecretKey};
use crate::session::{Credentials, PreparedSession};
use objc2_app_kit::{NSScreen, NSSecureTextField, NSTextField};
use objc2_foundation::NSSize;
use std::rc::Rc;
use std::sync::Arc;

/// The main display, for the initial session size and scale.
pub(super) fn display_info(app: &App) -> DisplayInfo {
    let Some(screen) = NSScreen::mainScreen(app.mtm) else {
        return DisplayInfo::default();
    };
    let scale = screen.backingScaleFactor();
    let frame = screen.frame();
    let to_device = |v: f64| (v * scale).round() as u32;
    let size = (to_device(frame.size.width), to_device(frame.size.height));
    DisplayInfo {
        device_size: (size.0 > 0 && size.1 > 0).then_some(size),
        scale_percent: Some((scale * 100.0).round() as u32),
    }
}

impl App {
    /// Connects to `target` now, or once the library has loaded.
    pub(super) fn connect_target(self: &Rc<Self>, target: LaunchTarget) {
        let item = self.library.borrow().find(&target);
        match (item, &target) {
            (Some(item), _) => self.connect_item(item),
            (None, LaunchTarget::RemotePc(_)) => self.report_missing(&target),
            (None, _) if self.is_loading() => *self.pending_target.borrow_mut() = Some(target),
            (None, _) if self.signed_in() => {
                *self.pending_target.borrow_mut() = Some(target);
                self.load();
            }
            (None, _) => self.set_status("Sign in to connect to your Cloud PCs."),
        }
    }

    pub(super) fn connect_item(self: &Rc<Self>, item: Item) {
        match item {
            Item::CloudPc(pc) => self.connect_cloud_pc(pc),
            Item::Avd(resource) => self.connect_avd(resource),
            Item::Remote(connection) => self.connect_saved_remote(connection),
        }
    }

    fn connect_cloud_pc(self: &Rc<Self>, pc: CloudPc) {
        if self.is_busy(&pc.id) {
            return;
        }
        self.set_busy(&pc.id, true);
        self.set_status(&format!("Preparing connection to {}…", pc.display_name));
        let service = Arc::clone(&self.service);
        let display = display_info(self);
        let id = pc.id.clone();
        let app = Rc::clone(self);
        util::spawn(
            async move { launch::prepare_cloud_pc(&service, &pc, display).await },
            move |prepared| {
                app.set_busy(&id, false);
                app.finish_prepare(prepared);
            },
        );
    }

    fn connect_avd(self: &Rc<Self>, resource: AvdResource) {
        if self.is_busy(&resource.id) {
            return;
        }
        self.set_busy(&resource.id, true);
        self.set_status(&format!("Preparing connection to {}…", resource.title));
        let service = Arc::clone(&self.service);
        let display = display_info(self);
        let id = resource.id.clone();
        let app = Rc::clone(self);
        util::spawn(
            async move { launch::prepare_avd(&service, &resource, display).await },
            move |prepared| {
                app.set_busy(&id, false);
                app.finish_prepare(prepared);
            },
        );
    }

    fn finish_prepare(self: &Rc<Self>, prepared: Result<launch::Prepared, String>) {
        match prepared {
            Ok((launch, prepared)) => self.start_session(launch, prepared),
            Err(e) => {
                log::error!("Preparing the connection failed: {e}");
                self.set_status(&format!("Could not connect: {e}"));
            }
        }
    }

    /// Runs the command before connecting (if any), then opens the session window.
    pub(super) fn start_session(self: &Rc<Self>, launch: Launch, prepared: PreparedSession) {
        if launch.overrides.pre_command.is_empty() {
            self.open_session(launch, prepared);
            return;
        }
        self.set_status(&format!(
            "Running the command before connecting to {}…",
            launch.name
        ));
        let app = Rc::clone(self);
        util::spawn(
            async move {
                let result = launch::run_pre_command(&launch).await;
                (launch, result)
            },
            move |(launch, result)| match result {
                Ok(()) => app.open_session(launch, prepared),
                Err(message) => app.set_status(&message),
            },
        );
    }

    fn open_session(self: &Rc<Self>, launch: Launch, prepared: PreparedSession) {
        let name = launch.name.clone();
        self.set_status(&format!("Connecting to {name}…"));
        match super::session_window::open(self.mtm, launch, prepared) {
            Ok(()) => self.refresh(),
            Err(e) => self.show_error(&format!("Could not connect to {name}"), &e),
        }
    }

    /// Called by the session window when a session is over.
    pub(super) fn session_ended(&self, name: &str) {
        self.set_status(&format!("Session with {name} ended."));
        self.refresh();
    }

    pub(super) fn connect_saved_remote(self: &Rc<Self>, connection: SavedConnection) {
        let overrides = self
            .overrides
            .get(&LaunchTarget::RemotePc(connection.id.clone()));
        self.connect_remote(connection, true, overrides);
    }

    /// Signs in (saved password or prompt) and connects. `saved`: the connection is in
    /// the library (it has history and can remember passwords).
    pub(super) fn connect_remote(
        self: &Rc<Self>,
        connection: SavedConnection,
        saved: bool,
        overrides: ConnectionOverrides,
    ) {
        let remembered = connection.remember_password;
        let app = Rc::clone(self);
        obtain_credentials(self, connection, saved, move |connection, credentials| {
            if saved && connection.remember_password != remembered {
                app.store_remote_pc(connection.clone());
            }
            let display = display_info(&app);
            let prepared = if saved {
                launch::prepare_remote(&connection, &credentials, true, display)
            } else {
                prepare_unsaved(&connection, &credentials, overrides, display)
            };
            app.finish_prepare(prepared);
        });
    }

    /// Enter in the search field: connect to the only match, or quick-connect to the
    /// typed address.
    pub(super) fn connect_from_search(self: &Rc<Self>, text: &str) {
        if SearchFilter::new(text).is_empty() {
            return;
        }
        let cards: Vec<Item> = self
            .sections()
            .into_iter()
            .flat_map(|s| s.cards.into_iter().map(|c| c.item))
            .collect();
        match cards.as_slice() {
            [item] => self.connect_item(item.clone()),
            [] => match SavedConnection::new("", text, "") {
                Ok(connection) => {
                    self.connect_remote(connection, false, ConnectionOverrides::default());
                }
                Err(e) => self.set_status(&e.to_string()),
            },
            _ => {}
        }
    }

    /// Connects to the PC described by a `.rdp` file without saving it.
    pub(super) fn open_rdp_file(self: &Rc<Self>, path: &std::path::Path) {
        match text::read_rdp_file(path) {
            Ok(imported) => {
                self.connect_remote(imported.connection, false, imported.overrides);
            }
            Err(message) => self.show_error("Could not open the file", &message),
        }
    }
}

/// Like [`launch::prepare_remote`] for an unsaved connection that brings its own settings
/// (an opened `.rdp` file).
fn prepare_unsaved(
    connection: &SavedConnection,
    credentials: &Credentials,
    overrides: ConnectionOverrides,
    display: DisplayInfo,
) -> Result<launch::Prepared, String> {
    let username = Some(connection.username.clone()).filter(|u| !u.is_empty());
    let (config, options) = launch::launch_options(&overrides, display, username);
    let prepared = crate::session::prepare_direct(connection, credentials, &options)
        .map_err(|e| e.to_string())?;
    Ok((
        Launch {
            name: connection.display_name().to_string(),
            target: None,
            address: connection.address.clone(),
            login_hint: None,
            overrides,
            multi_monitor: config.multi_monitor,
            dynamic_resolution: options.dynamic_resolution,
        },
        prepared,
    ))
}

/// Obtains the passwords of `connection`: from the keychain when remembered, otherwise
/// by asking. Nothing happens when the user cancels.
fn obtain_credentials(
    app: &Rc<App>,
    connection: SavedConnection,
    can_remember: bool,
    on_ready: impl FnOnce(SavedConnection, Credentials) + 'static,
) {
    if !(can_remember && connection.remember_password) {
        prompt(
            app,
            connection,
            can_remember,
            Credentials::default(),
            on_ready,
        );
        return;
    }
    let id = connection.id.clone();
    let needs_gateway = connection.gateway_needs_password();
    let app = Rc::clone(app);
    util::spawn_blocking(
        move || -> Result<Credentials, secrets::SecretError> {
            Ok(Credentials {
                password: secrets::load_password(&SecretKey::password(&id))?,
                gateway_password: if needs_gateway {
                    secrets::load_password(&SecretKey::gateway(&id))?
                } else {
                    None
                },
            })
        },
        move |stored| {
            let stored = stored.unwrap_or_else(|e| {
                log::warn!("Could not read the saved password: {e}");
                Credentials::default()
            });
            let complete =
                stored.password.is_some() && (!needs_gateway || stored.gateway_password.is_some());
            if complete {
                on_ready(connection, stored);
            } else {
                prompt(&app, connection, true, stored, on_ready);
            }
        },
    );
}

/// Asks for the username and passwords, pre-filled with `known`.
fn prompt(
    app: &Rc<App>,
    connection: SavedConnection,
    can_remember: bool,
    known: Credentials,
    on_ready: impl FnOnce(SavedConnection, Credentials) + 'static,
) {
    let mtm = app.mtm;
    let body = if can_remember {
        format!("Sign in to {}.", connection.address)
    } else {
        format!(
            "Sign in to {}. The password is used for this session only.",
            connection.address
        )
    };
    let alert = util::alert(
        mtm,
        &format!("Connect to {}", connection.display_name()),
        &body,
        &["Connect", "Cancel"],
    );
    let username = Form::text_field(mtm, &connection.username, "Username");
    let password = secure_field(mtm, known.password.as_deref().unwrap_or_default());
    let gateway = secure_field(mtm, known.gateway_password.as_deref().unwrap_or_default());
    let remember = util::checkbox(mtm, "Remember Password", connection.remember_password);
    let mut form = Form::new(mtm);
    form.row("Username:", &username);
    form.row("Password:", &password);
    if connection.gateway_needs_password() {
        form.row("Gateway password:", &gateway);
    }
    if can_remember {
        form.row("", &remember);
    }
    let grid = form.build();
    grid.setFrameSize(NSSize::new(320.0, grid.fittingSize().height));
    alert.setAccessoryView(Some(&grid));
    alert.layout();
    let first: &NSTextField = if connection.username.is_empty() {
        &username
    } else {
        &password
    };
    alert.window().setInitialFirstResponder(Some(first));

    let parent = app.sheet_parent();
    util::present(&alert, parent.as_deref(), move |index| {
        if index != 0 {
            return;
        }
        let mut connection = connection;
        connection.username = username.stringValue().to_string().trim().to_string();
        let was_remembered = connection.remember_password;
        connection.remember_password = can_remember && util::is_checked(&remember);
        let non_empty =
            |field: &NSTextField| Some(field.stringValue().to_string()).filter(|t| !t.is_empty());
        let credentials = Credentials {
            password: non_empty(&password),
            gateway_password: non_empty(&gateway).filter(|_| connection.gateway_needs_password()),
        };
        password.setStringValue(&ns(""));
        gateway.setStringValue(&ns(""));
        if connection.remember_password {
            store_secrets(&connection.id, &credentials);
        } else if was_remembered {
            forget_secrets(&connection.id);
        }
        on_ready(connection, credentials);
    });
}

fn secure_field(
    mtm: objc2::MainThreadMarker,
    text: &str,
) -> objc2::rc::Retained<NSSecureTextField> {
    let field = NSSecureTextField::new(mtm);
    field.setStringValue(&ns(text));
    field
}

/// Saves the passwords in the keychain (in the background).
fn store_secrets(id: &str, credentials: &Credentials) {
    let id = id.to_string();
    let credentials = credentials.clone();
    util::spawn_blocking(
        move || {
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
        },
        |()| {},
    );
}

/// Removes the saved passwords of connection `id` (in the background).
pub(super) fn forget_secrets(id: &str) {
    let id = id.to_string();
    util::spawn_blocking(move || secrets::delete_connection_secrets(&id), |()| {});
}
