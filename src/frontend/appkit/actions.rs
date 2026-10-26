//! Item actions: Cloud PC remote actions (start, stop, restart, rename, restore, reset,
//! troubleshoot), information sheets, and managing saved Remote PCs (add, edit, remove,
//! import, export, forget password).

use super::app::App;
use super::forms::Form;
use super::util::{self, ns};
use crate::api::Snapshot;
use crate::connections::SavedConnection;
use crate::frontend::common::library::{Item, ItemAction};
use crate::frontend::common::text;
use crate::launcher::LaunchTarget;
use crate::models::{CloudPc, CloudPcAction};
use crate::placement::PlacementStore;
use objc2::rc::Retained;
use objc2::Message;
use objc2_app_kit::{NSModalResponseOK, NSOpenPanel, NSPopUpButton, NSSavePanel, NSTextField};
use objc2_foundation::{NSArray, NSSize};
use std::future::Future;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

impl App {
    /// Runs a menu action of `item`.
    pub(super) fn item_action(self: &Rc<Self>, item: Item, action: ItemAction) {
        match (action, item) {
            (ItemAction::Connect, item) => self.connect_item(item),
            (ItemAction::ConnectionSettings, item) => {
                super::forms::connection_settings(self, item.target(), item.title());
            }
            (ItemAction::Preferences, _) => super::forms::show_settings(self),
            (ItemAction::Start, Item::CloudPc(pc)) => self.set_power(pc, true),
            (ItemAction::Stop, Item::CloudPc(pc)) => self.set_power(pc, false),
            (ItemAction::Restart, Item::CloudPc(pc)) => self.restart(pc),
            (ItemAction::Rename, Item::CloudPc(pc)) => self.rename(pc),
            (ItemAction::OpenInBrowser, Item::CloudPc(pc)) => {
                if let Some(url) = &pc.direct_launch_url {
                    util::open_url(url);
                }
            }
            (ItemAction::Troubleshoot, Item::CloudPc(pc)) => self.troubleshoot(pc),
            (ItemAction::Inspect, Item::CloudPc(pc)) => {
                super::dialogs::properties(
                    self,
                    &format!("Connection - {}", pc.display_name),
                    &text::inspect_rows(&pc),
                );
            }
            (ItemAction::Details, Item::CloudPc(pc)) => {
                super::dialogs::properties(
                    self,
                    &format!("Details - {}", pc.display_name),
                    &text::detail_rows(&pc),
                );
            }
            (ItemAction::Restore, Item::CloudPc(pc)) => self.restore(pc),
            (ItemAction::Reset, Item::CloudPc(pc)) => self.reset(pc),
            (ItemAction::Edit, Item::Remote(connection)) => self.edit_remote_pc(Some(connection)),
            (ItemAction::Export, Item::Remote(connection)) => self.export_remote_pc(connection),
            (ItemAction::ForgetPassword, Item::Remote(mut connection)) => {
                super::connect::forget_secrets(&connection.id);
                connection.remember_password = false;
                self.save_remote_pc(connection);
                self.set_status("The saved password was removed.");
            }
            (ItemAction::Remove, Item::Remote(connection)) => self.remove_remote_pc(connection),
            (action, item) => log::debug!("{action:?} does not apply to {}", item.title()),
        }
    }

    // ----- Cloud PC actions -----

    /// Runs a remote action on `pc`, marking it busy meanwhile, and reloads so the new
    /// state shows.
    fn run_pc_action<F>(self: &Rc<Self>, pc: &CloudPc, action: CloudPcAction, request: F)
    where
        F: Future<Output = crate::Result<()>> + Send + 'static,
    {
        let name = pc.display_name.clone();
        let id = pc.id.clone();
        self.set_busy(&id, true);
        self.set_status(&format!("{} {name}…", action.progress_label()));
        let app = Rc::clone(self);
        util::spawn(request, move |result| {
            app.set_busy(&id, false);
            match result {
                Ok(()) => {
                    app.load();
                    app.set_status(&format!(
                        "{} {name} — this can take a few minutes.",
                        action.progress_label()
                    ));
                }
                Err(e) => {
                    log::error!("{action:?} on {name} failed: {e}");
                    app.show_error(&format!("Could not finish on {name}"), &e.to_string());
                }
            }
        });
    }

    fn set_power(self: &Rc<Self>, pc: CloudPc, start: bool) {
        let service = Arc::clone(&self.service);
        let id = pc.id.clone();
        let action = if start {
            CloudPcAction::Start
        } else {
            CloudPcAction::Stop
        };
        self.run_pc_action(
            &pc,
            action,
            async move { service.set_power(&id, start).await },
        );
    }

    fn restart(self: &Rc<Self>, pc: CloudPc) {
        let alert = util::alert(
            self.mtm,
            &format!("Restart {}?", pc.display_name),
            text::RESTART_BODY,
            &["Cancel", "Restart"],
        );
        util::destructive(&alert, 1);
        let app = Rc::clone(self);
        util::present(&alert, self.sheet_parent().as_deref(), move |index| {
            if index != 1 {
                return;
            }
            let service = Arc::clone(&app.service);
            let id = pc.id.clone();
            app.run_pc_action(&pc, CloudPcAction::Restart, async move {
                service.restart_cloud_pc(&id).await
            });
        });
    }

    fn troubleshoot(self: &Rc<Self>, pc: CloudPc) {
        if self.is_busy(&pc.id) {
            return;
        }
        self.set_busy(&pc.id, true);
        self.set_status(&format!(
            "Troubleshooting {} — this can take several minutes…",
            pc.display_name
        ));
        let service = Arc::clone(&self.service);
        let id = pc.id.clone();
        let app = Rc::clone(self);
        util::spawn(
            async move { service.troubleshoot_cloud_pc(&id).await },
            move |outcome| {
                app.set_busy(&pc.id, false);
                let (heading, body) = text::troubleshoot_report(&pc.display_name, &outcome);
                app.set_status(&heading);
                util::inform(app.mtm, app.sheet_parent().as_deref(), &heading, &body);
                app.load();
            },
        );
    }

    fn restore(self: &Rc<Self>, pc: CloudPc) {
        self.set_status(&format!("Loading restore points of {}…", pc.display_name));
        let service = Arc::clone(&self.service);
        let id = pc.id.clone();
        let app = Rc::clone(self);
        util::spawn(
            async move { service.list_snapshots(&id).await },
            move |snapshots| match snapshots {
                Ok(list) if list.is_empty() => app.show_error(
                    &format!("No restore points for {}", pc.display_name),
                    "Restore points are created automatically; none are available yet.",
                ),
                Ok(list) => app.confirm_restore(pc, list),
                Err(e) => app.show_error(
                    &format!("Could not load restore points of {}", pc.display_name),
                    &e.to_string(),
                ),
            },
        );
    }

    fn confirm_restore(self: &Rc<Self>, pc: CloudPc, snapshots: Vec<Snapshot>) {
        self.set_status("Ready");
        let mtm = self.mtm;
        let alert = util::alert(
            mtm,
            &format!("Restore {}?", pc.display_name),
            text::RESTORE_BODY,
            &["Cancel", "Restore"],
        );
        util::destructive(&alert, 1);
        let popup = NSPopUpButton::new(mtm);
        for snapshot in &snapshots {
            popup.addItemWithTitle(&ns(&text::snapshot_label(snapshot)));
        }
        let mut form = Form::new(mtm);
        form.row("Restore point:", &popup);
        let grid = form.build();
        grid.setFrameSize(NSSize::new(320.0, grid.fittingSize().height));
        alert.setAccessoryView(Some(&grid));
        let app = Rc::clone(self);
        util::present(&alert, self.sheet_parent().as_deref(), move |index| {
            if index != 1 {
                return;
            }
            let selected = usize::try_from(popup.indexOfSelectedItem()).ok();
            let Some(snapshot) = selected.and_then(|i| snapshots.get(i)) else {
                return;
            };
            let service = Arc::clone(&app.service);
            let (id, snapshot_id) = (pc.id.clone(), snapshot.id.clone());
            app.run_pc_action(&pc, CloudPcAction::Restore, async move {
                service.restore_cloud_pc(&id, &snapshot_id).await
            });
        });
    }

    /// Rebuilds the Cloud PC after the name is typed as confirmation.
    fn reset(self: &Rc<Self>, pc: CloudPc) {
        let mtm = self.mtm;
        let alert = util::alert(
            mtm,
            &format!("Reset {}?", pc.display_name),
            text::RESET_BODY,
            &["Cancel", "Reset"],
        );
        util::destructive(&alert, 1);
        let field = Form::text_field(mtm, "", &pc.display_name);
        let prompt = util::label(mtm, &format!("Type “{}” to confirm", pc.display_name));
        let mut form = Form::new(mtm);
        form.full_row(&prompt);
        form.full_row(&field);
        let grid = form.build();
        grid.setFrameSize(NSSize::new(300.0, grid.fittingSize().height));
        alert.setAccessoryView(Some(&grid));
        let buttons = alert.buttons();
        if buttons.count() > 1 {
            let reset_button = buttons.objectAtIndex(1);
            reset_button.setEnabled(false);
            let expected = pc.display_name.trim().to_string();
            util::watch_text(
                &field,
                move |text| reset_button.setEnabled(text.trim() == expected),
                None,
            );
        }
        alert.window().setInitialFirstResponder(Some(&field));
        let app = Rc::clone(self);
        util::present(&alert, self.sheet_parent().as_deref(), move |index| {
            if index != 1 || field.stringValue().to_string().trim() != pc.display_name.trim() {
                return;
            }
            let service = Arc::clone(&app.service);
            let id = pc.id.clone();
            app.run_pc_action(&pc, CloudPcAction::Reset, async move {
                service.reset_cloud_pc(&id).await
            });
        });
    }

    fn rename(self: &Rc<Self>, pc: CloudPc) {
        let mtm = self.mtm;
        let alert = util::alert(
            mtm,
            "Rename Cloud PC",
            "Enter a new display name:",
            &["Rename", "Cancel"],
        );
        let field: Retained<NSTextField> = Form::text_field(mtm, &pc.display_name, "");
        field.setFrameSize(NSSize::new(260.0, field.fittingSize().height));
        alert.setAccessoryView(Some(&field));
        alert.window().setInitialFirstResponder(Some(&field));
        let app = Rc::clone(self);
        util::present(&alert, self.sheet_parent().as_deref(), move |index| {
            let new_name = field.stringValue().to_string().trim().to_string();
            if index != 0 || new_name.is_empty() || new_name == pc.display_name {
                return;
            }
            let service = Arc::clone(&app.service);
            let id = pc.id.clone();
            let app2 = Rc::clone(&app);
            util::spawn(
                async move { service.rename_cloud_pc(&id, &new_name).await },
                move |result| match result {
                    Ok(()) => app2.load(),
                    Err(e) => app2.set_status(&format!("Rename failed: {e}")),
                },
            );
        });
    }

    // ----- Remote PCs -----

    /// The add (`None`) or edit form of a Remote PC.
    pub(super) fn edit_remote_pc(self: &Rc<Self>, connection: Option<SavedConnection>) {
        self.present();
        super::forms::remote_pc(self, connection);
    }

    /// Updates a saved connection quietly (e.g. the "remember password" choice).
    pub(super) fn store_remote_pc(&self, connection: SavedConnection) {
        match self.connections.upsert(connection) {
            Ok(list) => {
                self.library.borrow_mut().remote_pcs = list;
                self.refresh();
            }
            Err(e) => log::warn!("Could not update the saved connection: {e}"),
        }
    }

    pub(super) fn save_remote_pc(&self, connection: SavedConnection) {
        let name = connection.display_name().to_string();
        match self.connections.upsert(connection) {
            Ok(list) => {
                self.library.borrow_mut().remote_pcs = list;
                self.set_status(&format!("Saved {name}."));
            }
            Err(e) => {
                log::error!("Saving {} failed: {e}", self.connections.path().display());
                self.set_status(&format!("Could not save {name}: {e}"));
            }
        }
        self.refresh();
    }

    fn remove_remote_pc(self: &Rc<Self>, connection: SavedConnection) {
        let alert = util::alert(
            self.mtm,
            &format!("Remove {}?", connection.display_name()),
            "The saved connection is removed from this computer.",
            &["Cancel", "Remove"],
        );
        util::destructive(&alert, 1);
        let app = Rc::clone(self);
        util::present(&alert, self.sheet_parent().as_deref(), move |index| {
            if index != 1 {
                return;
            }
            let name = connection.display_name();
            match app.connections.remove(&connection.id) {
                Ok(list) => {
                    app.library.borrow_mut().remote_pcs = list;
                    app.set_status(&format!("Removed {name}."));
                    let target = LaunchTarget::RemotePc(connection.id.clone());
                    crate::platform::remove_desktop_integration(&target);
                    super::connect::forget_secrets(&connection.id);
                    if let Err(e) = app.overrides.remove(&target) {
                        log::warn!("Could not remove the connection settings: {e}");
                    }
                    if let Err(e) = app.recent.remove(&target) {
                        log::warn!("Could not remove the connection history: {e}");
                    }
                    if let Err(e) = PlacementStore::default().remove(&target) {
                        log::warn!("Could not remove the session window placement: {e}");
                    }
                }
                Err(e) => app.set_status(&format!("Could not remove {name}: {e}")),
            }
            app.refresh();
        });
    }

    /// Saves a Remote PC with its settings as a `.rdp` file (without the password).
    fn export_remote_pc(self: &Rc<Self>, connection: SavedConnection) {
        let panel = NSSavePanel::savePanel(self.mtm);
        panel.setTitle(Some(&ns("Export RDP File")));
        panel.setNameFieldStringValue(&ns(&format!(
            "{}.rdp",
            text::sanitize_file_name(connection.display_name())
        )));
        restrict_to_rdp(&panel);
        let app = Rc::clone(self);
        run_panel(&panel, self.sheet_parent().as_deref(), move |urls| {
            let Some(path) = urls.into_iter().next() else {
                return;
            };
            let overrides = app
                .overrides
                .get(&LaunchTarget::RemotePc(connection.id.clone()));
            let contents = crate::rdp_file::export(&connection, &overrides);
            match std::fs::write(&path, contents) {
                Ok(()) => app.set_status(&format!(
                    "Exported {} to {}.",
                    connection.display_name(),
                    path.display()
                )),
                Err(e) => app.set_status(&format!(
                    "Could not export {}: {e}",
                    connection.display_name()
                )),
            }
        });
    }

    /// Imports a `.rdp` file chosen by the user as a new Remote PC.
    pub(super) fn import_rdp_file(self: &Rc<Self>) {
        self.present();
        let panel = NSOpenPanel::openPanel(self.mtm);
        panel.setTitle(Some(&ns("Import RDP File")));
        panel.setCanChooseDirectories(false);
        panel.setCanChooseFiles(true);
        panel.setAllowsMultipleSelection(false);
        restrict_to_rdp(&panel);
        let app = Rc::clone(self);
        run_panel(&panel, self.sheet_parent().as_deref(), move |urls| {
            let Some(path) = urls.into_iter().next() else {
                return;
            };
            match text::read_rdp_file(&path) {
                Ok(imported) => {
                    let target = LaunchTarget::RemotePc(imported.connection.id.clone());
                    if let Err(e) = app.overrides.set(&target, imported.overrides) {
                        log::warn!("Could not save the imported settings: {e}");
                    }
                    app.save_remote_pc(imported.connection);
                }
                Err(message) => app.show_error("Could not import the file", &message),
            }
        });
    }
}

fn restrict_to_rdp(panel: &NSSavePanel) {
    let types = NSArray::from_retained_slice(&[ns("rdp")]);
    // `allowedContentTypes` needs UniformTypeIdentifiers, which this crate does not link;
    // the extension list still works on current macOS.
    #[allow(deprecated)]
    panel.setAllowedFileTypes(Some(&types));
}

/// Runs a save or open panel (as a sheet when `parent` is visible) and passes the chosen
/// paths to `done` (nothing when cancelled).
fn run_panel(
    panel: &NSSavePanel,
    parent: Option<&objc2_app_kit::NSWindow>,
    done: impl FnOnce(Vec<PathBuf>) + 'static,
) {
    let chosen = {
        let panel = panel.retain();
        move |response| {
            if response != NSModalResponseOK {
                return Vec::new();
            }
            match panel.downcast_ref::<NSOpenPanel>() {
                Some(open) => open
                    .URLs()
                    .iter()
                    .filter_map(|url| url.path().map(|p| PathBuf::from(p.to_string())))
                    .collect(),
                None => panel
                    .URL()
                    .and_then(|url| url.path())
                    .map(|p| vec![PathBuf::from(p.to_string())])
                    .unwrap_or_default(),
            }
        }
    };
    match parent {
        Some(parent) => {
            let done = std::cell::Cell::new(Some(done));
            let handler = block2::RcBlock::new(move |response| {
                if let Some(done) = done.take() {
                    done(chosen(response));
                }
            });
            panel.beginSheetModalForWindow_completionHandler(parent, &handler);
        }
        None => {
            let response = panel.runModal();
            done(chosen(response));
        }
    }
}
