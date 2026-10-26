//! Content dialogs of the main window: messages and reports, property sheets,
//! confirmations, text and choice prompts, and the About dialog.
//!
//! The main window shows one dialog at a time from its queue. Each dialog is a component
//! that owns its field state and reports one [`Outcome`] through its `done` callback when
//! the user closes it.

use windows_reactor::*;

use super::{forms, win32};
use crate::connections::SavedConnection;
use crate::frontend::common::text::{
    ABOUT_COMMENTS, APP_NAME, COPYRIGHT, DEVELOPER, REPOSITORY_URL, TRADEMARKS,
};
use crate::overrides::ConnectionOverrides;
use crate::session::Credentials;

/// What a dialog asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum DialogSpec {
    /// Text with a close button (errors, reports).
    Message {
        title: String,
        body: String,
    },
    /// Label/value rows (connection details).
    Properties {
        title: String,
        rows: Vec<(String, String)>,
    },
    /// A yes/no question; `confirm` labels the primary button.
    Confirm {
        title: String,
        body: String,
        confirm: String,
    },
    /// A single line of text. With `must_equal`, the primary button is enabled only when
    /// the trimmed text equals it (typed confirmation).
    Text {
        title: String,
        body: String,
        label: String,
        initial: String,
        confirm: String,
        must_equal: Option<String>,
    },
    /// One of `options`.
    Choice {
        title: String,
        body: String,
        label: String,
        options: Vec<String>,
        confirm: String,
    },
    /// Username and passwords for a direct connection.
    Credentials {
        connection: SavedConnection,
        can_remember: bool,
        known: Credentials,
    },
    /// Adds (`None`) or edits a Remote PC.
    RemotePc {
        existing: Option<SavedConnection>,
    },
    /// The settings of one connection.
    ConnectionSettings {
        name: String,
        current: ConnectionOverrides,
    },
    About,
}

/// How a dialog was closed.
#[derive(Debug)]
pub enum Outcome {
    Cancelled,
    Confirmed,
    Text(String),
    Index(usize),
    Credentials {
        connection: SavedConnection,
        credentials: Credentials,
    },
    Connection(SavedConnection),
    Overrides(ConnectionOverrides),
}

/// The component showing `spec`; `done` receives the outcome once.
pub fn view(spec: &DialogSpec, done: Callback<Outcome>) -> View {
    match spec {
        DialogSpec::Credentials {
            connection,
            can_remember,
            known,
        } => View::component::<forms::CredentialsDialog>(forms::CredentialsInput {
            connection: connection.clone(),
            can_remember: *can_remember,
            known: known.clone(),
            done,
        }),
        DialogSpec::RemotePc { existing } => {
            View::component::<forms::RemotePcDialog>(forms::RemotePcInput {
                existing: existing.clone(),
                done,
            })
        }
        DialogSpec::ConnectionSettings { name, current } => {
            View::component::<forms::SettingsDialog>(forms::SettingsInput {
                name: name.clone(),
                current: current.clone(),
                done,
            })
        }
        _ => View::component::<SimpleDialog>(SimpleInput {
            spec: spec.clone(),
            done,
        }),
    }
}

/// A content dialog that closes with `on_closed`; `primary` adds the default button.
pub fn shell(
    title: &str,
    primary: Option<&str>,
    primary_enabled: bool,
    close: &str,
    open: bool,
    on_closed: Callback<ContentDialogResult>,
    content: impl Into<View>,
) -> View {
    ContentDialog::new()
        .is_open(open)
        .title(title)
        .primary_button_text_optional(primary)
        .is_primary_button_enabled(primary_enabled)
        .close_button_text(close)
        .on_closed(on_closed)
        .content(content)
}

/// Wrapped, selectable body text.
pub fn body_text(text: &str) -> TextBlock {
    TextBlock::new()
        .text(text)
        .text_wrapping(TextWrapping::Wrap)
        .is_text_selection_enabled(true)
}

/// Secondary text below a field (hints, validation errors).
pub fn hint_text(text: &str, error: bool) -> TextBlock {
    let block = TextBlock::new()
        .text(text)
        .text_wrapping(TextWrapping::Wrap)
        .font_size(12.0);
    if error {
        block.foreground(ThemeBrush::SystemCritical)
    } else {
        block
    }
}

// ----- Simple dialogs -----

#[derive(Clone, PartialEq)]
pub struct SimpleInput {
    spec: DialogSpec,
    done: Callback<Outcome>,
}

#[derive(Clone)]
pub enum SimpleMsg {
    Text(String),
    Selected(Option<usize>),
    Closed(ContentDialogResult),
}

pub struct SimpleDialog {
    spec: DialogSpec,
    done: Callback<Outcome>,
    text: String,
    selected: usize,
    closed: bool,
}

impl SimpleDialog {
    fn can_confirm(&self) -> bool {
        match &self.spec {
            DialogSpec::Text {
                must_equal: Some(expected),
                ..
            } => self.text.trim() == expected.as_str(),
            DialogSpec::Text { .. } => !self.text.trim().is_empty(),
            DialogSpec::Choice { options, .. } => self.selected < options.len(),
            _ => true,
        }
    }

    fn outcome(&self, result: ContentDialogResult) -> Outcome {
        if result != ContentDialogResult::Primary || !self.can_confirm() {
            return Outcome::Cancelled;
        }
        match &self.spec {
            DialogSpec::Text { .. } => Outcome::Text(self.text.trim().to_string()),
            DialogSpec::Choice { .. } => Outcome::Index(self.selected),
            _ => Outcome::Confirmed,
        }
    }

    fn about() -> View {
        StackPanel::new().spacing(12.0).children((
            StackPanel::new()
                .orientation(Orientation::Horizontal)
                .spacing(16.0)
                .children((
                    Viewbox::new()
                        .width(48.0)
                        .height(48.0)
                        .slot(ViewboxSlot::Child, FontIcon::new().glyph("\u{E770}")),
                    StackPanel::new().spacing(2.0).children((
                        TextBlock::new()
                            .text(APP_NAME)
                            .font_size(20.0)
                            .font_weight(FontWeight::SEMI_BOLD),
                        TextBlock::new().text(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                        TextBlock::new().text(format!("{COPYRIGHT} · {DEVELOPER}")),
                    )),
                )),
            body_text(ABOUT_COMMENTS),
            StackPanel::new()
                .orientation(Orientation::Horizontal)
                .spacing(4.0)
                .children((
                    HyperlinkButton::new()
                        .on_click(|| win32::open_uri(REPOSITORY_URL))
                        .content("Website"),
                    HyperlinkButton::new()
                        .on_click(|| win32::open_uri(&format!("{REPOSITORY_URL}/issues")))
                        .content("Report an Issue"),
                )),
            body_text(&format!(
                "{APP_NAME} {}\nRDP engine: FreeRDP {}\nUI: WinUI 3 (Windows App SDK)",
                env!("CARGO_PKG_VERSION"),
                crate::rdp::version()
            )),
            hint_text(TRADEMARKS, false),
        ))
    }
}

impl Component for SimpleDialog {
    type Input = SimpleInput;
    type Message = SimpleMsg;

    fn create(input: &Self::Input, _ctx: &ComponentContext<Self>) -> Self {
        let text = match &input.spec {
            DialogSpec::Text { initial, .. } => initial.clone(),
            _ => String::new(),
        };
        Self {
            spec: input.spec.clone(),
            done: input.done.clone(),
            text,
            selected: 0,
            closed: false,
        }
    }

    fn update(&mut self, msg: Self::Message, _ctx: &ComponentContext<Self>) {
        match msg {
            SimpleMsg::Text(text) => self.text = text,
            SimpleMsg::Selected(index) => self.selected = index.unwrap_or(usize::MAX),
            SimpleMsg::Closed(result) => {
                if !self.closed {
                    self.closed = true;
                    let _ = self.done.call(self.outcome(result));
                }
            }
        }
    }

    fn view(&self, _input: &Self::Input, ctx: &mut ViewContext<Self>) -> View {
        let on_closed = ctx.callback(SimpleMsg::Closed);
        let open = !self.closed;
        match &self.spec {
            DialogSpec::Message { title, body } => shell(
                title,
                None,
                true,
                "Close",
                open,
                on_closed,
                ScrollViewer::new().content(body_text(body)),
            ),
            DialogSpec::Properties { title, rows } => {
                let items = rows.iter().enumerate().map(|(index, (label, value))| {
                    (
                        index,
                        StackPanel::new().spacing(2.0).children((
                            TextBlock::new()
                                .text(label.as_str())
                                .font_size(12.0)
                                .font_weight(FontWeight::SEMI_BOLD),
                            body_text(value),
                        )),
                    )
                });
                shell(
                    title,
                    None,
                    true,
                    "Close",
                    open,
                    on_closed,
                    ScrollViewer::new()
                        .content(StackPanel::new().spacing(12.0).keyed_children(items)),
                )
            }
            DialogSpec::Confirm {
                title,
                body,
                confirm,
            } => shell(
                title,
                Some(confirm),
                true,
                "Cancel",
                open,
                on_closed,
                body_text(body),
            ),
            DialogSpec::Text {
                title,
                body,
                label,
                confirm,
                ..
            } => shell(
                title,
                Some(confirm),
                self.can_confirm(),
                "Cancel",
                open,
                on_closed,
                StackPanel::new().spacing(12.0).children((
                    body_text(body),
                    TextBox::new()
                        .text(self.text.as_str())
                        .on_text_changed(ctx.callback(SimpleMsg::Text))
                        .slot(TextBoxSlot::Header, label.as_str()),
                )),
            ),
            DialogSpec::Choice {
                title,
                body,
                label,
                options,
                confirm,
            } => shell(
                title,
                Some(confirm),
                self.can_confirm(),
                "Cancel",
                open,
                on_closed,
                StackPanel::new().spacing(12.0).children((
                    body_text(body),
                    ComboBox::new()
                        .items_source(options.iter().cloned())
                        .selected_index(self.selected.min(options.len().saturating_sub(1)))
                        .horizontal_alignment(HorizontalAlignment::Stretch)
                        .on_selection_changed(ctx.callback(SimpleMsg::Selected))
                        .slot(ComboBoxSlot::Header, label.as_str()),
                )),
            ),
            DialogSpec::About => shell(
                &format!("About {APP_NAME}"),
                None,
                true,
                "Close",
                open,
                on_closed,
                Self::about(),
            ),
            DialogSpec::Credentials { .. }
            | DialogSpec::RemotePc { .. }
            | DialogSpec::ConnectionSettings { .. } => View::empty(),
        }
    }
}
