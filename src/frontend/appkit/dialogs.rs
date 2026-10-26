//! Standalone dialogs: the About panel, read-only property sheets and the quit
//! confirmation.

use super::app::App;
use super::forms::{close_sheet, Form, Sheet};
use super::util::{self, ns};
use crate::frontend::common::text;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAboutPanelOptionApplicationName, NSAboutPanelOptionApplicationVersion,
    NSAboutPanelOptionCredits, NSApplication, NSFont, NSGridCellPlacement,
};
use objc2_foundation::{NSAttributedString, NSDictionary, NSString};
use std::rc::Rc;

/// The standard About panel with the app's credits.
pub(super) fn about(mtm: MainThreadMarker) {
    let credits = format!(
        "{}\n\n{}\n\nFreeRDP {}\n{}",
        text::ABOUT_COMMENTS,
        text::TRADEMARKS,
        crate::rdp::version(),
        text::REPOSITORY_URL,
    );
    let credits = NSAttributedString::from_nsstring(&ns(&credits));
    let name = ns(text::APP_NAME);
    let version = ns(env!("CARGO_PKG_VERSION"));
    let copyright = ns(text::COPYRIGHT);
    let copyright_key = ns("Copyright");
    // SAFETY: Reading immutable extern string constants.
    let (name_key, version_key, credits_key) = unsafe {
        (
            NSAboutPanelOptionApplicationName,
            NSAboutPanelOptionApplicationVersion,
            NSAboutPanelOptionCredits,
        )
    };
    let keys: [&NSString; 4] = [name_key, version_key, credits_key, &copyright_key];
    let values: [&AnyObject; 4] = [&name, &version, &credits, &copyright];
    let options = NSDictionary::from_slices(&keys, &values);
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: The dictionary holds the documented key and value types.
    unsafe { app.orderFrontStandardAboutPanelWithOptions(&options) };
    app.activate();
}

/// A sheet listing `rows` (title, value), with a button copying them as text.
pub(super) fn properties(app: &Rc<App>, title: &str, rows: &[(&str, String)]) {
    let mtm = app.mtm;
    let mut form = Form::new(mtm);
    for (key, value) in rows {
        let label = util::wrapping_label(mtm, value, 320.0);
        label.setSelectable(true);
        let row = form.row(&format!("{key}:"), &label);
        row.cellAtIndex(0)
            .setXPlacement(NSGridCellPlacement::Trailing);
    }
    let grid = form.build();
    let sheet = Sheet::new(mtm, title, &grid, app.sheet_parent());
    let report: String = rows
        .iter()
        .map(|(key, value)| format!("{key}: {value}\n"))
        .collect();
    let copy = sheet.button("Copy", false, move || util::set_pasteboard_text(&report));
    copy.setToolTip(Some(&ns("Copy all values as text")));
    let handle = sheet.handle();
    sheet.button("Done", true, move || close_sheet(&handle));
    sheet.show();
}

/// What to do about open sessions when quitting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QuitChoice {
    Quit,
    KeepOpen,
    Cancel,
}

/// Asks whether quitting should disconnect the open sessions `names` (modal).
pub(super) fn confirm_quit(mtm: MainThreadMarker, names: &[String]) -> QuitChoice {
    let (heading, body) = text::quit_prompt(names);
    let alert = util::alert(
        mtm,
        &heading,
        &body,
        &["Disconnect and Quit", "Keep Open", "Cancel"],
    );
    util::destructive(&alert, 0);
    match util::button_index(alert.runModal()) {
        0 => QuitChoice::Quit,
        1 => QuitChoice::KeepOpen,
        _ => QuitChoice::Cancel,
    }
}

/// A large, bold label (headings of empty states and panels).
pub(super) fn heading(mtm: MainThreadMarker, text: &str) -> Retained<objc2_app_kit::NSTextField> {
    let label = util::label(mtm, text);
    label.setFont(Some(&NSFont::boldSystemFontOfSize(17.0)));
    label
}
