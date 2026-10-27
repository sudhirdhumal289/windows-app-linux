//! The main window: the library of Cloud PCs, Azure Virtual Desktop resources and Remote
//! PCs, its navigation, search, menus and dialogs, and the connection flows that open
//! session windows.
//!
//! Network and keyring work runs on the thread pool (`spawn_background` with the shared
//! Tokio runtime) and comes back as a [`Msg`]. Dialogs are queued and shown one at a time.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows_reactor::*;

use super::dialogs::{self, DialogSpec, Outcome};
use super::session_window::{self, Notice, Notifier};
use super::settings::{SettingsPage, SettingsPageInput};
use super::signin::{AccountSignIn, AccountSignInInput, HostSignIn, HostSignInInput};
use super::signin::{ScaleInput, ScaleWindow};
use super::views::{self, ADD_CLOUD, ADD_IMPORT, ADD_REMOTE, NAV_ALL, NAV_SETTINGS};
use super::{block_on, win32};
use crate::api::{CloudPcService, GraphApiClient, Snapshot};
use crate::auth::{Account, AuthManager};
use crate::config::AppConfig;
use crate::connections::{self, ConnectionStore, SavedConnection};
use crate::frontend::common::launch::{self, DisplayInfo, Launch, Prepared};
use crate::frontend::common::library::{self, FeedLoad, Item, ItemAction, Library, SearchFilter};
use crate::frontend::common::text::{
    detail_rows, inspect_rows, load_error_message, load_summary, quit_prompt, read_rdp_file,
    sanitize_file_name, snapshot_label, troubleshoot_report, APP_NAME, RESET_BODY, RESTART_BODY,
    RESTORE_BODY,
};
use crate::history::RecentStore;
use crate::launcher::LaunchTarget;
use crate::models::{CloudPc, CloudPcAction};
use crate::overrides::{ConnectionOverrides, OverrideStore};
use crate::placement::PlacementStore;
use crate::platform::secrets::{self, SecretKey};
use crate::session::{self, Credentials};
use crate::{platform, rdp_file};

const MENU_SIGN_IN: &str = "Sign In…";
const MENU_ADD_ACCOUNT: &str = "Add Another Account…";
const MENU_SIGN_OUT: &str = "Sign Out";
const MENU_SETTINGS: &str = "Settings";
const MENU_ABOUT: &str = "About Windows App";
const MENU_QUIT: &str = "Quit";
const SWITCH_PREFIX: &str = "Switch to ";
const GUARD_ATTEMPTS: u32 = 50;
const SUGGESTIONS: usize = 8;

/// What the app was started with.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Startup {
    /// `--connect` target.
    pub target: Option<LaunchTarget>,
    /// `.rdp` files to open.
    pub files: Vec<PathBuf>,
}

pub enum Msg {
    InstallGuard,
    Loaded(FeedLoad),
    Refresh,
    SignIn,
    SignInDone(bool),
    Search(String),
    QuickConnect,
    PaneToggled(bool),
    Navigate(Option<String>),
    SectionExpanded(String, bool),
    ToggleListMode,
    DismissStatus,
    ColorScheme(ColorScheme),
    AddMenu(String),
    AccountMenu(String),
    ItemMenu(LaunchTarget, String),
    Connect(LaunchTarget),
    Dialog(dialogs::DialogMsg),
    ShowAbout,
    Prepared {
        id: String,
        result: Result<Prepared, String>,
    },
    PreCommandDone {
        result: Result<(), String>,
        prepared: Box<Prepared>,
    },
    SecretsLoaded {
        connection: Box<SavedConnection>,
        overrides: Box<ConnectionOverrides>,
        credentials: Credentials,
    },
    ActionDone {
        id: String,
        name: String,
        action: CloudPcAction,
        result: Result<(), String>,
    },
    Renamed {
        id: String,
        result: Result<(), String>,
    },
    Troubleshot {
        id: String,
        report: (String, String),
    },
    Snapshots {
        pc: Box<CloudPc>,
        result: Result<Vec<Snapshot>, String>,
    },
    ImportPicked(PathBuf),
    ExportPicked(SavedConnection, PathBuf),
    Notice(Notice),
    QuitRequested,
}

/// Why a dialog is shown: what its outcome applies to.
enum Purpose {
    Info,
    Restart(CloudPc),
    Rename(CloudPc),
    Reset(CloudPc),
    Restore(CloudPc, Vec<Snapshot>),
    Remove(SavedConnection),
    Credentials {
        saved: bool,
        remembered: bool,
        overrides: ConnectionOverrides,
    },
    RemotePc {
        remembered: bool,
    },
    ConnectionSettings(LaunchTarget, String),
    Quit,
}

struct Queued {
    serial: u64,
    spec: DialogSpec,
    purpose: Purpose,
}

struct Status {
    severity: InfoBarSeverity,
    text: String,
}

pub struct MainApp {
    auth: Arc<AuthManager>,
    service: Arc<CloudPcService>,
    library: Library,
    recent: BTreeMap<String, chrono::DateTime<chrono::Utc>>,
    collapsed: HashSet<String>,
    busy: HashSet<String>,
    loading: bool,
    signing_in: bool,
    search: String,
    is_pane_open: bool,
    page: String,
    list_mode: bool,
    status: Option<Status>,
    dialogs: VecDeque<Queued>,
    active_dialog: Option<dialogs::ActiveDialog>,
    next_serial: u64,
    pending_target: Option<LaunchTarget>,
    account: Option<Account>,
    accounts: Vec<Account>,
    notifier: Notifier,
    guard_attempts: u32,
}

impl MainApp {
    fn set_status(&mut self, severity: InfoBarSeverity, text: impl Into<String>) {
        self.status = Some(Status {
            severity,
            text: text.into(),
        });
    }

    fn info(&mut self, text: impl Into<String>) {
        self.set_status(InfoBarSeverity::Informational, text);
    }

    fn success(&mut self, text: impl Into<String>) {
        self.set_status(InfoBarSeverity::Success, text);
    }

    fn error(&mut self, text: impl Into<String>) {
        self.set_status(InfoBarSeverity::Error, text);
    }

    fn queue(&mut self, spec: DialogSpec, purpose: Purpose) {
        let is_first = self.dialogs.is_empty();
        self.next_serial += 1;
        self.dialogs.push_back(Queued {
            serial: self.next_serial,
            spec: spec.clone(),
            purpose,
        });
        if is_first || self.active_dialog.is_none() {
            self.active_dialog = Some(dialogs::ActiveDialog::new(&spec));
        }
    }

    fn show_message(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.queue(
            DialogSpec::Message {
                title: title.into(),
                body: body.into(),
            },
            Purpose::Info,
        );
    }

    /// Error in the status bar and in a dialog.
    fn show_error(&mut self, heading: &str, body: &str) {
        self.error(format!("{heading}: {body}"));
        self.show_message(heading, body);
    }

    fn refresh_account(&mut self) {
        self.account = self.auth.current_account();
        self.accounts = self.auth.accounts();
    }

    fn save_view_config(&self) {
        let mut config = AppConfig::load();
        config.view_mode = if self.list_mode { "list" } else { "grid" }.to_string();
        let mut collapsed: Vec<String> = self.collapsed.iter().cloned().collect();
        collapsed.sort();
        config.collapsed_sections = collapsed;
        if let Err(e) = config.save() {
            log::warn!("Could not save the view settings: {e}");
        }
    }

    // ----- Loading and accounts -----

    fn load(&mut self, ctx: &ComponentContext<Self>) {
        if self.loading {
            return;
        }
        self.refresh_account();
        if !self.auth.is_signed_in() {
            self.library.cloud_pcs.clear();
            self.library.avd.clear();
            self.info("Sign in to connect to your Cloud PCs.");
            if let Some(target) = self.pending_target.take() {
                self.report_missing(&target);
            }
            return;
        }
        self.loading = true;
        self.info("Fetching Cloud PCs from Windows 365…");
        let service = Arc::clone(&self.service);
        let _task =
            ctx.spawn_background(move |_| Msg::Loaded(block_on(library::load_feeds(&service))));
    }

    fn loaded(&mut self, load: FeedLoad, ctx: &ComponentContext<Self>) {
        self.loading = false;
        self.refresh_account();
        let FeedLoad { cloud_pcs, avd } = load;
        match cloud_pcs {
            Ok(pcs) => {
                self.success(load_summary(pcs.len(), avd.len()));
                self.library.cloud_pcs = pcs;
            }
            Err(e) => {
                log::error!("Loading Cloud PCs failed: {e}");
                self.error(load_error_message(&e));
            }
        }
        self.library.avd = avd;
        if let Some(target) = self.pending_target.take() {
            if self.library.find(&target).is_some() {
                self.connect(&target, ctx);
            } else {
                self.report_missing(&target);
            }
        }
    }

    fn report_missing(&mut self, target: &LaunchTarget) {
        let name = self
            .library
            .find(target)
            .map_or_else(|| target.to_string(), |item| item.title().to_string());
        log::warn!("Launch target {target} is not available");
        self.error(format!("{name} is no longer available to this account."));
    }

    fn sign_in(&mut self, ctx: &ComponentContext<Self>) {
        if self.signing_in {
            return;
        }
        let sender = ctx.sender();
        let input = AccountSignInInput {
            done: Callback::new(move |signed_in| {
                sender.send(Msg::SignInDone(signed_in));
            }),
        };
        if ctx.open_window(View::component::<AccountSignIn>(input)) {
            self.signing_in = true;
            self.info("Opening Microsoft sign-in…");
        } else {
            self.error("Could not open the sign-in window.");
        }
    }

    fn signed_in(&mut self, success: bool, ctx: &ComponentContext<Self>) {
        self.signing_in = false;
        if !success {
            self.refresh_account();
            if !self.auth.is_signed_in() {
                self.info("Sign in to connect to your Cloud PCs.");
            } else {
                self.status = None;
            }
            return;
        }
        // The new sign-in may be a different account.
        self.auth.forget_scoped_tokens();
        self.library.cloud_pcs.clear();
        self.library.avd.clear();
        self.load(ctx);
        self.info("Signed in. Loading Cloud PCs…");
    }

    fn sign_out(&mut self) {
        if let Err(e) = self.auth.sign_out() {
            log::error!("Sign-out failed: {e}");
            self.error(format!("Could not remove the stored sign-in: {e}"));
            return;
        }
        AppConfig::remove_legacy_files();
        self.library.cloud_pcs.clear();
        self.library.avd.clear();
        self.refresh_account();
        self.success("Signed out.");
    }

    fn switch_account(&mut self, upn: &str, ctx: &ComponentContext<Self>) {
        match self.auth.switch_account(upn) {
            Ok(true) => {
                self.library.cloud_pcs.clear();
                self.library.avd.clear();
                self.load(ctx);
                self.info(format!("Switched to {upn}. Loading Cloud PCs…"));
            }
            Ok(false) => {
                self.refresh_account();
                self.error(format!("{upn} needs to sign in again."));
            }
            Err(e) => self.error(format!("Could not switch accounts: {e}")),
        }
    }

    fn account_menu(&self) -> Vec<windows_reactor::MenuItem> {
        let active = self.account.as_ref().and_then(|a| a.upn.clone());
        let mut items = Vec::new();
        for other in &self.accounts {
            let Some(upn) = other.upn.as_deref() else {
                continue;
            };
            if Some(upn) == active.as_deref() {
                continue;
            }
            let label = switch_label(other.display_name.as_deref(), upn);
            items.push(windows_reactor::MenuItem::item(label.clone(), label));
        }
        if !items.is_empty() {
            items.push(windows_reactor::MenuItem::separator("switch"));
        }
        if self.account.is_some() {
            items.push(windows_reactor::MenuItem::item(
                MENU_ADD_ACCOUNT,
                MENU_ADD_ACCOUNT,
            ));
            items.push(windows_reactor::MenuItem::item(
                MENU_SIGN_OUT,
                MENU_SIGN_OUT,
            ));
        } else {
            items.push(windows_reactor::MenuItem::item(MENU_SIGN_IN, MENU_SIGN_IN));
        }
        items.push(windows_reactor::MenuItem::separator("options"));
        items.push(windows_reactor::MenuItem::item(
            MENU_SETTINGS,
            MENU_SETTINGS,
        ));
        items.push(windows_reactor::MenuItem::item(MENU_ABOUT, MENU_ABOUT));
        items.push(windows_reactor::MenuItem::separator("quit"));
        items.push(windows_reactor::MenuItem::item(MENU_QUIT, MENU_QUIT));
        items
    }

    fn account_chosen(&mut self, label: &str, ctx: &ComponentContext<Self>) {
        match label {
            MENU_SIGN_IN | MENU_ADD_ACCOUNT => self.sign_in(ctx),
            MENU_SIGN_OUT => self.sign_out(),
            MENU_SETTINGS => self.page = NAV_SETTINGS.to_string(),
            MENU_ABOUT => self.queue(DialogSpec::About, Purpose::Info),
            MENU_QUIT => self.quit(ctx),
            _ => {
                let upn = self.accounts.iter().find_map(|account| {
                    let upn = account.upn.as_deref()?;
                    (switch_label(account.display_name.as_deref(), upn) == label)
                        .then(|| upn.to_string())
                });
                if let Some(upn) = upn {
                    self.switch_account(&upn, ctx);
                }
            }
        }
    }

    // ----- Quitting -----

    fn quit(&mut self, ctx: &ComponentContext<Self>) {
        if self
            .dialogs
            .iter()
            .any(|d| matches!(d.purpose, Purpose::Quit))
        {
            return;
        }
        let names = session_window::active_names();
        if names.is_empty() {
            self.close_now(ctx);
            return;
        }
        let (title, body) = quit_prompt(&names);
        // Sessions are separate windows here; "keep open" has no meaning on Windows.
        let body = match body.split_once("; keeping") {
            Some((head, _)) => format!("{head}."),
            None => body,
        };
        self.queue(
            DialogSpec::Confirm {
                title,
                body,
                confirm: "Disconnect and Quit".to_string(),
            },
            Purpose::Quit,
        );
    }

    fn close_now(&mut self, ctx: &ComponentContext<Self>) {
        session_window::end_all();
        win32::allow_close();
        if !ctx.window().request_close() {
            log::warn!("Could not close the main window");
        }
    }

    fn install_guard(&mut self, ctx: &ComponentContext<Self>) {
        let sender = ctx.sender();
        let installed = win32::install_close_guard(move || {
            if session_window::active_names().is_empty() {
                true
            } else {
                sender.send(Msg::QuitRequested);
                false
            }
        });
        if installed {
            return;
        }
        self.guard_attempts += 1;
        if self.guard_attempts > GUARD_ATTEMPTS {
            log::warn!("The main window was not found; closing it will not ask about sessions");
            return;
        }
        let _task = ctx.spawn_background(|_| {
            std::thread::sleep(std::time::Duration::from_millis(200));
            Msg::InstallGuard
        });
    }

    // ----- Connecting -----

    fn connect(&mut self, target: &LaunchTarget, ctx: &ComponentContext<Self>) {
        match self.library.find(target) {
            Some(Item::CloudPc(pc)) => {
                if !self.busy.insert(pc.id.clone()) {
                    return;
                }
                self.info(format!("Preparing connection to {}…", pc.display_name));
                let service = Arc::clone(&self.service);
                let display = display_info();
                let _task = ctx.spawn_background(move |_| Msg::Prepared {
                    id: pc.id.clone(),
                    result: block_on(launch::prepare_cloud_pc(&service, &pc, display)),
                });
            }
            Some(Item::Avd(resource)) => {
                if !self.busy.insert(resource.id.clone()) {
                    return;
                }
                self.info(format!("Preparing connection to {}…", resource.title));
                let service = Arc::clone(&self.service);
                let display = display_info();
                let _task = ctx.spawn_background(move |_| Msg::Prepared {
                    id: resource.id.clone(),
                    result: block_on(launch::prepare_avd(&service, &resource, display)),
                });
            }
            Some(Item::Remote(connection)) => {
                let overrides = OverrideStore::default().get(target);
                self.connect_remote(connection, true, overrides, ctx);
            }
            None if matches!(target, LaunchTarget::RemotePc(_)) => self.report_missing(target),
            None if self.loading => self.pending_target = Some(target.clone()),
            None if self.auth.is_signed_in() => {
                self.pending_target = Some(target.clone());
                self.load(ctx);
            }
            None => self.info("Sign in to connect to your Cloud PCs."),
        }
    }

    /// Runs the connection's command before connecting (if any), then opens the session.
    fn start_session(&mut self, prepared: Prepared, ctx: &ComponentContext<Self>) {
        if prepared.0.overrides.pre_command.is_empty() {
            self.open_session(prepared);
            return;
        }
        self.info(format!(
            "Running the command before connecting to {}…",
            prepared.0.name
        ));
        let _task = ctx.spawn_background(move |_| {
            let result = block_on(launch::run_pre_command(&prepared.0));
            Msg::PreCommandDone {
                result,
                prepared: Box::new(prepared),
            }
        });
    }

    fn open_session(&mut self, (launch, prepared): Prepared) {
        log::debug!(
            "Session arguments: {:?}",
            session::redact_args(&prepared.args)
        );
        let name = launch.name.clone();
        match session_window::open(launch, prepared, Rc::clone(&self.notifier)) {
            Ok(()) => {
                self.info(format!("Connecting to {name}…"));
                self.recent = RecentStore::default().load();
            }
            Err(e) => {
                log::error!("Connecting to {name} failed: {e}");
                self.error(format!("Could not connect: {e}"));
            }
        }
    }

    /// Signs in (saved password or prompt) and connects to `connection`. `saved`: the
    /// connection is in the library (so it has history and can remember passwords).
    fn connect_remote(
        &mut self,
        connection: SavedConnection,
        saved: bool,
        overrides: ConnectionOverrides,
        ctx: &ComponentContext<Self>,
    ) {
        if !(saved && connection.remember_password) {
            self.prompt_credentials(connection, saved, overrides, Credentials::default());
            return;
        }
        let _task = ctx.spawn_background(move |_| {
            let credentials = load_secrets(&connection);
            Msg::SecretsLoaded {
                connection: Box::new(connection),
                overrides: Box::new(overrides),
                credentials,
            }
        });
    }

    fn secrets_loaded(
        &mut self,
        connection: SavedConnection,
        overrides: ConnectionOverrides,
        credentials: Credentials,
        ctx: &ComponentContext<Self>,
    ) {
        let complete = credentials.password.is_some()
            && (!connection.gateway_needs_password() || credentials.gateway_password.is_some());
        if complete {
            self.finish_remote(&connection, true, overrides, &credentials, ctx);
        } else {
            self.prompt_credentials(connection, true, overrides, credentials);
        }
    }

    fn prompt_credentials(
        &mut self,
        connection: SavedConnection,
        saved: bool,
        overrides: ConnectionOverrides,
        known: Credentials,
    ) {
        let remembered = connection.remember_password;
        self.queue(
            DialogSpec::Credentials {
                connection,
                can_remember: saved,
                known,
            },
            Purpose::Credentials {
                saved,
                remembered,
                overrides,
            },
        );
    }

    fn credentials_entered(
        &mut self,
        connection: SavedConnection,
        credentials: Credentials,
        saved: bool,
        remembered: bool,
        overrides: ConnectionOverrides,
        ctx: &ComponentContext<Self>,
    ) {
        if saved && connection.remember_password {
            store_secrets(&connection.id, &credentials);
        } else if remembered {
            forget_secrets(&connection.id);
        }
        if saved && connection.remember_password != remembered {
            match ConnectionStore::default().upsert(connection.clone()) {
                Ok(list) => self.library.remote_pcs = list,
                Err(e) => log::warn!("Could not update the saved connection: {e}"),
            }
        }
        self.finish_remote(&connection, saved, overrides, &credentials, ctx);
    }

    fn finish_remote(
        &mut self,
        connection: &SavedConnection,
        saved: bool,
        overrides: ConnectionOverrides,
        credentials: &Credentials,
        ctx: &ComponentContext<Self>,
    ) {
        let username = Some(connection.username.clone()).filter(|u| !u.is_empty());
        let (config, options) = launch::launch_options(&overrides, display_info(), username);
        match session::prepare_direct(connection, credentials, &options) {
            Ok(prepared) => {
                let launch = Launch {
                    name: connection.display_name().to_string(),
                    target: saved.then(|| LaunchTarget::RemotePc(connection.id.clone())),
                    address: connection.address.clone(),
                    login_hint: None,
                    overrides,
                    multi_monitor: config.multi_monitor,
                    dynamic_resolution: options.dynamic_resolution,
                };
                self.start_session((launch, prepared), ctx);
            }
            Err(e) => self.error(format!("Could not connect: {e}")),
        }
    }

    fn quick_connect(&mut self, ctx: &ComponentContext<Self>) {
        let text = self.search.trim().to_string();
        if text.is_empty() {
            return;
        }
        let sections = self.library.sections(
            &SearchFilter::new(&text),
            &HashSet::new(),
            &HashSet::new(),
            &BTreeMap::new(),
        );
        let matches: Vec<LaunchTarget> = sections
            .iter()
            .flat_map(|s| s.cards.iter().map(|c| c.item.target()))
            .collect();
        match matches.as_slice() {
            [target] => self.connect(target, ctx),
            [] => match SavedConnection::new("", &text, "") {
                Ok(connection) => {
                    self.connect_remote(connection, false, ConnectionOverrides::default(), ctx)
                }
                Err(e) => self.error(e.to_string()),
            },
            _ => {}
        }
    }

    fn open_rdp_file(&mut self, path: &std::path::Path, ctx: &ComponentContext<Self>) {
        match read_rdp_file(path) {
            Ok(imported) => {
                self.connect_remote(imported.connection, false, imported.overrides, ctx);
            }
            Err(message) => self.error(message),
        }
    }

    // ----- Cloud PC actions -----

    fn run_pc_action<F>(
        &mut self,
        pc: &CloudPc,
        action: CloudPcAction,
        request: F,
        ctx: &ComponentContext<Self>,
    ) where
        F: std::future::Future<Output = crate::Result<()>> + Send + 'static,
    {
        if !self.busy.insert(pc.id.clone()) {
            return;
        }
        let name = pc.display_name.clone();
        let id = pc.id.clone();
        self.info(format!("{} {name}…", action.progress_label()));
        let _task = ctx.spawn_background(move |_| Msg::ActionDone {
            id,
            name,
            action,
            result: block_on(request).map_err(|e| e.to_string()),
        });
    }

    fn action_done(
        &mut self,
        id: &str,
        name: &str,
        action: CloudPcAction,
        result: Result<(), String>,
        ctx: &ComponentContext<Self>,
    ) {
        self.busy.remove(id);
        match result {
            Ok(()) => {
                self.load(ctx);
                self.info(format!(
                    "{} {name} — this can take a few minutes.",
                    action.progress_label()
                ));
            }
            Err(e) => {
                log::error!("{action:?} on {name} failed: {e}");
                self.show_error(&format!("Could not finish on {name}"), &e);
            }
        }
    }

    fn item_action(&mut self, target: &LaunchTarget, label: &str, ctx: &ComponentContext<Self>) {
        let Some(action) = views::action_from_label(label) else {
            return;
        };
        let Some(item) = self.library.find(target) else {
            self.report_missing(target);
            return;
        };
        let name = item.title().to_string();
        match (action, item) {
            (ItemAction::Connect, _) => self.connect(target, ctx),
            (ItemAction::Preferences, _) => self.page = NAV_SETTINGS.to_string(),
            (ItemAction::ConnectionSettings, _) => self.queue(
                DialogSpec::ConnectionSettings {
                    name: name.clone(),
                    current: OverrideStore::default().get(target),
                },
                Purpose::ConnectionSettings(target.clone(), name),
            ),
            (ItemAction::Start | ItemAction::Stop, Item::CloudPc(pc)) => {
                let start = action == ItemAction::Start;
                let service = Arc::clone(&self.service);
                let id = pc.id.clone();
                let power = if start {
                    CloudPcAction::Start
                } else {
                    CloudPcAction::Stop
                };
                self.run_pc_action(
                    &pc,
                    power,
                    async move { service.set_power(&id, start).await },
                    ctx,
                );
            }
            (ItemAction::Restart, Item::CloudPc(pc)) => self.queue(
                DialogSpec::Confirm {
                    title: format!("Restart {name}?"),
                    body: RESTART_BODY.to_string(),
                    confirm: "Restart".to_string(),
                },
                Purpose::Restart(pc),
            ),
            (ItemAction::Rename, Item::CloudPc(pc)) => self.queue(
                DialogSpec::Text {
                    title: "Rename Cloud PC".to_string(),
                    body: "Enter a new display name:".to_string(),
                    label: "Display name".to_string(),
                    initial: pc.display_name.clone(),
                    confirm: "Rename".to_string(),
                    must_equal: None,
                },
                Purpose::Rename(pc),
            ),
            (ItemAction::OpenInBrowser, Item::CloudPc(pc)) => {
                if let Some(url) = &pc.direct_launch_url {
                    win32::open_uri(url);
                }
            }
            (ItemAction::Troubleshoot, Item::CloudPc(pc)) => {
                if !self.busy.insert(pc.id.clone()) {
                    return;
                }
                self.info(format!(
                    "Troubleshooting {name} — this can take several minutes…"
                ));
                let service = Arc::clone(&self.service);
                let _task = ctx.spawn_background(move |_| {
                    let outcome = block_on(service.troubleshoot_cloud_pc(&pc.id));
                    Msg::Troubleshot {
                        report: troubleshoot_report(&pc.display_name, &outcome),
                        id: pc.id,
                    }
                });
            }
            (ItemAction::Inspect, Item::CloudPc(pc)) => {
                let rows = owned_rows(inspect_rows(&pc));
                self.queue(
                    DialogSpec::Properties {
                        title: format!("Connection - {name}"),
                        rows,
                    },
                    Purpose::Info,
                );
            }
            (ItemAction::Details, Item::CloudPc(pc)) => {
                let rows = owned_rows(detail_rows(&pc));
                self.queue(
                    DialogSpec::Properties {
                        title: format!("Details - {name}"),
                        rows,
                    },
                    Purpose::Info,
                );
            }
            (ItemAction::Restore, Item::CloudPc(pc)) => {
                if self.busy.contains(&pc.id) {
                    return;
                }
                self.info(format!("Loading restore points of {name}…"));
                let service = Arc::clone(&self.service);
                let _task = ctx.spawn_background(move |_| {
                    let result =
                        block_on(service.list_snapshots(&pc.id)).map_err(|e| e.to_string());
                    Msg::Snapshots {
                        pc: Box::new(pc),
                        result,
                    }
                });
            }
            (ItemAction::Reset, Item::CloudPc(pc)) => self.queue(
                DialogSpec::Text {
                    title: format!("Reset {name}?"),
                    body: RESET_BODY.to_string(),
                    label: format!("Type “{name}” to confirm"),
                    initial: String::new(),
                    confirm: "Reset".to_string(),
                    must_equal: Some(name),
                },
                Purpose::Reset(pc),
            ),
            (ItemAction::Edit, Item::Remote(connection)) => {
                let remembered = connection.remember_password;
                self.queue(
                    DialogSpec::RemotePc {
                        existing: Some(connection),
                    },
                    Purpose::RemotePc { remembered },
                );
            }
            (ItemAction::Export, Item::Remote(connection)) => {
                let sender = ctx.sender();
                win32::defer(move || {
                    let file_name =
                        format!("{}.rdp", sanitize_file_name(connection.display_name()));
                    if let Some(path) = win32::pick_save_file(
                        win32::main_window(),
                        "Export RDP File",
                        &file_name,
                        ("Remote Desktop files", "*.rdp"),
                        "rdp",
                    ) {
                        sender.send(Msg::ExportPicked(connection, path));
                    }
                });
            }
            (ItemAction::ForgetPassword, Item::Remote(mut connection)) => {
                forget_secrets(&connection.id);
                connection.remember_password = false;
                match ConnectionStore::default().upsert(connection) {
                    Ok(list) => {
                        self.library.remote_pcs = list;
                        self.success("The saved password was removed.");
                    }
                    Err(e) => self.error(format!("Could not update {name}: {e}")),
                }
            }
            (ItemAction::Remove, Item::Remote(connection)) => self.queue(
                DialogSpec::Confirm {
                    title: format!("Remove {name}?"),
                    body: "The saved connection is removed from this computer.".to_string(),
                    confirm: "Remove".to_string(),
                },
                Purpose::Remove(connection),
            ),
            (action, _) => log::debug!("{action:?} is not available here"),
        }
    }

    fn snapshots_loaded(&mut self, pc: CloudPc, result: Result<Vec<Snapshot>, String>) {
        let name = pc.display_name.clone();
        match result {
            Ok(snapshots) if snapshots.is_empty() => {
                self.status = None;
                self.show_message(
                    format!("No restore points for {name}"),
                    "Restore points are created automatically; none are available yet.",
                );
            }
            Ok(snapshots) => {
                self.status = None;
                self.queue(
                    DialogSpec::Choice {
                        title: format!("Restore {name}?"),
                        body: RESTORE_BODY.to_string(),
                        label: "Restore point".to_string(),
                        options: snapshots.iter().map(snapshot_label).collect(),
                        confirm: "Restore".to_string(),
                    },
                    Purpose::Restore(pc, snapshots),
                );
            }
            Err(e) => {
                log::error!("Loading the restore points of {name} failed: {e}");
                self.show_error(&format!("Could not load restore points of {name}"), &e);
            }
        }
    }

    // ----- Remote PCs -----

    fn save_remote_pc(&mut self, connection: SavedConnection) {
        let name = connection.display_name().to_string();
        let store = ConnectionStore::default();
        match store.upsert(connection) {
            Ok(list) => {
                self.library.remote_pcs = list;
                self.success(format!("Saved {name}."));
            }
            Err(e) => {
                log::error!("Saving {} failed: {e}", store.path().display());
                self.error(format!("Could not save {name}: {e}"));
            }
        }
    }

    fn remove_remote_pc(&mut self, connection: &SavedConnection) {
        let name = connection.display_name().to_string();
        let target = LaunchTarget::RemotePc(connection.id.clone());
        platform::remove_desktop_integration(&target);
        forget_secrets(&connection.id);
        if let Err(e) = OverrideStore::default().remove(&target) {
            log::warn!("Could not remove the settings of {name}: {e}");
        }
        if let Err(e) = RecentStore::default().remove(&target) {
            log::warn!("Could not remove the history of {name}: {e}");
        }
        if let Err(e) = PlacementStore::default().remove(&target) {
            log::warn!("Could not remove the window placement of {name}: {e}");
        }
        match ConnectionStore::default().remove(&connection.id) {
            Ok(list) => {
                self.library.remote_pcs = list;
                self.recent = RecentStore::default().load();
                self.success(format!("Removed {name}."));
            }
            Err(e) => self.error(format!("Could not remove {name}: {e}")),
        }
    }

    fn import_rdp(&mut self, path: &std::path::Path) {
        match read_rdp_file(path) {
            Ok(imported) => {
                let target = LaunchTarget::RemotePc(imported.connection.id.clone());
                if let Err(e) = OverrideStore::default().set(&target, imported.overrides) {
                    log::warn!("Could not save the imported settings: {e}");
                }
                self.save_remote_pc(imported.connection);
            }
            Err(message) => self.error(message),
        }
    }

    fn export_rdp(&mut self, connection: &SavedConnection, path: &std::path::Path) {
        let overrides =
            OverrideStore::default().get(&LaunchTarget::RemotePc(connection.id.clone()));
        let text = rdp_file::export(connection, &overrides);
        let name = connection.display_name();
        match std::fs::write(path, text) {
            Ok(()) => self.success(format!("Exported {name} to {}.", path.display())),
            Err(e) => self.error(format!("Could not export {name}: {e}")),
        }
    }

    // ----- Dialog outcomes -----

    fn dialog_done(&mut self, outcome: Outcome, ctx: &ComponentContext<Self>) {
        let Some(Queued { purpose, .. }) = self.dialogs.pop_front() else {
            return;
        };
        self.active_dialog = self
            .dialogs
            .front()
            .map(|q| dialogs::ActiveDialog::new(&q.spec));
        match (purpose, outcome) {
            (_, Outcome::Cancelled) | (Purpose::Info, _) => {}
            (Purpose::Restart(pc), Outcome::Confirmed) => {
                let service = Arc::clone(&self.service);
                let id = pc.id.clone();
                self.run_pc_action(
                    &pc,
                    CloudPcAction::Restart,
                    async move { service.restart_cloud_pc(&id).await },
                    ctx,
                );
            }
            (Purpose::Rename(pc), Outcome::Text(new_name)) => {
                let new_name = new_name.trim().to_string();
                if new_name.is_empty() || new_name == pc.display_name {
                    return;
                }
                let service = Arc::clone(&self.service);
                self.info(format!("Renaming {}…", pc.display_name));
                let _task = ctx.spawn_background(move |_| Msg::Renamed {
                    result: block_on(service.rename_cloud_pc(&pc.id, &new_name))
                        .map_err(|e| e.to_string()),
                    id: pc.id,
                });
            }
            (Purpose::Reset(pc), Outcome::Text(_)) => {
                let service = Arc::clone(&self.service);
                let id = pc.id.clone();
                self.run_pc_action(
                    &pc,
                    CloudPcAction::Reset,
                    async move { service.reset_cloud_pc(&id).await },
                    ctx,
                );
            }
            (Purpose::Restore(pc, snapshots), Outcome::Index(index)) => {
                let Some(snapshot) = snapshots.get(index) else {
                    return;
                };
                let service = Arc::clone(&self.service);
                let id = pc.id.clone();
                let snapshot_id = snapshot.id.clone();
                self.run_pc_action(
                    &pc,
                    CloudPcAction::Restore,
                    async move { service.restore_cloud_pc(&id, &snapshot_id).await },
                    ctx,
                );
            }
            (Purpose::Remove(connection), Outcome::Confirmed) => {
                self.remove_remote_pc(&connection);
            }
            (
                Purpose::Credentials {
                    saved,
                    remembered,
                    overrides,
                },
                Outcome::Credentials {
                    connection,
                    credentials,
                },
            ) => {
                self.credentials_entered(connection, credentials, saved, remembered, overrides, ctx)
            }
            (Purpose::RemotePc { remembered }, Outcome::Connection(connection)) => {
                if remembered && !connection.remember_password {
                    forget_secrets(&connection.id);
                }
                self.save_remote_pc(connection);
            }
            (Purpose::ConnectionSettings(target, name), Outcome::Overrides(overrides)) => {
                match OverrideStore::default().set(&target, overrides) {
                    Ok(()) => self.success(format!("Saved the settings of {name}.")),
                    Err(e) => self.error(format!("Could not save the settings of {name}: {e}")),
                }
            }
            (Purpose::Quit, Outcome::Confirmed) => self.close_now(ctx),
            (_, outcome) => log::debug!("Unexpected dialog outcome {outcome:?}"),
        }
    }

    // ----- Session windows -----

    fn notice(&mut self, notice: Notice, ctx: &ComponentContext<Self>) {
        match notice {
            Notice::AuthRequired { id, name, url } => {
                let input = HostSignInInput {
                    id,
                    name: name.clone(),
                    url,
                };
                if !ctx.open_window(View::component::<HostSignIn>(input)) {
                    log::error!("Could not open the sign-in window for {name}");
                    session_window::provide_auth(id, None);
                }
            }
            Notice::CustomScale { id, name, current } => {
                let input = ScaleInput { id, name, current };
                if !ctx.open_window(View::component::<ScaleWindow>(input)) {
                    log::error!("Could not open the display scale window");
                }
            }
            Notice::Closed { name, message } => {
                self.recent = RecentStore::default().load();
                if message.is_empty() {
                    self.info(format!("Session with {name} ended."));
                } else {
                    self.info(format!("Session with {name} ended: {message}"));
                }
            }
        }
    }

    // ----- Views -----

    fn search_line(&self, ctx: &ViewContext<Self>, shown: usize) -> View {
        let text = self.search.trim();
        let address = connections::parse_address(text).is_ok();
        let (message, connect) = match (shown, address) {
            (0, true) => (
                "No matches. Press Ctrl+Enter to connect to this address.".to_string(),
                Some(format!("Connect to {text}")),
            ),
            (0, false) => ("No matches.".to_string(), None),
            (1, _) => (
                "1 match. Press Ctrl+Enter to connect.".to_string(),
                Some("Connect".to_string()),
            ),
            (n, _) => (format!("{n} matches."), None),
        };
        let mut children: Vec<(String, View)> = vec![(
            "text".into(),
            TextBlock::new()
                .text(message)
                .opacity(0.7)
                .vertical_alignment(VerticalAlignment::Center)
                .into(),
        )];
        if let Some(label) = connect {
            children.push((
                "connect".into(),
                Button::new()
                    .style(ButtonStyle::Accent)
                    .on_click(ctx.callback(|()| Msg::QuickConnect))
                    .content(label),
            ));
        }
        StackPanel::new()
            .orientation(Orientation::Horizontal)
            .spacing(12.0)
            .keyed_children(children)
    }

    fn header(&self, ctx: &ViewContext<Self>) -> View {
        let (view_glyph, view_tip) = if self.list_mode {
            ("\u{F0E2}", "Show as tiles")
        } else {
            ("\u{E8FD}", "Show as list")
        };
        Grid::new()
            .columns([GridLength::Star(1.0), GridLength::Auto])
            .children((
                TextBlock::new()
                    .text(views::page_title(&self.page))
                    .font_size(28.0)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .vertical_alignment(VerticalAlignment::Center),
                StackPanel::new()
                    .grid_column(1)
                    .orientation(Orientation::Horizontal)
                    .spacing(4.0)
                    .children((
                        DropDownButton::new()
                            .content(icon_label("\u{E710}", "Add"))
                            .menu(views::add_menu(ctx)),
                        Button::new()
                            .style(ButtonStyle::Subtle)
                            .is_enabled(!self.loading)
                            .on_click(ctx.callback(|()| Msg::Refresh))
                            .content(icon_label("\u{E72C}", "Refresh"))
                            .tooltip("Refresh (Ctrl+R)"),
                        Button::new()
                            .style(ButtonStyle::Subtle)
                            .on_click(ctx.callback(|()| Msg::ToggleListMode))
                            .content(FontIcon::new().glyph(view_glyph))
                            .tooltip(view_tip),
                    )),
            ))
    }

    fn library_view(&self, ctx: &ViewContext<Self>) -> View {
        if self.library.is_empty() {
            return if self.loading {
                views::loading()
            } else {
                views::empty_state(ctx, self.account.is_some())
            };
        }
        let filter = SearchFilter::new(&self.search);
        let sections: Vec<_> = self
            .library
            .sections(&filter, &self.collapsed, &self.busy, &self.recent)
            .into_iter()
            .filter(|s| self.page == NAV_ALL || s.id == self.page)
            .collect();
        let shown: usize = sections.iter().map(|s| s.cards.len()).sum();
        let mut rows: Vec<(String, View)> = Vec::new();
        if !self.search.trim().is_empty() {
            rows.push(("search".into(), self.search_line(ctx, shown)));
        }
        if self.page == NAV_ALL {
            for section in &sections {
                rows.push((
                    section.id.to_string(),
                    views::section(ctx, section, self.list_mode),
                ));
            }
        } else if let Some(section) = sections.first() {
            rows.push((
                section.id.to_string(),
                views::section_page(ctx, section, self.list_mode),
            ));
        }
        if sections.is_empty() && self.search.trim().is_empty() {
            let text = if self.loading {
                "Loading…"
            } else {
                "Nothing here yet. Use Add to create a connection."
            };
            rows.push((
                "nothing".into(),
                TextBlock::new().text(text).opacity(0.7).into(),
            ));
        }
        ScrollViewer::new().content(
            StackPanel::new()
                .spacing(12.0)
                .margin(Thickness::new(0.0, 0.0, 0.0, 32.0))
                .keyed_children(rows),
        )
    }

    fn connections_page(&self, ctx: &ViewContext<Self>) -> View {
        let status = self.status.as_ref();
        Grid::new()
            .rows([GridLength::Auto, GridLength::Auto, GridLength::Star(1.0)])
            .row_spacing(16.0)
            .margin(Thickness::new(32.0, 16.0, 32.0, 0.0))
            .children((
                self.header(ctx),
                InfoBar::new()
                    .grid_row(1)
                    .message(status.map(|s| s.text.clone()).unwrap_or_default())
                    .severity(status.map_or(InfoBarSeverity::Informational, |s| s.severity))
                    .is_open(status.is_some())
                    .is_closable(true)
                    .on_closed(ctx.callback(|()| Msg::DismissStatus)),
                Border::new().grid_row(2).content(self.library_view(ctx)),
            ))
    }

    fn suggestions(&self) -> Vec<String> {
        if self.search.trim().is_empty() {
            return Vec::new();
        }
        self.library
            .sections(
                &SearchFilter::new(&self.search),
                &HashSet::new(),
                &HashSet::new(),
                &BTreeMap::new(),
            )
            .into_iter()
            .flat_map(|s| s.cards.into_iter().map(|c| c.title))
            .take(SUGGESTIONS)
            .collect()
    }
}

impl Component for MainApp {
    type Input = Startup;
    type Message = Msg;

    fn create(input: &Self::Input, ctx: &ComponentContext<Self>) -> Self {
        let config = AppConfig::load();
        let auth = Arc::new(AuthManager::for_profile(config.auth_profile()));
        let graph = GraphApiClient::new(Arc::clone(&auth), &config.data_boundary);
        let service = Arc::new(CloudPcService::new(graph));
        let sender = ctx.sender();
        let notifier: Notifier = Rc::new(move |notice| {
            sender.send(Msg::Notice(notice));
        });
        let mut app = Self {
            auth,
            service,
            library: Library {
                remote_pcs: ConnectionStore::default().load(),
                ..Library::default()
            },
            recent: RecentStore::default().load(),
            collapsed: config.collapsed_sections.iter().cloned().collect(),
            busy: HashSet::new(),
            loading: false,
            signing_in: false,
            search: String::new(),
            is_pane_open: true,
            page: NAV_ALL.to_string(),
            list_mode: config.view_mode == "list",
            status: None,
            dialogs: VecDeque::new(),
            active_dialog: None,
            next_serial: 0,
            pending_target: None,
            account: None,
            accounts: Vec::new(),
            notifier,
            guard_attempts: 0,
        };
        ctx.sender().send(Msg::InstallGuard);
        app.load(ctx);
        if let Some(target) = &input.target {
            app.connect(target, ctx);
        }
        for path in &input.files {
            app.open_rdp_file(path, ctx);
        }
        app
    }

    fn update(&mut self, msg: Self::Message, ctx: &ComponentContext<Self>) {
        match msg {
            Msg::InstallGuard => self.install_guard(ctx),
            Msg::Loaded(load) => self.loaded(load, ctx),
            Msg::Refresh => {
                self.library.remote_pcs = ConnectionStore::default().load();
                self.recent = RecentStore::default().load();
                self.load(ctx);
            }
            Msg::SignIn => self.sign_in(ctx),
            Msg::SignInDone(success) => self.signed_in(success, ctx),
            Msg::Search(text) => self.search = text,
            Msg::QuickConnect => self.quick_connect(ctx),
            Msg::Navigate(tag) => {
                if let Some(tag) = tag {
                    self.page = tag;
                }
            }
            Msg::SectionExpanded(id, expanded) => {
                // Searching expands every section; that is not the user's choice.
                if !self.search.trim().is_empty() {
                    return;
                }
                let changed = if expanded {
                    self.collapsed.remove(&id)
                } else {
                    self.collapsed.insert(id)
                };
                if changed {
                    self.save_view_config();
                }
            }
            Msg::PaneToggled(open) => self.is_pane_open = open,
            Msg::ToggleListMode => {
                self.list_mode = !self.list_mode;
                self.save_view_config();
            }
            Msg::DismissStatus => self.status = None,
            Msg::ColorScheme(scheme) => win32::set_dark_mode(scheme == ColorScheme::Dark),
            Msg::AddMenu(label) => match label.as_str() {
                ADD_CLOUD => self.sign_in(ctx),
                ADD_REMOTE => self.queue(
                    DialogSpec::RemotePc { existing: None },
                    Purpose::RemotePc { remembered: false },
                ),
                ADD_IMPORT => {
                    let sender = ctx.sender();
                    win32::defer(move || {
                        if let Some(path) = win32::pick_open_file(
                            win32::main_window(),
                            "Import RDP File",
                            &[("Remote Desktop files", "*.rdp")],
                        ) {
                            sender.send(Msg::ImportPicked(path));
                        }
                    });
                }
                _ => {}
            },
            Msg::AccountMenu(label) => self.account_chosen(&label, ctx),
            Msg::ItemMenu(target, label) => self.item_action(&target, &label, ctx),
            Msg::Connect(target) => self.connect(&target, ctx),
            Msg::Dialog(dialog_msg) => {
                if let Some(active) = &mut self.active_dialog {
                    if let Some(outcome) = active.update(dialog_msg) {
                        self.dialog_done(outcome, ctx);
                    }
                }
            }
            Msg::ShowAbout => self.queue(DialogSpec::About, Purpose::Info),
            Msg::Prepared { id, result } => {
                self.busy.remove(&id);
                match result {
                    Ok(prepared) => self.start_session(prepared, ctx),
                    Err(e) => {
                        log::error!("Preparing the connection failed: {e}");
                        self.error(format!("Could not connect: {e}"));
                    }
                }
            }
            Msg::PreCommandDone { result, prepared } => match result {
                Ok(()) => self.open_session(*prepared),
                Err(e) => self.error(e),
            },
            Msg::SecretsLoaded {
                connection,
                overrides,
                credentials,
            } => self.secrets_loaded(*connection, *overrides, credentials, ctx),
            Msg::ActionDone {
                id,
                name,
                action,
                result,
            } => self.action_done(&id, &name, action, result, ctx),
            Msg::Renamed { id, result } => {
                self.busy.remove(&id);
                match result {
                    Ok(()) => self.load(ctx),
                    Err(e) => self.error(format!("Rename failed: {e}")),
                }
            }
            Msg::Troubleshot {
                id,
                report: (heading, body),
            } => {
                self.busy.remove(&id);
                self.status = None;
                self.show_message(heading, body);
                self.load(ctx);
            }
            Msg::Snapshots { pc, result } => self.snapshots_loaded(*pc, result),
            Msg::ImportPicked(path) => self.import_rdp(&path),
            Msg::ExportPicked(connection, path) => self.export_rdp(&connection, &path),
            Msg::Notice(notice) => self.notice(notice, ctx),
            Msg::QuitRequested => self.quit(ctx),
        }
    }

    fn view(&self, _input: &Self::Input, ctx: &mut ViewContext<Self>) -> View {
        ctx.window_title(APP_NAME);
        ctx.window_visuals(
            WindowVisuals::new()
                .backdrop(WindowBackdrop::Mica)
                .client_size(1100.0, 720.0)
                .constraints(WindowConstraints {
                    min_width: Some(640.0),
                    min_height: Some(480.0),
                    max_width: None,
                    max_height: None,
                }),
        );
        ctx.on_color_scheme(ctx.callback(Msg::ColorScheme));

        let content = if self.page == NAV_SETTINGS {
            View::component::<SettingsPage>(SettingsPageInput {
                about: ctx.callback(|()| Msg::ShowAbout),
            })
        } else {
            self.connections_page(ctx)
        };
        let account = self.account.as_ref();
        let navigation = NavigationView::new()
            .grid_row(1)
            .pane_display_mode(NavigationViewPaneDisplayMode::Left)
            .is_pane_open(self.is_pane_open)
            .on_is_pane_open_changed(ctx.callback(Msg::PaneToggled))
            .is_settings_visible(false)
            .is_back_button_visible(NavigationViewBackButtonVisible::Collapsed)
            .open_pane_length(280.0)
            .on_selected_tag_changed(ctx.callback(Msg::Navigate))
            .slots([
                SlotView::collection(NavigationViewSlot::MenuItems, views::nav_items(&self.page)),
                SlotView::new(
                    NavigationViewSlot::PaneFooter,
                    views::account_button(
                        ctx,
                        account.and_then(|a| a.display_name.as_deref()),
                        account.and_then(|a| a.upn.as_deref()),
                        self.account_menu(),
                        self.is_pane_open,
                    ),
                ),
                SlotView::new(NavigationViewSlot::Content, content),
            ]);

        let search_box = AutoSuggestBox::new()
            .text(&self.search)
            .width(360.0)
            .horizontal_alignment(HorizontalAlignment::Left)
            .vertical_alignment(VerticalAlignment::Center)
            .placeholder_text("Search or enter a PC name to connect")
            .items_source(self.suggestions())
            .on_text_changed(ctx.callback(Msg::Search))
            .on_suggestion_chosen(ctx.callback(Msg::Search));

        let top_bar = TitleBar::new()
            .grid_row(0)
            .title(APP_NAME)
            .slots([SlotView::new(TitleBarSlot::Content, search_box)]);

        let dialog_view =
            if let (Some(dialog), Some(active)) = (self.dialogs.front(), &self.active_dialog) {
                KeyedView::new(dialog.serial, active.view(ctx))
            } else {
                KeyedView::new("dialog_idle", ContentDialog::new().is_open(false))
            };

        let children: Vec<KeyedView> = vec![
            KeyedView::new("topbar", top_bar),
            KeyedView::new("navigation", navigation),
            dialog_view,
        ];
        Grid::new()
            .rows([GridLength::Auto, GridLength::Star(1.0)])
            .keyed_children(children)
    }
}

fn switch_label(display_name: Option<&str>, upn: &str) -> String {
    match display_name {
        Some(name) if name != upn => format!("{SWITCH_PREFIX}{name} ({upn})"),
        _ => format!("{SWITCH_PREFIX}{upn}"),
    }
}

fn icon_label(glyph: &str, label: &str) -> View {
    StackPanel::new()
        .orientation(Orientation::Horizontal)
        .spacing(8.0)
        .children((FontIcon::new().glyph(glyph), TextBlock::new().text(label)))
}

fn owned_rows(rows: Vec<(&'static str, String)>) -> Vec<(String, String)> {
    rows.into_iter()
        .map(|(label, value)| (label.to_string(), value))
        .collect()
}

/// The monitor of the main window in device pixels and its scale.
fn display_info() -> DisplayInfo {
    let Some(hwnd) = win32::main_window() else {
        return DisplayInfo::default();
    };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `hwnd` is a live window of this thread and `info` is a properly sized
    // MONITORINFO that outlives the call.
    let (found, dpi) = unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        (
            GetMonitorInfoW(monitor, &mut info).as_bool(),
            GetDpiForWindow(hwnd),
        )
    };
    let (width, height) = win32::rect_size(&info.rcMonitor);
    DisplayInfo {
        device_size: (found && width > 0 && height > 0)
            .then(|| (width.unsigned_abs(), height.unsigned_abs())),
        scale_percent: (dpi > 0).then(|| dpi * 100 / 96),
    }
}

/// Reads the saved passwords of `connection` (keyring access; off the UI thread).
fn load_secrets(connection: &SavedConnection) -> Credentials {
    let id = &connection.id;
    let read = || -> Result<Credentials, secrets::SecretError> {
        Ok(Credentials {
            password: secrets::load_password(&SecretKey::password(id))?,
            gateway_password: if connection.gateway_needs_password() {
                secrets::load_password(&SecretKey::gateway(id))?
            } else {
                None
            },
        })
    };
    read().unwrap_or_else(|e| {
        log::warn!("Could not read the saved password: {e}");
        Credentials::default()
    })
}

/// Saves (or clears) the passwords of connection `id` in the background.
fn store_secrets(id: &str, credentials: &Credentials) {
    let id = id.to_string();
    let credentials = credentials.clone();
    std::thread::spawn(move || {
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

/// Removes the saved passwords of connection `id` in the background.
fn forget_secrets(id: &str) {
    let id = id.to_string();
    std::thread::spawn(move || secrets::delete_connection_secrets(&id));
}
