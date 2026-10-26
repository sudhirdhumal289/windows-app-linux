//! Forms: a two-column grid builder, sheet windows, and the Remote PC, connection
//! settings and app settings forms.

use super::app::App;
use super::util::{self, ns};
use crate::config::{AppConfig, AuthProfile};
use crate::connections::{
    AddressError, AuthMethod, CertificatePolicy, Gateway, RemoteApp, SavedConnection,
    SecurityProtocol,
};
use crate::launcher::LaunchTarget;
use crate::overrides::{ConnectionOverrides, OverrideStore, Resolution};
use crate::session::{GraphicsMode, Quality};
use objc2::rc::{Retained, Weak};
use objc2::{MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSColor, NSGridCell, NSGridCellPlacement, NSGridRow,
    NSGridRowAlignment, NSGridView, NSLayoutAttribute, NSModalResponseOK, NSOpenPanel,
    NSPopUpButton, NSStackView, NSStackViewGravity, NSTextField, NSUserInterfaceLayoutOrientation,
    NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSEdgeInsets, NSRange};
use std::cell::RefCell;
use std::rc::Rc;

/// Width of the value column of forms.
const FIELD_WIDTH: f64 = 280.0;

const GRAPHICS_MODES: [(GraphicsMode, &str); 2] = [
    (GraphicsMode::Auto, "Automatic (H.264 / AVC444)"),
    (GraphicsMode::RemoteFx, "RemoteFX (low latency LAN)"),
];

/// A two-column form: trailing-aligned labels and their controls.
pub(super) struct Form {
    grid: Retained<NSGridView>,
}

impl Form {
    pub(super) fn new(mtm: MainThreadMarker) -> Self {
        let grid = NSGridView::gridViewWithNumberOfColumns_rows(2, 0, mtm);
        grid.setRowSpacing(8.0);
        grid.setColumnSpacing(8.0);
        grid.setRowAlignment(NSGridRowAlignment::FirstBaseline);
        grid.columnAtIndex(0)
            .setXPlacement(NSGridCellPlacement::Trailing);
        grid.columnAtIndex(1).setWidth(FIELD_WIDTH);
        Self { grid }
    }

    /// An editable text field.
    pub(super) fn text_field(
        mtm: MainThreadMarker,
        text: &str,
        placeholder: &str,
    ) -> Retained<NSTextField> {
        let field = NSTextField::textFieldWithString(&ns(text), mtm);
        if !placeholder.is_empty() {
            field.setPlaceholderString(Some(&ns(placeholder)));
        }
        field
    }

    /// A pop-up button with `labels`, `selected` chosen.
    pub(super) fn popup(
        mtm: MainThreadMarker,
        labels: &[String],
        selected: usize,
    ) -> Retained<NSPopUpButton> {
        let popup = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            util::rect(0.0, 0.0, 200.0, 26.0),
            false,
        );
        let titles: Vec<Retained<objc2_foundation::NSString>> =
            labels.iter().map(|l| ns(l)).collect();
        popup.addItemsWithTitles(&NSArray::from_retained_slice(&titles));
        popup.selectItemAtIndex(isize::try_from(selected).unwrap_or(0));
        popup
    }

    /// Index of the chosen entry of a pop-up button.
    pub(super) fn selected(popup: &NSPopUpButton) -> usize {
        usize::try_from(popup.indexOfSelectedItem()).unwrap_or(0)
    }

    /// A labelled row.
    pub(super) fn row(&mut self, title: &str, view: &NSView) -> Retained<NSGridRow> {
        let mtm = self.grid.mtm();
        let label: Retained<NSView> = if title.is_empty() {
            NSGridCell::emptyContentView(mtm)
        } else {
            Retained::into_super(Retained::into_super(util::label(mtm, title)))
        };
        let views = NSArray::from_retained_slice(&[label, view.retain()]);
        self.grid.addRowWithViews(&views)
    }

    /// A row spanning both columns.
    pub(super) fn full_row(&mut self, view: &NSView) -> Retained<NSGridRow> {
        let mtm = self.grid.mtm();
        let views =
            NSArray::from_retained_slice(&[view.retain(), NSGridCell::emptyContentView(mtm)]);
        let row = self.grid.addRowWithViews(&views);
        let index = self.grid.indexOfRow(&row);
        if let Ok(index) = usize::try_from(index) {
            self.grid.mergeCellsInHorizontalRange_verticalRange(
                NSRange::new(0, 2),
                NSRange::new(index, 1),
            );
        }
        row.cellAtIndex(0)
            .setXPlacement(NSGridCellPlacement::Leading);
        row
    }

    /// A bold section title with some space above.
    pub(super) fn section(&mut self, title: &str) -> Retained<NSGridRow> {
        let label = util::label(self.grid.mtm(), title);
        util::font(&label, 13.0, true);
        let row = self.full_row(&label);
        if self.grid.numberOfRows() > 1 {
            row.setTopPadding(10.0);
        }
        row
    }

    /// Small secondary text spanning the form.
    pub(super) fn note(&mut self, text: &str) -> Retained<NSGridRow> {
        let label = util::wrapping_label(self.grid.mtm(), text, FIELD_WIDTH + 140.0);
        util::secondary(&label);
        label.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(11.0)));
        self.full_row(&label)
    }

    pub(super) fn build(self) -> Retained<NSGridView> {
        self.grid
    }
}

// ----- Sheets -----

/// A document-modal sheet on `parent` (or a centred panel when there is no visible
/// parent) holding `content` above a row of buttons.
pub(super) struct Sheet {
    pub window: Retained<NSWindow>,
    buttons: Retained<NSStackView>,
    parent: Option<Retained<NSWindow>>,
}

impl Sheet {
    pub(super) fn new(
        mtm: MainThreadMarker,
        title: &str,
        content: &NSView,
        parent: Option<Retained<NSWindow>>,
    ) -> Self {
        // SAFETY: Plain window creation; the window is not released on close.
        let window = unsafe {
            let window = NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                util::rect(0.0, 0.0, 480.0, 300.0),
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            );
            window.setReleasedWhenClosed(false);
            window
        };
        window.setTitle(&ns(title));
        let buttons = NSStackView::new(mtm);
        buttons.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        buttons.setSpacing(12.0);
        let root = NSStackView::new(mtm);
        root.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        root.setAlignment(NSLayoutAttribute::Trailing);
        root.setSpacing(20.0);
        root.setEdgeInsets(NSEdgeInsets {
            top: 20.0,
            left: 20.0,
            bottom: 20.0,
            right: 20.0,
        });
        root.addArrangedSubview(content);
        root.addArrangedSubview(&buttons);
        window.setContentView(Some(&root));
        Self {
            window,
            buttons,
            parent: parent.filter(|p| p.isVisible()),
        }
    }

    /// Adds a button; `default` buttons answer Return, "Cancel" answers Escape.
    pub(super) fn button(
        &self,
        title: &str,
        default: bool,
        action: impl Fn() + 'static,
    ) -> Retained<NSButton> {
        let button = util::button(self.window.mtm(), title, action);
        if default {
            button.setKeyEquivalent(&ns("\r"));
        } else if title == "Cancel" {
            button.setKeyEquivalent(&ns("\u{1b}"));
        }
        self.buttons
            .addView_inGravity(&button, NSStackViewGravity::Trailing);
        button
    }

    /// A weak handle for closures (avoids a window → button → closure → window cycle).
    pub(super) fn handle(&self) -> Weak<NSWindow> {
        Weak::from_retained(&self.window)
    }

    pub(super) fn show(&self) {
        if let Some(view) = self.window.contentView() {
            self.window.setContentSize(view.fittingSize());
        }
        match &self.parent {
            Some(parent) => parent.beginSheet_completionHandler(&self.window, None),
            None => {
                self.window.center();
                self.window.makeKeyAndOrderFront(None);
            }
        }
    }
}

/// Closes a window shown by [`Sheet::show`].
pub(super) fn close_sheet(window: &Weak<NSWindow>) {
    let Some(window) = window.load() else { return };
    match window.sheetParent() {
        Some(parent) => parent.endSheet(&window),
        None => window.orderOut(None),
    }
}

// ----- Remote PC -----

fn index_of<T: PartialEq>(all: &[T], value: T) -> usize {
    all.iter().position(|v| *v == value).unwrap_or(0)
}

fn pick<T: Copy + Default>(all: &[T], index: usize) -> T {
    all.get(index).copied().unwrap_or_default()
}

/// The add (`existing` is `None`) or edit form of a Remote PC.
pub(super) fn remote_pc(app: &Rc<App>, existing: Option<SavedConnection>) {
    let mtm = app.mtm;
    let editing = existing.is_some();
    let remembered = existing.as_ref().is_some_and(|c| c.remember_password);
    let base = existing.unwrap_or_else(SavedConnection::blank);

    let address = Form::text_field(mtm, &base.address, "PC name or IP address");
    let name = Form::text_field(mtm, &base.name, "Display name (optional)");
    let username = Form::text_field(mtm, &base.username, "Username (optional)");
    let error = util::wrapping_label(mtm, "", FIELD_WIDTH + 140.0);
    error.setTextColor(Some(&NSColor::systemRedColor()));
    let domain = Form::text_field(mtm, &base.domain, "Domain (optional)");
    let group = Form::text_field(mtm, &base.group, "Group (optional)");
    let remember = util::checkbox(mtm, "Remember Password", base.remember_password);

    let gateway = base.gateway.clone().unwrap_or_default();
    let use_gateway = util::checkbox(mtm, "Use a Remote Desktop Gateway", base.gateway.is_some());
    let gateway_address = Form::text_field(
        mtm,
        &gateway.address,
        "Gateway address (port 443 if omitted)",
    );
    let gateway_separate = util::checkbox(
        mtm,
        "Separate Gateway Credentials",
        gateway.separate_credentials,
    );
    let gateway_user = Form::text_field(mtm, &gateway.username, "Gateway username");

    let program = base.remote_app.clone().unwrap_or_default();
    let use_app = util::checkbox(mtm, "Start a RemoteApp program", base.remote_app.is_some());
    let app_program = Form::text_field(mtm, &program.program, "Program (path or ||alias)");
    let app_args = Form::text_field(mtm, &program.arguments, "Arguments (optional)");
    let admin = util::checkbox(mtm, "Administrative Session", base.admin_session);

    let labels = |all: &[&str]| all.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
    let security = Form::popup(
        mtm,
        &labels(&SecurityProtocol::ALL.map(SecurityProtocol::label)),
        index_of(&SecurityProtocol::ALL, base.security),
    );
    let certificate = Form::popup(
        mtm,
        &labels(&CertificatePolicy::ALL.map(CertificatePolicy::label)),
        index_of(&CertificatePolicy::ALL, base.certificate),
    );
    let auth = Form::popup(
        mtm,
        &labels(&AuthMethod::ALL.map(AuthMethod::label)),
        index_of(&AuthMethod::ALL, base.auth),
    );

    let mut form = Form::new(mtm);
    form.row("PC name:", &address);
    form.row("Display name:", &name);
    form.row("Username:", &username);
    form.note(
        "Examples: office-pc, pc01.contoso.com:3390, 192.168.1.20. Usernames can be \
         written as DOMAIN\\user or user@domain.",
    );
    let error_row = form.full_row(&error);
    form.section("Sign-in");
    form.row("Domain:", &domain);
    form.row("Group:", &group);
    form.row("", &remember);
    form.section("Session");
    form.full_row(&use_gateway);
    let gateway_rows = [
        form.row("Gateway:", &gateway_address),
        form.row("", &gateway_separate),
        form.row("Gateway user:", &gateway_user),
    ];
    form.full_row(&use_app);
    let app_rows = [
        form.row("Program:", &app_program),
        form.row("Arguments:", &app_args),
    ];
    form.row("", &admin);
    form.note("Connect to the console session.");
    form.section("Security");
    form.row("Security:", &security);
    form.row("Certificates:", &certificate);
    form.row("Authentication:", &auth);
    form.note("Kerberos needs a ticket, e.g. from kinit.");
    let grid = form.build();

    let collect = {
        let address = address.clone();
        let name = name.clone();
        let username = username.clone();
        let use_gateway = use_gateway.clone();
        let gateway_address = gateway_address.clone();
        let gateway_separate = gateway_separate.clone();
        let gateway_user = gateway_user.clone();
        let use_app = use_app.clone();
        let app_program = app_program.clone();
        let app_args = app_args.clone();
        let text = |field: &NSTextField| field.stringValue().to_string();
        Rc::new(move || -> Result<SavedConnection, AddressError> {
            let mut connection = base.clone();
            connection.update(&text(&name), &text(&address), &text(&username))?;
            connection.domain = text(&domain);
            connection.group = text(&group);
            connection.remember_password = util::is_checked(&remember);
            connection.gateway = util::is_checked(&use_gateway).then(|| Gateway {
                address: text(&gateway_address),
                username: text(&gateway_user),
                separate_credentials: util::is_checked(&gateway_separate),
            });
            connection.remote_app = util::is_checked(&use_app).then(|| RemoteApp {
                program: text(&app_program),
                arguments: text(&app_args),
            });
            connection.admin_session = util::is_checked(&admin);
            connection.security = pick(&SecurityProtocol::ALL, Form::selected(&security));
            connection.certificate = pick(&CertificatePolicy::ALL, Form::selected(&certificate));
            connection.auth = pick(&AuthMethod::ALL, Form::selected(&auth));
            connection.validate()?;
            Ok(connection)
        })
    };

    let sheet = Sheet::new(
        mtm,
        if editing {
            "Edit Remote PC"
        } else {
            "Add Remote PC"
        },
        &grid,
        app.sheet_parent(),
    );
    let handle = sheet.handle();
    sheet.button("Cancel", false, {
        let handle = handle.clone();
        move || close_sheet(&handle)
    });
    let save = sheet.button(if editing { "Save" } else { "Add" }, true, {
        let collect = Rc::clone(&collect);
        let app = Rc::clone(app);
        move || {
            let Ok(connection) = collect() else { return };
            if remembered && !connection.remember_password {
                super::connect::forget_secrets(&connection.id);
            }
            app.save_remote_pc(connection);
            close_sheet(&handle);
        }
    });

    let validate: Rc<dyn Fn()> = {
        let save = save.clone();
        let use_gateway = use_gateway.clone();
        let gateway_separate = gateway_separate.clone();
        let gateway_user = gateway_user.clone();
        let use_app = use_app.clone();
        let window = sheet.handle();
        Rc::new(move || {
            let result = collect();
            save.setEnabled(result.is_ok());
            // Nothing typed yet is not an error worth showing.
            let message = result
                .err()
                .filter(|e| *e != AddressError::Empty)
                .map(|e| e.to_string());
            error.setStringValue(&ns(message.as_deref().unwrap_or_default()));
            error_row.setHidden(message.is_none());
            for row in &gateway_rows {
                row.setHidden(!util::is_checked(&use_gateway));
            }
            gateway_user.setEnabled(util::is_checked(&gateway_separate));
            for row in &app_rows {
                row.setHidden(!util::is_checked(&use_app));
            }
            if let Some(window) = window.load() {
                if let Some(view) = window.contentView() {
                    window.setContentSize(view.fittingSize());
                }
            }
        })
    };
    for field in [
        &address,
        &name,
        &username,
        &gateway_address,
        &gateway_user,
        &app_program,
        &app_args,
    ] {
        let validate = Rc::clone(&validate);
        util::watch_text(field, move |_| validate(), None);
    }
    for checkbox in [&use_gateway, &gateway_separate, &use_app] {
        let validate = Rc::clone(&validate);
        util::on_action(checkbox, move |_| validate());
    }
    validate();
    sheet.window.setInitialFirstResponder(Some(&address));
    sheet.show();
}

// ----- Connection settings -----

/// A pop-up choosing between the preference ("Default (…)") and explicit values.
struct Choice<T> {
    popup: Retained<NSPopUpButton>,
    values: Vec<T>,
}

impl<T: Clone + PartialEq> Choice<T> {
    fn new(
        mtm: MainThreadMarker,
        default_label: &str,
        options: Vec<(T, String)>,
        current: Option<&T>,
    ) -> Self {
        let mut labels = vec![format!("Default ({default_label})")];
        labels.extend(options.iter().map(|(_, label)| label.clone()));
        let values: Vec<T> = options.into_iter().map(|(value, _)| value).collect();
        let selected = current
            .and_then(|c| values.iter().position(|v| v == c))
            .map_or(0, |i| i + 1);
        Self {
            popup: Form::popup(mtm, &labels, selected),
            values,
        }
    }

    fn value(&self) -> Option<T> {
        Form::selected(&self.popup)
            .checked_sub(1)
            .and_then(|i| self.values.get(i).cloned())
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "On"
    } else {
        "Off"
    }
}

fn toggle(mtm: MainThreadMarker, default: bool, current: Option<bool>) -> Choice<bool> {
    Choice::new(
        mtm,
        on_off(default),
        vec![(true, "On".into()), (false, "Off".into())],
        current.as_ref(),
    )
}

/// The settings of one connection (values replacing the app settings).
pub(super) fn connection_settings(app: &Rc<App>, target: LaunchTarget, name: &str) {
    let mtm = app.mtm;
    let store = OverrideStore::default();
    let current = store.get(&target);
    let config = AppConfig::load();

    let mut sizes: Vec<Resolution> = Resolution::PRESETS.to_vec();
    if let Some(size) = current.resolution.filter(|s| !sizes.contains(s)) {
        sizes.push(size);
    }
    let mut size_labels = vec!["Fit Window".to_string()];
    size_labels.extend(sizes.iter().map(ToString::to_string));
    size_labels.push("Custom…".to_string());
    let custom_index = sizes.len() + 1;
    let resolution = Form::popup(
        mtm,
        &size_labels,
        current
            .resolution
            .and_then(|r| sizes.iter().position(|s| *s == r))
            .map_or(0, |i| i + 1),
    );
    let custom = Form::text_field(mtm, "", "Custom size, e.g. 1920x1080");

    let default_graphics = GraphicsMode::from_id(&config.graphics_mode);
    let graphics = Choice::new(
        mtm,
        GRAPHICS_MODES
            .iter()
            .find(|(m, _)| *m == default_graphics)
            .map_or("Automatic", |(_, l)| l),
        GRAPHICS_MODES
            .iter()
            .map(|(m, l)| (m.id().to_string(), (*l).to_string()))
            .collect(),
        current.graphics_mode.as_ref(),
    );
    let quality = Choice::new(
        mtm,
        Quality::from_id(&config.quality).label(),
        Quality::ALL
            .iter()
            .map(|q| (q.id().to_string(), q.label().to_string()))
            .collect(),
        current.quality.as_ref(),
    );
    let multi_monitor = toggle(mtm, config.multi_monitor, current.multi_monitor);
    let clipboard = toggle(mtm, config.enable_clipboard, current.clipboard);
    let audio = toggle(mtm, config.enable_audio, current.audio);
    let microphone = toggle(mtm, config.enable_microphone, current.microphone);
    let camera = toggle(mtm, config.enable_camera, current.camera);
    let drives = toggle(mtm, config.enable_drive_redirection, current.drives);
    let printers = toggle(mtm, config.enable_printers, current.printers);
    let smartcard = toggle(mtm, config.enable_smartcard, current.smartcard);
    let pre = Form::text_field(mtm, &current.pre_command, "");
    let post = Form::text_field(mtm, &current.post_command, "");
    let error = util::wrapping_label(mtm, "", FIELD_WIDTH + 140.0);
    error.setTextColor(Some(&NSColor::systemRedColor()));

    let mut form = Form::new(mtm);
    form.section("Display");
    form.row("Resolution:", &resolution);
    let custom_row = form.row("Custom size:", &custom);
    custom_row.setHidden(true);
    form.note("A fixed size is scaled to the window.");
    form.row("Graphics:", &graphics.popup);
    form.row("Connection Quality:", &quality.popup);
    form.row("Use All Monitors:", &multi_monitor.popup);
    form.section("Devices");
    form.row("Clipboard:", &clipboard.popup);
    form.row("Audio Output:", &audio.popup);
    form.row("Microphone:", &microphone.popup);
    form.row("Camera:", &camera.popup);
    form.row("Shared Folders:", &drives.popup);
    form.row("Printers:", &printers.popup);
    form.row("Smart Cards:", &smartcard.popup);
    form.note("Default uses the value from Settings.");
    form.section("Commands");
    form.row("Before connecting:", &pre);
    form.row("After disconnecting:", &post);
    form.note(
        "Shell commands run on this computer. The connection is aborted when the command \
         before connecting fails or runs longer than a minute. WINDOWS_APP_NAME, \
         WINDOWS_APP_ADDRESS and WINDOWS_APP_TARGET describe the connection.",
    );
    let error_row = form.full_row(&error);
    error_row.setHidden(true);
    let grid = form.build();

    let sheet = Sheet::new(mtm, &format!("{name} Settings"), &grid, app.sheet_parent());
    let handle = sheet.handle();
    {
        let handle = handle.clone();
        let custom_row = custom_row.clone();
        util::on_action(&resolution, move |sender| {
            let chosen = sender.downcast_ref::<NSPopUpButton>().map(Form::selected);
            custom_row.setHidden(chosen != Some(custom_index));
            if let Some(window) = handle.load() {
                if let Some(view) = window.contentView() {
                    window.setContentSize(view.fittingSize());
                }
            }
        });
    }
    sheet.button("Cancel", false, {
        let handle = handle.clone();
        move || close_sheet(&handle)
    });
    let toggles = [
        clipboard, audio, microphone, camera, drives, printers, smartcard,
    ];
    let show_error = move |message: &str| {
        error.setStringValue(&ns(message));
        error_row.setHidden(false);
    };
    sheet.button("Save", true, move || {
        let selected = Form::selected(&resolution);
        let resolution = if selected == custom_index {
            match Resolution::parse(&custom.stringValue().to_string()) {
                Some(size) => Some(size),
                None => {
                    show_error("Enter the custom size as width x height, e.g. 1920x1080.");
                    return;
                }
            }
        } else {
            selected.checked_sub(1).and_then(|i| sizes.get(i).copied())
        };
        let [clipboard, audio, microphone, camera, drives, printers, smartcard] = &toggles;
        let mut overrides = ConnectionOverrides {
            resolution,
            graphics_mode: graphics.value(),
            quality: quality.value(),
            clipboard: clipboard.value(),
            audio: audio.value(),
            microphone: microphone.value(),
            camera: camera.value(),
            drives: drives.value(),
            printers: printers.value(),
            smartcard: smartcard.value(),
            multi_monitor: multi_monitor.value(),
            pre_command: pre.stringValue().to_string(),
            post_command: post.stringValue().to_string(),
        };
        if let Err(e) = overrides.normalize() {
            show_error(&e.to_string());
            return;
        }
        if let Err(e) = store.set(&target, overrides) {
            show_error(&format!("Could not save the settings: {e}"));
            return;
        }
        close_sheet(&handle);
    });
    sheet.show();
}

// ----- App settings -----

thread_local! {
    static SETTINGS: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
}

/// The Settings window; changes apply immediately (macOS convention).
pub(super) fn show_settings(app: &Rc<App>) {
    if let Some(window) = SETTINGS.with_borrow(Clone::clone) {
        window.makeKeyAndOrderFront(None);
        return;
    }
    let mtm = app.mtm;
    let config = AppConfig::load();
    let save = |edit: &dyn Fn(&mut AppConfig)| {
        let mut config = AppConfig::load();
        edit(&mut config);
        if let Err(e) = config.save() {
            log::warn!("Failed to save configuration: {e}");
        }
    };

    let profile = Form::popup(
        mtm,
        &AuthProfile::ALL
            .iter()
            .map(|p| p.label().to_string())
            .collect::<Vec<_>>(),
        config.auth_profile().index() as usize,
    );
    util::on_action(&profile, move |sender| {
        if let Some(popup) = sender.downcast_ref::<NSPopUpButton>() {
            let index = u32::try_from(Form::selected(popup)).unwrap_or(0);
            save(&|c| c.auth_profile_index = index);
        }
    });
    let graphics = Form::popup(
        mtm,
        &GRAPHICS_MODES
            .iter()
            .map(|(_, l)| (*l).to_string())
            .collect::<Vec<_>>(),
        GRAPHICS_MODES
            .iter()
            .position(|(m, _)| *m == GraphicsMode::from_id(&config.graphics_mode))
            .unwrap_or(0),
    );
    util::on_action(&graphics, move |sender| {
        if let Some(popup) = sender.downcast_ref::<NSPopUpButton>() {
            let mode = GRAPHICS_MODES
                .get(Form::selected(popup))
                .map_or(GraphicsMode::Auto, |(m, _)| *m);
            save(&|c| mode.id().clone_into(&mut c.graphics_mode));
        }
    });
    let quality = Form::popup(
        mtm,
        &Quality::ALL
            .iter()
            .map(|q| q.label().to_string())
            .collect::<Vec<_>>(),
        Quality::ALL
            .iter()
            .position(|q| *q == Quality::from_id(&config.quality))
            .unwrap_or(0),
    );
    util::on_action(&quality, move |sender| {
        if let Some(popup) = sender.downcast_ref::<NSPopUpButton>() {
            let quality = Quality::ALL
                .get(Form::selected(popup))
                .copied()
                .unwrap_or_default();
            save(&|c| quality.id().clone_into(&mut c.quality));
        }
    });

    let switch = |title: &str, value: bool, apply: fn(&mut AppConfig, bool)| {
        let checkbox = util::checkbox(mtm, title, value);
        util::on_action(&checkbox, move |sender| {
            if let Some(button) = sender.downcast_ref::<NSButton>() {
                let on = util::is_checked(button);
                save(&|c| apply(c, on));
            }
        });
        checkbox
    };
    let multi_monitor = switch(
        "Use all monitors in full screen",
        config.multi_monitor,
        |c, v| {
            c.multi_monitor = v;
        },
    );
    let clipboard = switch("Clipboard sharing", config.enable_clipboard, |c, v| {
        c.enable_clipboard = v;
    });
    let audio = switch("Audio output", config.enable_audio, |c, v| {
        c.enable_audio = v
    });
    let microphone = switch("Microphone", config.enable_microphone, |c, v| {
        c.enable_microphone = v;
    });
    let camera = switch("Camera", config.enable_camera, |c, v| c.enable_camera = v);
    let smartcard = switch(
        "Smart cards and security keys",
        config.enable_smartcard,
        |c, v| {
            c.enable_smartcard = v;
        },
    );
    let printers = switch("Printers", config.enable_printers, |c, v| {
        c.enable_printers = v;
    });

    let folder_label = util::label(mtm, &folder_text(&config.shared_folder_path));
    util::secondary(&folder_label);
    let folder_button = {
        let folder_label = folder_label.clone();
        util::button(mtm, "Choose…", move || {
            let panel = NSOpenPanel::openPanel(folder_label.mtm());
            panel.setCanChooseDirectories(true);
            panel.setCanChooseFiles(false);
            panel.setAllowsMultipleSelection(false);
            panel.setPrompt(Some(&ns("Share")));
            if panel.runModal() != NSModalResponseOK {
                return;
            }
            let Some(path) = panel.URLs().firstObject().and_then(|url| url.path()) else {
                return;
            };
            let path = path.to_string();
            folder_label.setStringValue(&ns(&folder_text(&path)));
            save(&|c| path.clone_into(&mut c.shared_folder_path));
        })
    };
    let drives = {
        let folder_button = folder_button.clone();
        let checkbox = util::checkbox(mtm, "Share a folder", config.enable_drive_redirection);
        folder_button.setEnabled(config.enable_drive_redirection);
        util::on_action(&checkbox, move |sender| {
            if let Some(button) = sender.downcast_ref::<NSButton>() {
                let on = util::is_checked(button);
                folder_button.setEnabled(on);
                save(&|c| c.enable_drive_redirection = on);
            }
        });
        checkbox
    };
    let folder_row = NSStackView::new(mtm);
    folder_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    folder_row.addArrangedSubview(&folder_label);
    folder_row.addArrangedSubview(&folder_button);

    let mut form = Form::new(mtm);
    form.row("Sign-in profile:", &profile);
    form.note(
        "The platform identity used during Microsoft sign-in. Takes effect the next time \
         the app starts.",
    );
    form.row("Graphics:", &graphics);
    form.row("Connection quality:", &quality);
    form.row("Display:", &multi_monitor);
    let mut first = true;
    for checkbox in [
        &clipboard,
        &audio,
        &microphone,
        &camera,
        &smartcard,
        &printers,
    ] {
        form.row(if first { "Redirection:" } else { "" }, checkbox);
        first = false;
    }
    form.row("Folder sharing:", &drives);
    form.row("", &folder_row);
    form.note(
        "In the session, the folder appears under This PC in “Redirected drives and \
         folders”.",
    );
    let grid = form.build();

    let content = NSStackView::new(mtm);
    content.setEdgeInsets(NSEdgeInsets {
        top: 20.0,
        left: 20.0,
        bottom: 20.0,
        right: 20.0,
    });
    content.addArrangedSubview(&grid);
    // SAFETY: Plain window creation; the window is not released on close.
    let window = unsafe {
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            util::rect(0.0, 0.0, 520.0, 400.0),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        );
        window.setReleasedWhenClosed(false);
        window
    };
    window.setTitle(&ns("Settings"));
    window.setContentView(Some(&content));
    window.setContentSize(content.fittingSize());
    window.center();
    window.makeKeyAndOrderFront(None);
    SETTINGS.with_borrow_mut(|settings| *settings = Some(window));
}

fn folder_text(path: &str) -> String {
    if path.is_empty() {
        "No folder selected".to_string()
    } else {
        path.to_string()
    }
}
