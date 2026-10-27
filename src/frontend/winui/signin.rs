//! Sign-in windows: the account sign-in (embedded browser, device code or pasted token),
//! the session host sign-in requested by the RDP engine, and the custom display scale
//! prompt of a session window. Each is a separate Reactor window.
//!
//! The browser flows drive a WebView2: the Reactor control creates the `CoreWebView2`, and
//! this module hooks `NavigationStarting` to hand every URL to the shared sign-in flow,
//! which decides whether to let it load, replace it (device challenges with extra headers)
//! or stop because the flow finished.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2AddScriptToExecuteOnDocumentCreatedCompletedHandler,
    ICoreWebView2Environment2, ICoreWebView2NavigationStartingEventArgs, ICoreWebView2Settings2,
    ICoreWebView2_2,
};
use webview2_com::{take_pwstr, NavigationStartingEventHandler};
use windows::core::{w, IUnknown, IUnknown_Vtbl, Interface, GUID, HRESULT, HSTRING, PWSTR};
use windows::Win32::System::Com::IStream;
use windows_reactor::*;

use super::session_window;
use super::win32;
use crate::frontend::common::signin::{
    BrowserSignIn, DeviceCode, DeviceCodeSignIn, Navigation, SessionHostSignIn,
};
use crate::frontend::common::text::APP_NAME;
use crate::session::{MAX_SCALE, MIN_SCALE};

// ----- WebView2 plumbing -----

/// `ICoreWebView2Interop2`: hands out the COM `ICoreWebView2` behind the WinRT
/// `CoreWebView2` that the WinUI control exposes.
#[repr(transparent)]
#[derive(Clone)]
struct ICoreWebView2Interop2(IUnknown);

#[repr(C)]
#[allow(non_snake_case)]
struct ICoreWebView2Interop2Vtbl {
    base__: IUnknown_Vtbl,
    GetComICoreWebView2: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
}

// SAFETY: the vtable layout matches the interface declared in WebView2Interop.h (IUnknown
// followed by one method), and the IID is that interface's.
unsafe impl Interface for ICoreWebView2Interop2 {
    type Vtable = ICoreWebView2Interop2Vtbl;
    const IID: GUID = GUID::from_u128(0xb151ad7c_cfb0_4ecf_b9b2_afca868581a6);
}

impl ICoreWebView2Interop2 {
    fn com_webview(&self) -> windows::core::Result<ICoreWebView2> {
        let mut raw: *mut c_void = std::ptr::null_mut();
        // SAFETY: calls the interface's only method with a valid out pointer; on success it
        // holds an owned ICoreWebView2 reference, which `from_raw` takes over.
        unsafe {
            (self.vtable().GetComICoreWebView2)(self.as_raw(), &mut raw).ok()?;
            if raw.is_null() {
                return Err(windows::core::Error::empty());
            }
            Ok(ICoreWebView2::from_raw(raw))
        }
    }
}

/// The COM WebView2 behind the object returned by Reactor's WebView2 request.
fn com_webview(core: IUnknown) -> Result<ICoreWebView2, String> {
    if let Ok(webview) = core.cast::<ICoreWebView2>() {
        return Ok(webview);
    }
    core.cast::<ICoreWebView2Interop2>()
        .and_then(|interop| interop.com_webview())
        .map_err(|e| format!("The embedded browser is not available: {e}"))
}

/// A sign-in flow that inspects every navigation of the web view.
trait NavigationFlow {
    fn on_navigation(&mut self, uri: &str) -> Navigation;
}

impl NavigationFlow for BrowserSignIn {
    fn on_navigation(&mut self, uri: &str) -> Navigation {
        BrowserSignIn::on_navigation(self, uri)
    }
}

impl NavigationFlow for SessionHostSignIn {
    fn on_navigation(&mut self, uri: &str) -> Navigation {
        SessionHostSignIn::on_navigation(self, uri)
    }
}

/// How a flow ended, as seen by the web view.
enum FlowEnd {
    Done(String),
    Failed(String),
}

/// Configures `webview` for a flow and loads its first page.
fn start_flow(
    webview: &ICoreWebView2,
    url: &str,
    user_agent: &str,
    script: Option<&str>,
    flow: Rc<RefCell<dyn NavigationFlow>>,
    on_end: Rc<dyn Fn(FlowEnd)>,
) -> Result<(), String> {
    let watched = webview.clone();
    let handler = NavigationStartingEventHandler::create(Box::new(
        move |_sender: Option<ICoreWebView2>,
              args: Option<ICoreWebView2NavigationStartingEventArgs>| {
            let Some(args) = args else {
                return Ok(());
            };
            let mut raw = PWSTR::null();
            // SAFETY: `Uri` fills `raw` with a CoTaskMem string, which `take_pwstr` frees.
            let uri = unsafe {
                args.Uri(&mut raw)?;
                take_pwstr(raw)
            };
            let navigation = flow.borrow_mut().on_navigation(&uri);
            if navigation == Navigation::Continue {
                return Ok(());
            }
            // SAFETY: plain COM call on the event arguments.
            unsafe { args.SetCancel(true)? };
            match navigation {
                Navigation::Continue => {}
                Navigation::Load { url, headers } => {
                    // Navigating from inside NavigationStarting is not allowed.
                    let webview = watched.clone();
                    win32::defer(move || load_with_headers(&webview, &url, &headers));
                }
                Navigation::Done(result) => on_end(FlowEnd::Done(result)),
                Navigation::Failed(message) => on_end(FlowEnd::Failed(message)),
            }
            Ok(())
        },
    ));
    let mut token = 0i64;
    // SAFETY: COM calls on the UI thread that owns the web view; the strings outlive them.
    unsafe {
        webview
            .add_NavigationStarting(&handler, &mut token)
            .map_err(|e| format!("Could not watch the sign-in page: {e}"))?;
        match webview
            .Settings()
            .and_then(|settings| settings.cast::<ICoreWebView2Settings2>())
        {
            Ok(settings) => {
                if let Err(e) = settings.SetUserAgent(&HSTRING::from(user_agent)) {
                    log::warn!("Could not set the sign-in user agent: {e}");
                }
            }
            Err(e) => log::warn!("The web view settings are not available: {e}"),
        }
        if let Some(script) = script {
            if let Err(e) = webview.AddScriptToExecuteOnDocumentCreated(
                &HSTRING::from(script),
                None::<&ICoreWebView2AddScriptToExecuteOnDocumentCreatedCompletedHandler>,
            ) {
                log::warn!("Could not add the sign-in script: {e}");
            }
        }
    }
    navigate(webview, url)
}

fn navigate(webview: &ICoreWebView2, url: &str) -> Result<(), String> {
    // SAFETY: COM call on the UI thread; the string outlives it.
    unsafe { webview.Navigate(&HSTRING::from(url)) }
        .map_err(|e| format!("Could not open the sign-in page: {e}"))
}

/// Loads `url` with extra request headers (device challenge answers).
fn load_with_headers(webview: &ICoreWebView2, url: &str, headers: &[(&'static str, String)]) {
    let header_text: String = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    // SAFETY: COM calls on the UI thread; the strings outlive them.
    let result = unsafe {
        webview.cast::<ICoreWebView2_2>().and_then(|webview2| {
            let environment = webview2
                .Environment()?
                .cast::<ICoreWebView2Environment2>()?;
            let request = environment.CreateWebResourceRequest(
                &HSTRING::from(url),
                w!("GET"),
                None::<&IStream>,
                &HSTRING::from(header_text.as_str()),
            )?;
            webview2.NavigateWithWebResourceRequest(&request)
        })
    };
    if let Err(e) = result {
        log::warn!("Could not send the device challenge answer ({e}); loading without it");
        if let Err(e) = navigate(webview, url) {
            log::error!("{e}");
        }
    }
}

/// Requests the CoreWebView2 of `element` and starts `flow` on it.
#[allow(clippy::too_many_arguments)]
fn attach_flow(
    element: &ElementRef<WebView2>,
    url: String,
    user_agent: String,
    script: Option<&'static str>,
    flow: Rc<RefCell<dyn NavigationFlow>>,
    on_end: Rc<dyn Fn(FlowEnd)>,
    on_ready: Rc<dyn Fn(Result<ICoreWebView2, String>)>,
) -> bool {
    element.request_core_web_view2(move |result| {
        let webview = match result {
            // SAFETY: Reactor hands out a windows-core COM interface pointer.
            Ok(core) => unsafe { win32::rebind_unknown(core) }
                .ok_or_else(|| "The embedded browser returned no object.".to_string())
                .and_then(com_webview),
            Err(e) => Err(format!("The embedded browser could not start ({e:?}). Make sure the Microsoft Edge WebView2 Runtime is installed.")),
        };
        let webview = webview.and_then(|webview| {
            start_flow(
                &webview,
                &url,
                &user_agent,
                script,
                Rc::clone(&flow),
                Rc::clone(&on_end),
            )
            .map(|()| webview)
        });
        on_ready(webview);
    })
}

fn overlay(content: impl Into<View>) -> View {
    Border::new()
        .background(ThemeBrush::SolidBackground)
        .padding(24.0)
        .content(content)
}

fn busy_overlay(text: &str) -> View {
    overlay(
        StackPanel::new()
            .spacing(16.0)
            .horizontal_alignment(HorizontalAlignment::Center)
            .vertical_alignment(VerticalAlignment::Center)
            .children((
                ProgressRing::new().is_active(true).width(48.0).height(48.0),
                TextBlock::new()
                    .text(text)
                    .text_wrapping(TextWrapping::Wrap)
                    .horizontal_alignment(HorizontalAlignment::Center),
            )),
    )
}

// ----- Account sign-in -----

#[derive(Clone, PartialEq)]
pub struct AccountSignInInput {
    /// Called once with whether the account is now signed in.
    pub done: Callback<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Browser,
    DeviceCode,
    Token,
}

impl Mode {
    const ALL: [(Self, &'static str); 3] = [
        (Self::Browser, "Browser"),
        (Self::DeviceCode, "Device Code"),
        (Self::Token, "Access Token"),
    ];

    fn label(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(mode, _)| *mode == self)
            .map_or("", |(_, label)| label)
    }
}

#[derive(Clone)]
pub enum AccountMsg {
    ModeChanged(Option<String>),
    BrowserReady(Result<(), String>),
    BrowserCode(String),
    BrowserFailed(String),
    Exchanged(Result<(), String>),
    Retry,
    CodeReady(Result<DeviceCode, String>),
    CodeApproved(Result<(), String>),
    CopyCode,
    OpenPage,
    NewCode,
    TokenChanged(String),
    SubmitToken,
    Cancel,
}

pub struct AccountSignIn {
    done: Callback<bool>,
    mode: Mode,
    webview_ref: ElementRef<WebView2>,
    webview: Rc<RefCell<Option<ICoreWebView2>>>,
    flow: Rc<RefCell<BrowserSignIn>>,
    /// Bumped to attach the flow again when the web view was not ready.
    attempt: u32,
    device: DeviceCodeSignIn,
    code: Option<DeviceCode>,
    requesting_code: bool,
    device_task: Option<ComponentTask>,
    token: String,
    busy: Option<String>,
    error: Option<String>,
    finished: bool,
}

impl AccountSignIn {
    fn finish(&mut self, signed_in: bool, ctx: &ComponentContext<Self>) {
        if self.finished {
            return;
        }
        self.finished = true;
        if let Some(task) = self.device_task.take() {
            task.cancel();
        }
        let _ = self.done.call(signed_in);
        if !ctx.window().request_close() {
            log::warn!("Could not close the sign-in window");
        }
    }

    fn request_code(&mut self, ctx: &ComponentContext<Self>) {
        if let Some(task) = self.device_task.take() {
            task.cancel();
        }
        self.code = None;
        self.requesting_code = true;
        self.error = None;
        let request = self.device.request();
        self.device_task =
            Some(ctx.spawn_background(move |_| AccountMsg::CodeReady(super::block_on(request))));
    }

    fn browser_view(&self) -> View {
        WebView2::new().element_ref(&self.webview_ref).into()
    }

    fn device_view(&self, ctx: &mut ViewContext<Self>) -> View {
        let body: View = match &self.code {
            None if self.requesting_code => busy_overlay("Getting a sign-in code…"),
            None => StackPanel::new().spacing(12.0).children((
                TextBlock::new()
                    .text("Sign in on another device or browser with a one-time code.")
                    .text_wrapping(TextWrapping::Wrap),
                Button::new()
                    .style(ButtonStyle::Accent)
                    .on_click(ctx.message(AccountMsg::NewCode))
                    .content("Get a Code"),
            )),
            Some(code) => StackPanel::new().spacing(12.0).children((
                TextBlock::new()
                    .text(format!(
                        "Open {} on any device and enter this code:",
                        code.verification_uri
                    ))
                    .text_wrapping(TextWrapping::Wrap),
                TextBlock::new()
                    .text(code.user_code.clone())
                    .font_size(32.0)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .is_text_selection_enabled(true),
                StackPanel::new()
                    .orientation(Orientation::Horizontal)
                    .spacing(8.0)
                    .children((
                        Button::new()
                            .on_click(ctx.message(AccountMsg::CopyCode))
                            .content("Copy Code"),
                        Button::new()
                            .style(ButtonStyle::Accent)
                            .on_click(ctx.message(AccountMsg::OpenPage))
                            .content("Open Sign-in Page"),
                        Button::new()
                            .on_click(ctx.message(AccountMsg::NewCode))
                            .content("Get New Code"),
                    )),
                StackPanel::new()
                    .orientation(Orientation::Horizontal)
                    .spacing(8.0)
                    .children((
                        ProgressRing::new().is_active(true).width(16.0).height(16.0),
                        TextBlock::new().text("Waiting for you to approve the sign-in…"),
                    )),
            )),
        };
        overlay(ScrollViewer::new().content(body))
    }

    fn token_view(&self, ctx: &mut ViewContext<Self>) -> View {
        overlay(
            Grid::new()
                .rows([GridLength::Auto, GridLength::Star(1.0), GridLength::Auto])
                .row_spacing(12.0)
                .children((
                    TextBlock::new()
                        .text(
                            "Paste an access token for Microsoft Graph, for example one copied \
                             from another signed-in tool.",
                        )
                        .text_wrapping(TextWrapping::Wrap)
                        .grid_row(0),
                    TextBox::new()
                        .text(self.token.clone())
                        .placeholder_text("Paste an Access Token")
                        .accepts_return(true)
                        .text_wrapping(TextWrapping::Wrap)
                        .on_text_changed(ctx.callback(AccountMsg::TokenChanged))
                        .grid_row(1),
                    Button::new()
                        .style(ButtonStyle::Accent)
                        .is_enabled(!self.token.trim().is_empty())
                        .on_click(ctx.message(AccountMsg::SubmitToken))
                        .grid_row(2)
                        .content("Sign In with Token"),
                )),
        )
    }
}

impl Drop for AccountSignIn {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.done.call(false);
        }
    }
}

impl Component for AccountSignIn {
    type Input = AccountSignInInput;
    type Message = AccountMsg;

    fn create(input: &Self::Input, _ctx: &ComponentContext<Self>) -> Self {
        Self {
            done: input.done.clone(),
            mode: Mode::Browser,
            webview_ref: ElementRef::new(),
            webview: Rc::new(RefCell::new(None)),
            flow: Rc::new(RefCell::new(BrowserSignIn::start())),
            attempt: 0,
            device: DeviceCodeSignIn::new(),
            code: None,
            requesting_code: false,
            device_task: None,
            token: String::new(),
            busy: Some("Opening the sign-in page…".to_string()),
            error: None,
            finished: false,
        }
    }

    fn input_changed(&mut self, input: &Self::Input, _ctx: &ComponentContext<Self>) {
        self.done = input.done.clone();
    }

    fn update(&mut self, msg: Self::Message, ctx: &ComponentContext<Self>) {
        match msg {
            AccountMsg::ModeChanged(label) => {
                let mode = Mode::ALL
                    .iter()
                    .find(|(_, l)| Some(*l) == label.as_deref())
                    .map(|(mode, _)| *mode);
                if let Some(mode) = mode {
                    self.mode = mode;
                    self.error = None;
                    if mode == Mode::DeviceCode && self.code.is_none() && !self.requesting_code {
                        self.request_code(ctx);
                    }
                }
            }
            AccountMsg::BrowserReady(result) => {
                self.busy = None;
                if let Err(e) = result {
                    self.error = Some(e);
                }
            }
            AccountMsg::BrowserCode(code) => {
                self.busy = Some("Finishing sign-in…".to_string());
                self.error = None;
                let exchange = self.flow.borrow().exchange(code);
                let _task =
                    ctx.spawn_background(move |_| AccountMsg::Exchanged(super::block_on(exchange)));
            }
            AccountMsg::BrowserFailed(message) => {
                self.busy = None;
                self.error = Some(message);
            }
            AccountMsg::Exchanged(result) => {
                self.busy = None;
                match result {
                    Ok(()) => self.finish(true, ctx),
                    Err(e) => self.error = Some(e),
                }
            }
            AccountMsg::Retry => {
                self.error = None;
                let flow = BrowserSignIn::start();
                let url = flow.url().to_string();
                *self.flow.borrow_mut() = flow;
                let webview = self.webview.borrow().clone();
                match webview {
                    Some(webview) => {
                        if let Err(e) = navigate(&webview, &url) {
                            self.error = Some(e);
                        }
                    }
                    None => self.attempt += 1,
                }
            }
            AccountMsg::CodeReady(result) => {
                self.requesting_code = false;
                match result {
                    Ok(code) => {
                        self.code = Some(code.clone());
                        // A fresh handle for the same profile: the poll runs off the UI thread.
                        self.device_task = Some(ctx.spawn_background(move |token| {
                            let wait =
                                DeviceCodeSignIn::new().wait(&code, move || !token.is_cancelled());
                            AccountMsg::CodeApproved(super::block_on(wait))
                        }));
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            AccountMsg::CodeApproved(result) => match result {
                Ok(()) => self.finish(true, ctx),
                Err(e) => {
                    self.code = None;
                    self.error = Some(e);
                }
            },
            AccountMsg::CopyCode => {
                if let (Some(code), Some(owner)) = (&self.code, win32::main_window()) {
                    if let Err(e) = win32::set_clipboard_text(owner, &code.user_code) {
                        self.error = Some(e);
                    }
                }
            }
            AccountMsg::OpenPage => {
                if let Some(code) = &self.code {
                    win32::open_uri(&code.verification_uri);
                }
            }
            AccountMsg::NewCode => self.request_code(ctx),
            AccountMsg::TokenChanged(text) => self.token = text,
            AccountMsg::SubmitToken => match self.device.import_token(self.token.trim()) {
                Ok(()) => self.finish(true, ctx),
                Err(e) => self.error = Some(e),
            },
            AccountMsg::Cancel => self.finish(false, ctx),
        }
    }

    fn view(&self, _input: &Self::Input, ctx: &mut ViewContext<Self>) -> View {
        ctx.window_title(format!("Sign in - {APP_NAME}"));
        ctx.window_visuals(
            WindowVisuals::new()
                .backdrop(WindowBackdrop::None)
                .client_size(520.0, 720.0)
                .constraints(WindowConstraints {
                    min_width: Some(400.0),
                    min_height: Some(480.0),
                    max_width: None,
                    max_height: None,
                }),
        );

        {
            let element = self.webview_ref.clone();
            let flow: Rc<RefCell<dyn NavigationFlow>> = self.flow.clone();
            let (url, user_agent, script) = {
                let flow = self.flow.borrow();
                (
                    flow.url().to_string(),
                    flow.user_agent().to_string(),
                    flow.injected_script(),
                )
            };
            let sender = ctx.sender();
            let on_end: Rc<dyn Fn(FlowEnd)> = Rc::new(move |end| {
                sender.send(match end {
                    FlowEnd::Done(code) => AccountMsg::BrowserCode(code),
                    FlowEnd::Failed(message) => AccountMsg::BrowserFailed(message),
                });
            });
            let sender = ctx.sender();
            let slot = Rc::clone(&self.webview);
            let on_ready: Rc<dyn Fn(Result<ICoreWebView2, String>)> = Rc::new(move |result| {
                let result = result.map(|webview| {
                    *slot.borrow_mut() = Some(webview);
                });
                sender.send(AccountMsg::BrowserReady(result));
            });
            ctx.use_effect("webview", self.attempt, move || {
                if !attach_flow(&element, url, user_agent, script, flow, on_end, on_ready) {
                    log::warn!("The sign-in web view was not ready");
                }
                None
            });
        }

        let selector = SelectorBar::new()
            .on_selected_text_changed(ctx.callback(AccountMsg::ModeChanged))
            .collection_slot(
                SelectorBarSlot::Items,
                Mode::ALL.iter().map(|(mode, label)| {
                    (
                        *label,
                        View::from(
                            SelectorBarItem::new()
                                .text(*label)
                                .is_selected(*mode == self.mode),
                        ),
                    )
                }),
            );

        let mut layers: Vec<(&'static str, View)> = vec![("browser", self.browser_view())];
        match self.mode {
            Mode::Browser => {
                if let Some(text) = &self.busy {
                    layers.push(("busy", busy_overlay(text)));
                }
            }
            Mode::DeviceCode => layers.push(("device", self.device_view(ctx))),
            Mode::Token => layers.push(("token", self.token_view(ctx))),
        }
        let content = Grid::new().grid_row(1).keyed_children(layers);

        let error = InfoBar::new()
            .title("Sign-in did not finish")
            .message(self.error.clone().unwrap_or_default())
            .severity(InfoBarSeverity::Error)
            .is_open(self.error.is_some())
            .is_closable(false);
        let footer = Grid::new()
            .columns([GridLength::Star(1.0), GridLength::Auto, GridLength::Auto])
            .column_spacing(8.0)
            .grid_row(2)
            .margin(Thickness::uniform(12.0))
            .children((
                TextBlock::new()
                    .text(format!(
                        "Signing in with {}",
                        self.mode.label().to_lowercase()
                    ))
                    .vertical_alignment(VerticalAlignment::Center)
                    .grid_column(0),
                Button::new()
                    .is_enabled(self.mode == Mode::Browser && self.busy.is_none())
                    .on_click(ctx.message(AccountMsg::Retry))
                    .grid_column(1)
                    .content("Start Over"),
                Button::new()
                    .on_click(ctx.message(AccountMsg::Cancel))
                    .grid_column(2)
                    .content("Cancel"),
            ));

        Grid::new()
            .rows([
                GridLength::Auto,
                GridLength::Star(1.0),
                GridLength::Auto,
                GridLength::Auto,
            ])
            .children((
                selector,
                content,
                footer,
                Border::new().grid_row(3).content(error),
            ))
    }
}

// ----- Session host sign-in -----

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSignInInput {
    /// Session window waiting for the result.
    pub id: u64,
    pub name: String,
    pub url: String,
}

#[derive(Clone)]
pub enum HostMsg {
    Ready(Result<(), String>),
    Reveal,
    Done(String),
    Failed(String),
    Cancel,
}

pub struct HostSignIn {
    id: u64,
    name: String,
    webview_ref: ElementRef<WebView2>,
    flow: Rc<RefCell<SessionHostSignIn>>,
    revealed: bool,
    error: Option<String>,
    answered: bool,
    timer: Option<ComponentTask>,
}

impl HostSignIn {
    fn answer(&mut self, redirect_url: Option<String>) {
        if self.answered {
            return;
        }
        self.answered = true;
        if let Some(timer) = self.timer.take() {
            timer.cancel();
        }
        session_window::provide_auth(self.id, redirect_url);
    }

    fn close(ctx: &ComponentContext<Self>) {
        if !ctx.window().request_close() {
            log::warn!("Could not close the session sign-in window");
        }
    }
}

impl Drop for HostSignIn {
    fn drop(&mut self) {
        if !self.answered {
            self.flow.borrow_mut().cancel();
            self.answer(None);
        }
    }
}

impl Component for HostSignIn {
    type Input = HostSignInInput;
    type Message = HostMsg;

    fn create(input: &Self::Input, ctx: &ComponentContext<Self>) -> Self {
        let timer = ctx.spawn_background(|token| {
            std::thread::sleep(SessionHostSignIn::SILENT_TIMEOUT);
            if token.is_cancelled() {
                HostMsg::Cancel
            } else {
                HostMsg::Reveal
            }
        });
        Self {
            id: input.id,
            name: input.name.clone(),
            webview_ref: ElementRef::new(),
            flow: Rc::new(RefCell::new(SessionHostSignIn::new(&input.url))),
            revealed: false,
            error: None,
            answered: false,
            timer: Some(timer),
        }
    }

    fn update(&mut self, msg: Self::Message, ctx: &ComponentContext<Self>) {
        match msg {
            HostMsg::Ready(Ok(())) => {}
            HostMsg::Ready(Err(e)) | HostMsg::Failed(e) => {
                self.revealed = true;
                self.error = Some(e);
                self.answer(None);
            }
            HostMsg::Reveal => self.revealed = true,
            HostMsg::Done(url) => {
                self.answer(Some(url));
                Self::close(ctx);
            }
            HostMsg::Cancel => {
                if !self.answered {
                    self.flow.borrow_mut().cancel();
                    self.answer(None);
                    Self::close(ctx);
                }
            }
        }
    }

    fn view(&self, _input: &Self::Input, ctx: &mut ViewContext<Self>) -> View {
        ctx.window_title(format!("{} - {}", SessionHostSignIn::TITLE, self.name));
        ctx.window_visuals(
            WindowVisuals::new()
                .backdrop(WindowBackdrop::None)
                .client_size(480.0, 640.0),
        );
        {
            let element = self.webview_ref.clone();
            let flow: Rc<RefCell<dyn NavigationFlow>> = self.flow.clone();
            let url = self.flow.borrow().url().to_string();
            let sender = ctx.sender();
            let on_end: Rc<dyn Fn(FlowEnd)> = Rc::new(move |end| {
                sender.send(match end {
                    FlowEnd::Done(url) => HostMsg::Done(url),
                    FlowEnd::Failed(message) => {
                        HostMsg::Failed(format!("Sign-in to the Cloud PC failed: {message}"))
                    }
                });
            });
            let sender = ctx.sender();
            let on_ready: Rc<dyn Fn(Result<ICoreWebView2, String>)> = Rc::new(move |result| {
                sender.send(HostMsg::Ready(result.map(|_| ())));
            });
            ctx.use_effect("webview", (), move || {
                if !attach_flow(
                    &element,
                    url,
                    SessionHostSignIn::USER_AGENT.to_string(),
                    None,
                    flow,
                    on_end,
                    on_ready,
                ) {
                    log::warn!("The session sign-in web view was not ready");
                }
                None
            });
        }

        let mut layers: Vec<(&'static str, View)> = vec![(
            "browser",
            WebView2::new().element_ref(&self.webview_ref).into(),
        )];
        if !self.revealed {
            layers.push((
                "busy",
                busy_overlay(&format!("Signing in to {}…", self.name)),
            ));
        }
        if let Some(error) = &self.error {
            layers.push((
                "error",
                overlay(
                    StackPanel::new().spacing(12.0).children((
                        InfoBar::new()
                            .title("Sign-in failed")
                            .message(error.clone())
                            .severity(InfoBarSeverity::Error)
                            .is_open(true)
                            .is_closable(false),
                        Button::new()
                            .on_click(ctx.message(HostMsg::Cancel))
                            .content("Close"),
                    )),
                ),
            ));
        }
        Grid::new()
            .rows([GridLength::Star(1.0), GridLength::Auto])
            .children((
                Grid::new().grid_row(0).keyed_children(layers),
                Grid::new()
                    .grid_row(1)
                    .margin(Thickness::uniform(12.0))
                    .children((Button::new()
                        .horizontal_alignment(HorizontalAlignment::Right)
                        .on_click(ctx.message(HostMsg::Cancel))
                        .content("Cancel"),)),
            ))
    }
}

// ----- Custom display scale -----

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaleInput {
    pub id: u64,
    pub name: String,
    pub current: u32,
}

#[derive(Clone)]
pub enum ScaleMsg {
    Changed(Option<f64>),
    Apply,
    Cancel,
}

pub struct ScaleWindow {
    id: u64,
    name: String,
    value: u32,
}

impl ScaleWindow {
    fn close(ctx: &ComponentContext<Self>) {
        if !ctx.window().request_close() {
            log::warn!("Could not close the display scale window");
        }
    }
}

impl Component for ScaleWindow {
    type Input = ScaleInput;
    type Message = ScaleMsg;

    fn create(input: &Self::Input, _ctx: &ComponentContext<Self>) -> Self {
        Self {
            id: input.id,
            name: input.name.clone(),
            value: input.current.clamp(MIN_SCALE, MAX_SCALE),
        }
    }

    fn update(&mut self, msg: Self::Message, ctx: &ComponentContext<Self>) {
        match msg {
            ScaleMsg::Changed(Some(value)) if value.is_finite() => {
                self.value = (value.round() as u32).clamp(MIN_SCALE, MAX_SCALE);
            }
            ScaleMsg::Changed(_) => {}
            ScaleMsg::Apply => {
                session_window::set_custom_scale(self.id, self.value);
                Self::close(ctx);
            }
            ScaleMsg::Cancel => Self::close(ctx),
        }
    }

    fn view(&self, _input: &Self::Input, ctx: &mut ViewContext<Self>) -> View {
        ctx.window_title(format!("Display Scale - {}", self.name));
        ctx.window_visuals(
            WindowVisuals::new()
                .backdrop(WindowBackdrop::None)
                .client_size(380.0, 220.0),
        );
        StackPanel::new()
            .spacing(16.0)
            .margin(Thickness::uniform(24.0))
            .children((
                TextBlock::new()
                    .text(format!(
                        "Scale of the remote desktop, from {MIN_SCALE}% to {MAX_SCALE}%."
                    ))
                    .text_wrapping(TextWrapping::Wrap),
                NumberBox::new()
                    .minimum(f64::from(MIN_SCALE))
                    .maximum(f64::from(MAX_SCALE))
                    .value(f64::from(self.value))
                    .on_value_changed(ctx.callback(ScaleMsg::Changed))
                    .slot(NumberBoxSlot::Header, "Scale (%)"),
                StackPanel::new()
                    .orientation(Orientation::Horizontal)
                    .spacing(8.0)
                    .horizontal_alignment(HorizontalAlignment::Right)
                    .children((
                        Button::new()
                            .on_click(ctx.message(ScaleMsg::Cancel))
                            .content("Cancel"),
                        Button::new()
                            .style(ButtonStyle::Accent)
                            .on_click(ctx.message(ScaleMsg::Apply))
                            .content("Apply"),
                    )),
            ))
    }
}
