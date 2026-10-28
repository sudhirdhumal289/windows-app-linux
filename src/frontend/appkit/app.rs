//! Application state shared by the main window, its menus and dialogs: the library
//! (Cloud PCs, Azure Virtual Desktop, Remote PCs), loading, search and accounts.

use super::main_window::{MainWindow, ViewMode};
use super::util;
use crate::api::{CloudPcService, GraphApiClient};
use crate::auth::AuthManager;
use crate::config::AppConfig;
use crate::connections::ConnectionStore;
use crate::frontend::common::library::{self, Item, Library, SearchFilter, SectionView};
use crate::frontend::common::text;
use crate::history::RecentStore;
use crate::launcher::LaunchTarget;
use crate::overrides::OverrideStore;
use objc2::MainThreadMarker;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

/// What the sidebar selection shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Filter {
    All,
    Favorites,
    /// One of the library sections ([`library::SECTION_CLOUD_PCS`], …).
    Section(&'static str),
    /// Remote PCs of one group.
    Group(String),
}

/// Counts and groups for the sidebar.
pub(super) struct SidebarCounts {
    pub favorites: usize,
    pub cloud_pcs: usize,
    pub avd: usize,
    pub remote_pcs: usize,
    pub groups: Vec<(String, usize)>,
}

pub(super) struct App {
    pub mtm: MainThreadMarker,
    pub auth: Arc<AuthManager>,
    pub service: Arc<CloudPcService>,
    pub connections: ConnectionStore,
    pub overrides: OverrideStore,
    pub recent: RecentStore,
    pub library: RefCell<Library>,
    /// Ids of items with a running action (connecting, restarting, …).
    pub busy: RefCell<HashSet<String>>,
    collapsed: RefCell<HashSet<String>>,
    loading: Cell<bool>,
    pub pending_target: RefCell<Option<LaunchTarget>>,
    search: RefCell<String>,
    mode: Cell<ViewMode>,
    filter: RefCell<Filter>,
    window: OnceCell<Rc<MainWindow>>,
}

thread_local! {
    static APP: OnceCell<Rc<App>> = const { OnceCell::new() };
}

/// The application state, once started.
pub(super) fn get() -> Option<Rc<App>> {
    APP.with(|app| app.get().cloned())
}

/// Runs `f` with the application state (UI callbacks).
pub(super) fn with(f: impl FnOnce(&Rc<App>)) {
    if let Some(app) = get() {
        f(&app);
    }
}

impl App {
    /// Creates the state and the main window.
    pub(super) fn start(mtm: MainThreadMarker) -> Rc<Self> {
        let config = AppConfig::load();
        let auth = Arc::new(AuthManager::for_profile(config.auth_profile()));
        let graph = GraphApiClient::new(
            Arc::clone(&auth),
            &config.data_boundary,
            config.cloud_environment,
        );
        let service = Arc::new(CloudPcService::new(graph));
        let connections = ConnectionStore::default();
        let library = Library {
            remote_pcs: connections.load(),
            ..Library::default()
        };
        let app = Rc::new(Self {
            mtm,
            auth,
            service,
            connections,
            overrides: OverrideStore::default(),
            recent: RecentStore::default(),
            library: RefCell::new(library),
            busy: RefCell::default(),
            collapsed: RefCell::new(config.collapsed_sections.iter().cloned().collect()),
            loading: Cell::new(false),
            pending_target: RefCell::new(None),
            search: RefCell::default(),
            mode: Cell::new(ViewMode::from_config(&config.view_mode)),
            filter: RefCell::new(Filter::All),
            window: OnceCell::new(),
        });
        APP.with(|cell| {
            if cell.set(Rc::clone(&app)).is_err() {
                log::warn!("The application state was already created");
            }
        });
        let window = MainWindow::new(mtm, app.mode.get());
        if app.window.set(window).is_err() {
            log::warn!("The main window was already created");
        }
        app.update_account_menu();
        app.refresh();
        app
    }

    pub(super) fn window(&self) -> Option<&Rc<MainWindow>> {
        self.window.get()
    }

    /// The main window for sheets, if it is visible.
    pub(super) fn sheet_parent(&self) -> Option<objc2::rc::Retained<objc2_app_kit::NSWindow>> {
        self.window()
            .map(|w| w.window.clone())
            .filter(|w| w.isVisible())
    }

    pub(super) fn present(&self) {
        if let Some(window) = self.window() {
            window.present();
        }
    }

    /// Shows `message` as the main window's subtitle.
    pub(super) fn set_status(&self, message: &str) {
        if let Some(window) = self.window() {
            window.set_status(message);
        }
    }

    /// Shows an error in the status line and as an alert.
    pub(super) fn show_error(&self, heading: &str, body: &str) {
        self.set_status(&format!("{heading}: {body}"));
        util::inform(self.mtm, self.sheet_parent().as_deref(), heading, body);
    }

    pub(super) fn signed_in(&self) -> bool {
        self.auth.is_signed_in()
    }

    pub(super) fn is_loading(&self) -> bool {
        self.loading.get()
    }

    // ----- View -----

    pub(super) fn mode(&self) -> ViewMode {
        self.mode.get()
    }

    pub(super) fn set_mode(&self, mode: ViewMode) {
        self.mode.set(mode);
        let mut config = AppConfig::load();
        mode.config_id().clone_into(&mut config.view_mode);
        if let Err(e) = config.save() {
            log::warn!("Could not save the view mode: {e}");
        }
        self.refresh();
    }

    pub(super) fn set_search(&self, text: &str) {
        if *self.search.borrow() == text {
            return;
        }
        text.clone_into(&mut self.search.borrow_mut());
        self.refresh();
    }

    pub(super) fn search_text(&self) -> String {
        self.search.borrow().clone()
    }

    pub(super) fn set_filter(&self, filter: Filter) {
        if *self.filter.borrow() == filter {
            return;
        }
        *self.filter.borrow_mut() = filter;
        self.refresh();
    }

    pub(super) fn set_section_collapsed(&self, id: &str, collapsed: bool) {
        let changed = if collapsed {
            self.collapsed.borrow_mut().insert(id.to_string())
        } else {
            self.collapsed.borrow_mut().remove(id)
        };
        if !changed {
            return;
        }
        let mut config = AppConfig::load();
        let mut ids: Vec<String> = self.collapsed.borrow().iter().cloned().collect();
        ids.sort();
        config.collapsed_sections = ids;
        if let Err(e) = config.save() {
            log::warn!("Could not save the collapsed sections: {e}");
        }
        self.refresh();
    }

    pub(super) fn set_busy(&self, id: &str, busy: bool) {
        let changed = if busy {
            self.busy.borrow_mut().insert(id.to_string())
        } else {
            self.busy.borrow_mut().remove(id)
        };
        if changed {
            self.refresh();
        }
    }

    pub(super) fn is_busy(&self, id: &str) -> bool {
        self.busy.borrow().contains(id)
    }

    /// The visible sections for the search text and the sidebar selection.
    pub(super) fn sections(&self) -> Vec<SectionView> {
        let filter = SearchFilter::new(&self.search.borrow());
        let recent = self.recent.load();
        let sections = self.library.borrow().sections(
            &filter,
            &self.collapsed.borrow(),
            &self.busy.borrow(),
            &recent,
        );
        match &*self.filter.borrow() {
            Filter::All => sections,
            Filter::Favorites => sections
                .into_iter()
                .map(|mut s| {
                    s.cards.retain(|c| c.is_favorite);
                    s.expanded = true;
                    s
                })
                .filter(|s| !s.cards.is_empty())
                .collect(),
            Filter::Section(id) => sections
                .into_iter()
                .filter(|s| s.id == *id)
                .map(|s| SectionView {
                    expanded: true,
                    ..s
                })
                .collect(),
            Filter::Group(group) => sections
                .into_iter()
                .filter(|s| s.id == library::SECTION_REMOTE_PCS)
                .map(|mut s| {
                    s.cards.retain(
                        |c| matches!(&c.item, Item::Remote(connection) if connection.folder_or_group() == group.as_str()),
                    );
                    s.expanded = true;
                    s
                })
                .filter(|s| !s.cards.is_empty())
                .collect(),
        }
    }

    pub(super) fn sidebar_counts(&self) -> SidebarCounts {
        let filter = SearchFilter::new("");
        let recent = self.recent.load();
        let all_sections =
            self.library
                .borrow()
                .sections(&filter, &HashSet::new(), &HashSet::new(), &recent);
        let favorites = all_sections
            .iter()
            .flat_map(|s| &s.cards)
            .filter(|c| c.is_favorite)
            .count();

        let library = self.library.borrow();
        let mut groups: Vec<(String, usize)> = Vec::new();
        for connection in &library.remote_pcs {
            let folder = connection.folder_or_group();
            if folder.is_empty() {
                continue;
            }
            match groups.iter_mut().find(|(g, _)| g.as_str() == folder) {
                Some((_, count)) => *count += 1,
                None => groups.push((folder.to_string(), 1)),
            }
        }
        groups.sort_by_key(|(g, _)| g.to_lowercase());
        SidebarCounts {
            favorites,
            cloud_pcs: library.cloud_pcs.len(),
            avd: library.avd.len(),
            remote_pcs: library.remote_pcs.len(),
            groups,
        }
    }

    /// Re-renders the main window from the current state.
    pub(super) fn refresh(&self) {
        let Some(window) = self.window() else { return };
        let has_items = {
            let library = self.library.borrow();
            !library.cloud_pcs.is_empty()
                || !library.avd.is_empty()
                || !library.remote_pcs.is_empty()
        };
        // A group that no longer exists falls back to everything.
        let counts = self.sidebar_counts();
        let stale_group = matches!(
            &*self.filter.borrow(),
            Filter::Group(g) if !counts.groups.iter().any(|(name, _)| name == g)
        );
        if stale_group {
            *self.filter.borrow_mut() = Filter::All;
        }
        let sections = self.sections();
        window.render(&super::main_window::RenderState {
            sections: &sections,
            mode: self.mode.get(),
            signed_in: self.signed_in(),
            loading: self.loading.get(),
            has_items,
            searching: !SearchFilter::new(&self.search.borrow()).is_empty(),
            counts: &counts,
            filter: &self.filter.borrow(),
        });
        let search = self.search.borrow().clone();
        if !SearchFilter::new(&search).is_empty() {
            let shown = sections.iter().map(|s| s.cards.len()).sum();
            window.set_status(&library::search_status(&search, shown));
        }
    }

    // ----- Accounts and loading -----

    pub(super) fn update_account_menu(&self) {
        if let Some(window) = self.window() {
            window.set_account_menu(&super::menus::account_menu(self));
        }
    }

    /// Loads the Cloud PCs and the Azure Virtual Desktop feed of the active account.
    pub(super) fn load(self: &Rc<Self>) {
        self.update_account_menu();
        if !self.signed_in() {
            {
                let mut library = self.library.borrow_mut();
                library.cloud_pcs.clear();
                library.avd.clear();
            }
            self.refresh();
            if let Some(target) = self.pending_target.take() {
                self.connect_target(target);
            }
            return;
        }
        if self.loading.replace(true) {
            return;
        }
        self.set_status("Fetching Cloud PCs from Windows 365…");
        self.refresh();
        let service = Arc::clone(&self.service);
        let app = Rc::clone(self);
        util::spawn(
            async move { library::load_feeds(&service).await },
            move |feeds| {
                app.loading.set(false);
                match feeds.cloud_pcs {
                    Ok(pcs) => {
                        log::debug!(
                            "Loaded {} Cloud PCs and {} AVD resources",
                            pcs.len(),
                            feeds.avd.len()
                        );
                        app.set_status(&text::load_summary(pcs.len(), feeds.avd.len()));
                        let mut library = app.library.borrow_mut();
                        library.cloud_pcs = pcs;
                        library.avd = feeds.avd;
                    }
                    Err(e) => {
                        log::error!("Loading Cloud PCs failed: {e}");
                        app.set_status(&text::load_error_message(&e));
                        let mut library = app.library.borrow_mut();
                        library.cloud_pcs.clear();
                        library.avd = feeds.avd;
                    }
                }
                app.update_account_menu();
                app.refresh();
                if let Some(target) = app.pending_target.take() {
                    if app.library.borrow().find(&target).is_some() {
                        app.connect_target(target);
                    } else {
                        app.report_missing(&target);
                    }
                }
            },
        );
    }

    pub(super) fn report_missing(&self, target: &LaunchTarget) {
        log::warn!("Launch target {target} is not available");
        self.set_status(&format!("{target} is no longer available to this account."));
    }

    pub(super) fn switch_account(self: &Rc<Self>, upn: &str) {
        match self.auth.switch_account(upn) {
            Ok(true) => {
                {
                    let mut library = self.library.borrow_mut();
                    library.cloud_pcs.clear();
                    library.avd.clear();
                }
                self.set_status(&format!("Switched to {upn}. Loading Cloud PCs…"));
                self.load();
            }
            Ok(false) => {
                self.set_status(&format!("{upn} needs to sign in again."));
                self.update_account_menu();
            }
            Err(e) => self.set_status(&format!("Could not switch accounts: {e}")),
        }
    }

    pub(super) fn sign_out(self: &Rc<Self>) {
        if let Err(e) = self.auth.sign_out() {
            log::error!("Sign-out failed: {e}");
            self.set_status(&format!("Could not remove the stored sign-in: {e}"));
            return;
        }
        AppConfig::remove_legacy_files();
        {
            let mut library = self.library.borrow_mut();
            library.cloud_pcs.clear();
            library.avd.clear();
        }
        self.update_account_menu();
        self.refresh();
        self.set_status("Signed out.");
    }

    /// Microsoft sign-in (the sheet also offers a device code).
    pub(super) fn sign_in(self: &Rc<Self>) {
        self.present();
        self.set_status("Opening Microsoft sign-in…");
        let Some(parent) = self.window().map(|w| w.window.clone()) else {
            return;
        };
        let app = Rc::clone(self);
        super::signin::sign_in(&parent, move |signed_in| {
            if signed_in {
                // The new sign-in may be a different account.
                app.auth.forget_scoped_tokens();
                app.set_status("Signed in. Loading Cloud PCs…");
                app.load();
            } else {
                app.set_status("Ready");
                app.update_account_menu();
            }
        });
    }
}
