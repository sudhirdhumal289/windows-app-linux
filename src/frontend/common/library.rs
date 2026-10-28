//! The connection library: Cloud PCs, Azure Virtual Desktop resources and saved Remote
//! PCs, grouped into collapsible sections and filtered by the search text. Frontends turn
//! the [`SectionView`]s into native cards or list rows and the [`MenuItem`]s into native
//! menus, then send the chosen [`ItemAction`] back to their controller.

use crate::api::{self, AvdResource, AvdResourceKind, CloudPcService};
use crate::connections::{self, SavedConnection};
use crate::history;
use crate::launcher::LaunchTarget;
use crate::models::{CloudPc, CloudPcAction, StatusTone};
use std::collections::{BTreeMap, HashSet};

/// Ids of the main window sections, remembered in `AppConfig::collapsed_sections`.
pub const SECTION_CLOUD_PCS: &str = "cloud-pcs";
pub const SECTION_AVD: &str = "avd";
pub const SECTION_REMOTE_PCS: &str = "remote-pcs";

/// Case-insensitive search terms; a connection matches when every term appears in one
/// of its fields.
#[derive(Debug, Clone, Default)]
pub struct SearchFilter(Vec<String>);

impl SearchFilter {
    #[must_use]
    pub fn new(text: &str) -> Self {
        Self(text.split_whitespace().map(str::to_lowercase).collect())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn matches(&self, fields: &[&str]) -> bool {
        let fields: Vec<String> = fields.iter().map(|f| f.to_lowercase()).collect();
        self.0
            .iter()
            .all(|term| fields.iter().any(|field| field.contains(term.as_str())))
    }
}

/// Result of loading the account's feeds.
#[derive(Debug)]
pub struct FeedLoad {
    pub cloud_pcs: crate::Result<Vec<CloudPc>>,
    /// Azure Virtual Desktop resources that are not also listed as Cloud PCs.
    pub avd: Vec<AvdResource>,
}

/// Loads the Cloud PCs and the (optional) Azure Virtual Desktop feed of the signed-in
/// account. Cloud PCs are published in the feed too; they are listed only once.
pub async fn load_feeds(service: &CloudPcService) -> FeedLoad {
    let result = service.list_user_cloud_pcs().await;
    let avd = match service.list_avd_resources().await {
        Ok(resources) => resources,
        Err(e) => {
            log::warn!("Loading the Azure Virtual Desktop feed failed: {e}");
            Vec::new()
        }
    };
    let avd = match &result {
        Ok(pcs) => {
            let cloud_ids: HashSet<String> =
                pcs.iter().filter_map(api::launch_resource_id).collect();
            avd.into_iter()
                .filter(|r| !cloud_ids.contains(&r.id))
                .collect()
        }
        Err(_) => avd,
    };
    FeedLoad {
        cloud_pcs: result,
        avd,
    }
}

/// Everything shown in the main window.
#[derive(Debug, Clone, Default)]
pub struct Library {
    pub cloud_pcs: Vec<CloudPc>,
    pub avd: Vec<AvdResource>,
    pub remote_pcs: Vec<SavedConnection>,
}

/// One connection of the library.
#[derive(Debug, Clone, PartialEq, Eq)]
// Items are short-lived view-model values; boxing would only add indirection for callers.
#[allow(clippy::large_enum_variant)]
pub enum Item {
    CloudPc(CloudPc),
    Avd(AvdResource),
    Remote(SavedConnection),
}

/// Icon of a card; frontends map these to platform symbols (Segoe Fluent Icons, SF Symbols,
/// symbolic icon names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemIcon {
    /// Windows logo (Cloud PCs).
    Windows,
    /// A published desktop.
    Desktop,
    /// A published RemoteApp.
    App,
    /// A directly reached Remote PC.
    RemotePc,
}

/// Action offered for a library item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemAction {
    Connect,
    Start,
    Stop,
    Restart,
    Rename,
    OpenInBrowser,
    Troubleshoot,
    Inspect,
    Details,
    Restore,
    Reset,
    Edit,
    Export,
    ForgetPassword,
    Remove,
    ShowInLauncher,
    ConnectAtLogin,
    ConnectionSettings,
    Preferences,
    ToggleFavorite,
}

impl ItemAction {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Connect => "Connect",
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Restart => "Restart",
            Self::Rename => "Rename…",
            Self::OpenInBrowser => "Open in Web Browser",
            Self::Troubleshoot => "Troubleshoot",
            Self::Inspect => "Inspect Connection",
            Self::Details => "View Details",
            Self::Restore => "Restore…",
            Self::Reset => "Reset…",
            Self::Edit => "Edit…",
            Self::Export => "Export as .rdp File…",
            Self::ForgetPassword => "Forget Password",
            Self::Remove => "Remove…",
            Self::ShowInLauncher => "Show in App Launcher",
            Self::ConnectAtLogin => "Connect at Login",
            Self::ConnectionSettings => "Connection Settings…",
            Self::Preferences => "Preferences",
            Self::ToggleFavorite => "Toggle Favorite",
        }
    }

    /// Whether the action removes data or interrupts work (shown in red).
    #[must_use]
    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            Self::Restart | Self::Restore | Self::Reset | Self::Remove
        )
    }

    /// The core action behind this menu item, for Cloud PC remote actions.
    #[must_use]
    pub fn cloud_pc_action(self) -> Option<CloudPcAction> {
        Some(match self {
            Self::Start => CloudPcAction::Start,
            Self::Stop => CloudPcAction::Stop,
            Self::Restart => CloudPcAction::Restart,
            Self::Rename => CloudPcAction::Rename,
            Self::Troubleshoot => CloudPcAction::Troubleshoot,
            Self::Restore => CloudPcAction::Restore,
            Self::Reset => CloudPcAction::Reset,
            _ => return None,
        })
    }
}

/// Entry of an item's action menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    Action {
        action: ItemAction,
        enabled: bool,
    },
    /// On/off entry (app launcher entry, connect at login).
    Toggle {
        action: ItemAction,
        checked: bool,
    },
    Separator,
}

impl Item {
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::CloudPc(pc) => &pc.id,
            Self::Avd(resource) => &resource.id,
            Self::Remote(connection) => &connection.id,
        }
    }

    #[must_use]
    pub fn target(&self) -> LaunchTarget {
        match self {
            Self::CloudPc(pc) => LaunchTarget::CloudPc(pc.id.clone()),
            Self::Avd(resource) => LaunchTarget::Avd(resource.id.clone()),
            Self::Remote(connection) => LaunchTarget::RemotePc(connection.id.clone()),
        }
    }

    #[must_use]
    pub fn title(&self) -> &str {
        match self {
            Self::CloudPc(pc) => &pc.display_name,
            Self::Avd(resource) => &resource.title,
            Self::Remote(connection) => connection.display_name(),
        }
    }

    /// Account line (UPN, username or AVD folder).
    #[must_use]
    pub fn account(&self) -> Option<String> {
        match self {
            Self::CloudPc(pc) => pc.user_principal_name.clone(),
            Self::Avd(resource) => resource.folder.clone(),
            Self::Remote(connection) => Some(connection.username.clone()).filter(|u| !u.is_empty()),
        }
    }

    /// Kind shown under the icon (OS name, "Remote PC · Group", "Desktop").
    #[must_use]
    pub fn kind(&self) -> String {
        match self {
            Self::CloudPc(pc) => pc
                .os_name
                .as_deref()
                .or(pc.image_display_name.as_deref())
                .unwrap_or("Windows")
                .to_string(),
            Self::Avd(resource) => resource.kind_label().to_string(),
            Self::Remote(connection) => {
                match (connection.folder_or_group(), &connection.remote_app) {
                    ("", None) => "Remote PC".to_string(),
                    ("", Some(_)) => "RemoteApp".to_string(),
                    (group, None) => format!("Remote PC · {group}"),
                    (group, Some(_)) => format!("RemoteApp · {group}"),
                }
            }
        }
    }

    /// Short technical summary (hardware, workspace, address) and its label.
    #[must_use]
    pub fn detail(&self) -> (&'static str, String) {
        match self {
            Self::CloudPc(pc) => ("Spec: ", pc.hardware_summary().to_string()),
            Self::Avd(resource) => ("Workspace: ", resource.workspace.clone()),
            Self::Remote(connection) => ("", connection.address.clone()),
        }
    }

    #[must_use]
    pub fn icon(&self) -> ItemIcon {
        match self {
            Self::CloudPc(_) => ItemIcon::Windows,
            Self::Avd(resource) => match resource.kind {
                AvdResourceKind::Desktop => ItemIcon::Desktop,
                AvdResourceKind::RemoteApp => ItemIcon::App,
            },
            Self::Remote(_) => ItemIcon::RemotePc,
        }
    }

    /// Status badge (Cloud PCs only).
    #[must_use]
    pub fn status(&self) -> Option<(String, StatusTone)> {
        match self {
            Self::CloudPc(pc) => Some(pc.status_summary(chrono::Utc::now())),
            _ => None,
        }
    }

    #[must_use]
    pub fn connect_tooltip(&self) -> &'static str {
        match self {
            Self::CloudPc(_) => "Click to connect to Cloud PC",
            Self::Avd(resource) => match resource.kind {
                AvdResourceKind::Desktop => "Click to connect to the desktop",
                AvdResourceKind::RemoteApp => "Click to start the app",
            },
            Self::Remote(_) => "Click to connect to Remote PC",
        }
    }

    fn search_fields(&self) -> Vec<&str> {
        match self {
            Self::CloudPc(pc) => vec![
                &pc.display_name,
                pc.user_principal_name.as_deref().unwrap_or_default(),
            ],
            Self::Avd(resource) => vec![
                &resource.title,
                &resource.workspace,
                resource.folder.as_deref().unwrap_or_default(),
            ],
            Self::Remote(c) => vec![
                c.display_name(),
                &c.address,
                c.folder_or_group(),
                &c.username,
            ],
        }
    }

    /// The item's action menu, in sections. `launcher` and `autostart` are the states of
    /// the desktop integration toggles; `busy` disables actions while one is running.
    #[must_use]
    pub fn menu(&self, launcher: bool, autostart: bool, busy: bool) -> Vec<MenuItem> {
        let action = |action, enabled| MenuItem::Action { action, enabled };
        let mut items = Vec::new();
        match self {
            Self::CloudPc(pc) => {
                items.push(action(ItemAction::Connect, !busy));
                if pc.allows(CloudPcAction::Start) || pc.allows(CloudPcAction::Stop) {
                    items.push(action(
                        ItemAction::Start,
                        !busy && pc.allows(CloudPcAction::Start),
                    ));
                    items.push(action(
                        ItemAction::Stop,
                        !busy && pc.allows(CloudPcAction::Stop),
                    ));
                }
                items.push(action(
                    ItemAction::Restart,
                    !busy && pc.allows(CloudPcAction::Restart),
                ));
                items.push(action(
                    ItemAction::Rename,
                    !busy && pc.allows(CloudPcAction::Rename),
                ));
                if pc.direct_launch_url.is_some() {
                    items.push(action(ItemAction::OpenInBrowser, true));
                }
                items.push(MenuItem::Separator);
                items.push(action(
                    ItemAction::Troubleshoot,
                    !busy && pc.allows(CloudPcAction::Troubleshoot),
                ));
                items.push(action(ItemAction::Inspect, true));
                items.push(action(ItemAction::Details, true));
                items.push(MenuItem::Separator);
                items.push(action(
                    ItemAction::Restore,
                    !busy && pc.allows(CloudPcAction::Restore),
                ));
                items.push(action(
                    ItemAction::Reset,
                    !busy && pc.allows(CloudPcAction::Reset),
                ));
            }
            Self::Avd(_) => items.push(action(ItemAction::Connect, !busy)),
            Self::Remote(connection) => {
                items.push(action(ItemAction::Connect, true));
                items.push(action(ItemAction::Edit, true));
                items.push(action(ItemAction::Export, true));
                if connection.remember_password {
                    items.push(action(ItemAction::ForgetPassword, true));
                }
            }
        }
        items.push(MenuItem::Separator);
        items.push(MenuItem::Toggle {
            action: ItemAction::ShowInLauncher,
            checked: launcher,
        });
        items.push(MenuItem::Toggle {
            action: ItemAction::ConnectAtLogin,
            checked: autostart,
        });
        if matches!(self, Self::Remote(_)) {
            items.push(MenuItem::Separator);
            items.push(action(ItemAction::Remove, true));
        }
        items.push(MenuItem::Separator);
        items.push(action(ItemAction::ToggleFavorite, true));
        items.push(action(ItemAction::ConnectionSettings, true));
        items.push(action(ItemAction::Preferences, true));
        items
    }
}

/// One card or list row.
#[derive(Debug, Clone)]
pub struct CardView {
    pub item: Item,
    pub title: String,
    pub account: Option<String>,
    pub kind: String,
    pub detail_prefix: &'static str,
    pub detail: String,
    pub icon: ItemIcon,
    pub status: Option<(String, StatusTone)>,
    /// When the connection was last used ("2 hours ago").
    pub last_used: Option<String>,
    /// A connection or action is in progress.
    pub busy: bool,
    pub tooltip: &'static str,
    pub is_favorite: bool,
    pub tags: Vec<String>,
    pub folder: Option<String>,
}

impl CardView {
    /// List-row subtitle: "kind • detail • last used", then the account.
    #[must_use]
    pub fn subtitle(&self) -> String {
        let mut subtitle = format!("{} • {}", self.kind, self.detail);
        if let Some(last_used) = &self.last_used {
            subtitle.push_str(" • ");
            subtitle.push_str(last_used);
        }
        if !self.tags.is_empty() {
            subtitle.push_str(" • [");
            subtitle.push_str(&self.tags.join(", "));
            subtitle.push(']');
        }
        if let Some(account) = &self.account {
            subtitle.push('\n');
            subtitle.push_str(account);
        }
        subtitle
    }
}

/// A titled, collapsible group of cards.
#[derive(Debug, Clone)]
pub struct SectionView {
    pub id: &'static str,
    pub title: &'static str,
    pub expanded: bool,
    pub cards: Vec<CardView>,
}

impl Library {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cloud_pcs.is_empty() && self.avd.is_empty() && self.remote_pcs.is_empty()
    }

    /// Finds the item behind `target`.
    #[must_use]
    pub fn find(&self, target: &LaunchTarget) -> Option<Item> {
        match target {
            LaunchTarget::CloudPc(id) => self
                .cloud_pcs
                .iter()
                .find(|pc| &pc.id == id)
                .cloned()
                .map(Item::CloudPc),
            LaunchTarget::Avd(id) => self
                .avd
                .iter()
                .find(|r| &r.id == id)
                .cloned()
                .map(Item::Avd),
            LaunchTarget::RemotePc(id) => self
                .remote_pcs
                .iter()
                .find(|c| &c.id == id)
                .cloned()
                .map(Item::Remote),
        }
    }

    /// The non-empty sections matching `filter`. Remote PCs are ordered by group, then
    /// name. Searching expands every section.
    #[must_use]
    pub fn sections(
        &self,
        filter: &SearchFilter,
        collapsed: &HashSet<String>,
        busy: &HashSet<String>,
        recent: &BTreeMap<String, chrono::DateTime<chrono::Utc>>,
    ) -> Vec<SectionView> {
        let now = chrono::Utc::now();
        let store = crate::overrides::OverrideStore::default();
        let all_overrides = store.load_all();

        let card = |item: Item| -> Option<CardView> {
            let target = item.target();
            let ov = all_overrides.get(&target.to_string());
            let is_favorite = match &item {
                Item::Remote(c) => c.is_favorite || ov.is_some_and(|o| o.is_favorite),
                _ => ov.is_some_and(|o| o.is_favorite),
            };
            let tags = match &item {
                Item::Remote(c) => {
                    if !c.tags.is_empty() {
                        c.tags.clone()
                    } else {
                        ov.map(|o| o.tags.clone()).unwrap_or_default()
                    }
                }
                _ => ov.map(|o| o.tags.clone()).unwrap_or_default(),
            };
            let folder = match &item {
                Item::Remote(c) => {
                    let f = c.folder_or_group();
                    if !f.is_empty() {
                        Some(f.to_string())
                    } else {
                        ov.and_then(|o| o.folder.clone())
                    }
                }
                Item::Avd(r) => ov
                    .and_then(|o| o.folder.clone())
                    .or_else(|| r.folder.clone()),
                Item::CloudPc(_) => ov.and_then(|o| o.folder.clone()),
            };

            let mut fields = item.search_fields();
            for t in &tags {
                fields.push(t.as_str());
            }
            if let Some(f) = folder.as_deref() {
                fields.push(f);
            }
            if !filter.matches(&fields) {
                return None;
            }
            let (detail_prefix, detail) = item.detail();
            Some(CardView {
                title: item.title().to_string(),
                account: item.account(),
                kind: item.kind(),
                detail_prefix,
                detail,
                icon: item.icon(),
                status: item.status(),
                last_used: recent
                    .get(&item.target().to_string())
                    .map(|when| history::relative_time(*when, now)),
                busy: busy.contains(item.id()),
                tooltip: item.connect_tooltip(),
                is_favorite,
                tags,
                folder,
                item,
            })
        };
        let mut remote_pcs = self.remote_pcs.clone();
        remote_pcs.sort_by_cached_key(|c| {
            (
                c.folder_or_group().is_empty(),
                c.folder_or_group().to_lowercase(),
                c.display_name().to_lowercase(),
            )
        });
        let groups: [(&'static str, &'static str, Vec<CardView>); 3] = [
            (
                SECTION_CLOUD_PCS,
                "Cloud PCs",
                self.cloud_pcs
                    .iter()
                    .cloned()
                    .filter_map(|pc| card(Item::CloudPc(pc)))
                    .collect(),
            ),
            (
                SECTION_AVD,
                "Azure Virtual Desktop",
                self.avd
                    .iter()
                    .cloned()
                    .filter_map(|r| card(Item::Avd(r)))
                    .collect(),
            ),
            (
                SECTION_REMOTE_PCS,
                "Remote PCs",
                remote_pcs
                    .into_iter()
                    .filter_map(|c| card(Item::Remote(c)))
                    .collect(),
            ),
        ];
        groups
            .into_iter()
            .filter(|(_, _, cards)| !cards.is_empty())
            .map(|(id, title, cards)| SectionView {
                id,
                title,
                expanded: !filter.is_empty() || !collapsed.contains(id),
                cards,
            })
            .collect()
    }
}

/// Status line while searching: number of matches and whether Enter connects.
#[must_use]
pub fn search_status(text: &str, shown: usize) -> String {
    match (shown, connections::parse_address(text).is_ok()) {
        (0, true) => format!("No matches. Press Enter to connect to {}.", text.trim()),
        (0, false) => "No matches.".to_string(),
        (1, _) => "1 match. Press Enter to connect.".to_string(),
        (n, _) => format!("{n} matches."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Library {
        let mut office = SavedConnection::new("Office", "office.example.com", "me").unwrap();
        office.group = "Work".into();
        let lab = SavedConnection::new("Lab", "10.0.0.5", "").unwrap();
        Library {
            cloud_pcs: vec![CloudPc {
                id: "pc1".into(),
                display_name: "Dev Box".into(),
                ..CloudPc::default()
            }],
            avd: Vec::new(),
            remote_pcs: vec![lab, office],
        }
    }

    #[test]
    fn filters_and_orders_sections() {
        let lib = library();
        let none = HashSet::new();
        let sections = lib.sections(&SearchFilter::new(""), &none, &none, &BTreeMap::new());
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].id, SECTION_CLOUD_PCS);
        // Grouped connections come first.
        assert_eq!(sections[1].cards[0].title, "Office");

        let sections = lib.sections(&SearchFilter::new("lab"), &none, &none, &BTreeMap::new());
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].cards[0].title, "Lab");
    }

    #[test]
    fn search_expands_collapsed_sections() {
        let lib = library();
        let collapsed: HashSet<String> = [SECTION_REMOTE_PCS.to_string()].into();
        let none = HashSet::new();
        let sections = lib.sections(&SearchFilter::new(""), &collapsed, &none, &BTreeMap::new());
        assert!(!sections[1].expanded);
        let sections = lib.sections(&SearchFilter::new("o"), &collapsed, &none, &BTreeMap::new());
        assert!(sections.iter().all(|s| s.expanded));
    }

    #[test]
    fn remote_menu_has_remove() {
        let lib = library();
        let item = Item::Remote(lib.remote_pcs[0].clone());
        let menu = item.menu(false, false, false);
        assert!(menu.contains(&MenuItem::Action {
            action: ItemAction::Remove,
            enabled: true
        }));
    }

    #[test]
    fn finds_items_by_target() {
        let lib = library();
        assert!(lib.find(&LaunchTarget::CloudPc("pc1".into())).is_some());
        assert!(lib.find(&LaunchTarget::Avd("x".into())).is_none());
    }

    #[test]
    fn matches_tags_and_folders() {
        let mut lib = library();
        let mut tagged = SavedConnection::new("Prod DB", "10.0.1.10", "admin").unwrap();
        tagged.tags = vec!["production".into(), "database".into()];
        tagged.folder = Some("Infrastructure".into());
        tagged.is_favorite = true;
        lib.remote_pcs.push(tagged);

        let none = HashSet::new();
        let filter_tag = SearchFilter::new("production");
        let sections = lib.sections(&filter_tag, &none, &none, &BTreeMap::new());
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].cards[0].title, "Prod DB");
        assert!(sections[0].cards[0].is_favorite);
        assert_eq!(
            sections[0].cards[0].folder.as_deref(),
            Some("Infrastructure")
        );
        assert_eq!(sections[0].cards[0].tags, vec!["production", "database"]);

        let filter_folder = SearchFilter::new("Infrastructure");
        let sections_folder = lib.sections(&filter_folder, &none, &none, &BTreeMap::new());
        assert_eq!(sections_folder.len(), 1);
        assert_eq!(sections_folder[0].cards[0].title, "Prod DB");
    }
}
