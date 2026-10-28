//! Add / edit dialog for a Remote PC: address, name and username, plus the advanced
//! settings (domain, group, gateway, administrative session, RemoteApp, security).

use crate::connections::{
    AddressError, AuthMethod, CertificatePolicy, Gateway, RemoteApp, SavedConnection,
    SecurityProtocol,
};
use adw::prelude::*;
use gtk4::{Button, Label, StringList};
use std::rc::Rc;

/// Shows the dialog over `parent`; `on_save` receives the validated connection.
pub fn show(
    parent: &impl IsA<gtk4::Widget>,
    existing: Option<SavedConnection>,
    on_save: impl Fn(SavedConnection) + 'static,
) {
    let editing = existing.is_some();
    let base = existing.unwrap_or_else(SavedConnection::blank);

    let dialog = adw::Dialog::builder()
        .title(if editing {
            "Edit Remote PC"
        } else {
            "Add Remote PC"
        })
        .content_width(520)
        .content_height(640)
        .build();

    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .build();
    let cancel_btn = Button::with_label("Cancel");
    header.pack_start(&cancel_btn);
    let save_btn = Button::with_label(if editing { "Save" } else { "Add" });
    save_btn.add_css_class("suggested-action");
    header.pack_end(&save_btn);

    let entry = |title: &str, text: &str| {
        adw::EntryRow::builder()
            .title(title)
            .text(text)
            .activates_default(true)
            .build()
    };

    // Basic settings.
    let address_row = entry("PC name or IP address", &base.address);
    let name_row = entry("Display name (optional)", &base.name);
    let username_row = entry("Username (optional)", &base.username);
    let basic = adw::PreferencesGroup::builder()
        .description(
            "Examples: office-pc, pc01.contoso.com:3390, 192.168.1.20. Usernames can be \
             written as DOMAIN\\user or user@domain.",
        )
        .build();
    basic.add(&address_row);
    basic.add(&name_row);
    basic.add(&username_row);

    let error_label = Label::new(None);
    error_label.add_css_class("error");
    error_label.set_halign(gtk4::Align::Start);
    error_label.set_wrap(true);
    error_label.set_visible(false);
    basic.add(&error_label);

    // Organisation and sign-in.
    let domain_row = entry("Domain (optional)", &base.domain);
    let group_row = entry("Folder / Group (optional)", base.folder_or_group());
    let tags_row = entry("Tags (comma separated)", &base.tags.join(", "));
    let favorite_row = adw::SwitchRow::builder()
        .title("Favorite")
        .subtitle("Star this connection for quick access")
        .active(base.is_favorite)
        .build();
    let remember_row = adw::SwitchRow::builder()
        .title("Remember Password")
        .subtitle("Keep the password in the system keyring")
        .active(base.remember_password)
        .build();
    let sign_in = adw::PreferencesGroup::builder()
        .title("Sign-in & Organization")
        .build();
    sign_in.add(&domain_row);
    sign_in.add(&group_row);
    sign_in.add(&tags_row);
    sign_in.add(&favorite_row);
    sign_in.add(&remember_row);

    // Gateway.
    let gateway = base.gateway.clone().unwrap_or_default();
    let gateway_row = adw::ExpanderRow::builder()
        .title("Remote Desktop Gateway")
        .subtitle("Reach the PC through an RD Gateway")
        .show_enable_switch(true)
        .enable_expansion(base.gateway.is_some())
        .expanded(base.gateway.is_some())
        .build();
    let gateway_address_row = entry("Gateway address (port 443 if omitted)", &gateway.address);
    let gateway_separate_row = adw::SwitchRow::builder()
        .title("Separate Gateway Credentials")
        .subtitle("Otherwise the PC's username and password are used")
        .active(gateway.separate_credentials)
        .build();
    let gateway_user_row = entry("Gateway username", &gateway.username);
    gateway_user_row.set_sensitive(gateway.separate_credentials);
    gateway_row.add_row(&gateway_address_row);
    gateway_row.add_row(&gateway_separate_row);
    gateway_row.add_row(&gateway_user_row);

    // RemoteApp.
    let app = base.remote_app.clone().unwrap_or_default();
    let app_row = adw::ExpanderRow::builder()
        .title("RemoteApp")
        .subtitle("Start a single program instead of the desktop")
        .show_enable_switch(true)
        .enable_expansion(base.remote_app.is_some())
        .expanded(base.remote_app.is_some())
        .build();
    let app_program_row = entry("Program (path or ||alias)", &app.program);
    let app_args_row = entry("Arguments (optional)", &app.arguments);
    app_row.add_row(&app_program_row);
    app_row.add_row(&app_args_row);

    let admin_row = adw::SwitchRow::builder()
        .title("Administrative Session")
        .subtitle("Connect to the console session")
        .active(base.admin_session)
        .build();
    let session = adw::PreferencesGroup::builder().title("Session").build();
    session.add(&gateway_row);
    session.add(&app_row);
    session.add(&admin_row);

    // Security.
    let security_row = combo(
        "Security",
        SecurityProtocol::ALL.map(SecurityProtocol::label),
        index_of(&SecurityProtocol::ALL, base.security),
    );
    let certificate_row = combo(
        "Certificates",
        CertificatePolicy::ALL.map(CertificatePolicy::label),
        index_of(&CertificatePolicy::ALL, base.certificate),
    );
    let auth_row = combo(
        "Authentication",
        AuthMethod::ALL.map(AuthMethod::label),
        index_of(&AuthMethod::ALL, base.auth),
    );
    auth_row.set_subtitle("Kerberos needs a ticket, e.g. from kinit");
    let security = adw::PreferencesGroup::builder().title("Security").build();
    security.add(&security_row);
    security.add(&certificate_row);
    security.add(&auth_row);

    let page = adw::PreferencesPage::new();
    for group in [&basic, &sign_in, &session, &security] {
        page.add(group);
    }

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&page));
    dialog.set_child(Some(&toolbar_view));
    dialog.set_default_widget(Some(&save_btn));
    dialog.set_focus(Some(&address_row));

    // Builds the connection from the form.
    let collect = {
        let base = base.clone();
        let rows = (
            address_row.clone(),
            name_row.clone(),
            username_row.clone(),
            domain_row.clone(),
            group_row.clone(),
            tags_row.clone(),
            favorite_row.clone(),
            remember_row.clone(),
        );
        let gateway_rows = (
            gateway_row.clone(),
            gateway_address_row.clone(),
            gateway_separate_row.clone(),
            gateway_user_row.clone(),
        );
        let app_rows = (
            app_row.clone(),
            app_program_row.clone(),
            app_args_row.clone(),
        );
        let combos = (
            admin_row.clone(),
            security_row.clone(),
            certificate_row.clone(),
            auth_row.clone(),
        );
        Rc::new(move || -> Result<SavedConnection, AddressError> {
            let (address, name, username, domain, group, tags, fav, remember) = &rows;
            let mut connection = base.clone();
            connection.update(&name.text(), &address.text(), &username.text())?;
            connection.domain = domain.text().to_string();
            connection.group = group.text().to_string();
            connection.folder = Some(group.text().to_string()).filter(|s| !s.is_empty());
            connection.tags = tags
                .text()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            connection.is_favorite = fav.is_active();
            connection.remember_password = remember.is_active();
            let (gw, gw_address, gw_separate, gw_user) = &gateway_rows;
            connection.gateway = gw.enables_expansion().then(|| Gateway {
                address: gw_address.text().to_string(),
                username: gw_user.text().to_string(),
                separate_credentials: gw_separate.is_active(),
            });
            let (app, program, arguments) = &app_rows;
            connection.remote_app = app.enables_expansion().then(|| RemoteApp {
                program: program.text().to_string(),
                arguments: arguments.text().to_string(),
            });
            let (admin, security, certificate, auth) = &combos;
            connection.admin_session = admin.is_active();
            connection.security = pick(&SecurityProtocol::ALL, security.selected());
            connection.certificate = pick(&CertificatePolicy::ALL, certificate.selected());
            connection.auth = pick(&AuthMethod::ALL, auth.selected());
            connection.validate()?;
            Ok(connection)
        })
    };

    let validate = {
        let collect = Rc::clone(&collect);
        let save_btn = save_btn.clone();
        let error_label = error_label.clone();
        let address_row = address_row.clone();
        Rc::new(move || {
            let result = collect();
            save_btn.set_sensitive(result.is_ok());
            // Nothing typed yet is not an error worth showing.
            let message = result
                .err()
                .filter(|e| *e != AddressError::Empty)
                .map(|e| e.to_string());
            if message.is_some() {
                address_row.add_css_class("error");
            } else {
                address_row.remove_css_class("error");
            }
            error_label.set_visible(message.is_some());
            error_label.set_text(message.as_deref().unwrap_or_default());
        })
    };
    validate();
    for row in [
        &address_row,
        &name_row,
        &username_row,
        &domain_row,
        &group_row,
        &gateway_address_row,
        &gateway_user_row,
        &app_program_row,
        &app_args_row,
    ] {
        let validate = Rc::clone(&validate);
        row.connect_changed(move |_| validate());
    }
    for row in [&gateway_row, &app_row] {
        let validate = Rc::clone(&validate);
        row.connect_enable_expansion_notify(move |_| validate());
    }
    {
        let validate = Rc::clone(&validate);
        let user_row = gateway_user_row.clone();
        gateway_separate_row.connect_active_notify(move |row| {
            user_row.set_sensitive(row.is_active());
            validate();
        });
    }

    let dlg = dialog.downgrade();
    cancel_btn.connect_clicked(move |_| {
        if let Some(dialog) = dlg.upgrade() {
            dialog.close();
        }
    });

    let dlg = dialog.downgrade();
    save_btn.connect_clicked(move |_| {
        let Ok(connection) = collect() else { return };
        on_save(connection);
        if let Some(dialog) = dlg.upgrade() {
            dialog.close();
        }
    });
    dialog.present(Some(parent));
}

pub(super) fn combo<const N: usize>(
    title: &str,
    labels: [&str; N],
    selected: u32,
) -> adw::ComboRow {
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&StringList::new(&labels))
        .build();
    row.set_selected(selected);
    row
}

fn index_of<T: PartialEq>(all: &[T], value: T) -> u32 {
    all.iter().position(|v| *v == value).unwrap_or(0) as u32
}

fn pick<T: Copy + Default>(all: &[T], index: u32) -> T {
    all.get(index as usize).copied().unwrap_or_default()
}
