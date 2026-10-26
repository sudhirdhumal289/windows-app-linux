//! Toolkit-neutral pieces shared by the native frontends: what the main window lists and
//! offers ([`library`]), how connections are prepared ([`launch`]), the session controller
//! ([`session`]), web-view sign-in ([`signin`]) and every user-facing text ([`text`]).
//!
//! Nothing here touches a UI toolkit, so a frontend only maps these models to native
//! widgets and native events back to these calls.

pub mod launch;
pub mod library;
pub mod session;
pub mod signin;
pub mod text;
