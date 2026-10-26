//! Shared WebKit network session so every sign-in web view uses one persistent cookie jar.

use crate::config::AppConfig;
use gtk4::gio;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use webkit6::{CookieAcceptPolicy, CookieManager, CookiePersistentStorage, NetworkSession};

static IS_UPDATING_COOKIES: AtomicBool = AtomicBool::new(false);

thread_local! {
    static SHARED_SESSION: RefCell<Option<NetworkSession>> = const { RefCell::new(None) };
}

/// Converts in-memory session cookies into 30-day persistent cookies
/// so corporate SSO (Okta, Microsoft Entra ID) sessions survive across process restarts.
pub fn persist_session_cookies() {
    if IS_UPDATING_COOKIES
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    let session = shared_network_session();
    if let Some(cm) = session.cookie_manager() {
        let cm_clone = cm.clone();
        CookieManager::all_cookies(&cm, None::<&gio::Cancellable>, move |res| {
            if let Ok(cookies) = res {
                for mut cookie in cookies {
                    if cookie.expires().is_none() {
                        if let Ok(future) =
                            gtk4::glib::DateTime::now_utc().and_then(|t| t.add_days(30))
                        {
                            cookie.set_expires(&future);
                            CookieManager::add_cookie(
                                &cm_clone,
                                &cookie,
                                None::<&gio::Cancellable>,
                                |_| {},
                            );
                        }
                    }
                }
            }
            IS_UPDATING_COOKIES.store(false, Ordering::SeqCst);
        });
    } else {
        IS_UPDATING_COOKIES.store(false, Ordering::SeqCst);
    }
}

/// Returns a shared, persistent WebKit NetworkSession with cookies and storage saved to disk.
/// Implements a process-wide singleton so all login and RDP authentication sessions share
/// the exact same in-memory and on-disk cookie jar.
pub fn shared_network_session() -> NetworkSession {
    SHARED_SESSION.with(|cell| {
        let mut opt = cell.borrow_mut();
        if let Some(ref session) = *opt {
            session.clone()
        } else {
            let base_dir = AppConfig::config_dir();
            let data_dir = base_dir.join("webkit_data");
            let cache_dir = base_dir.join("webkit_cache");
            let _ = std::fs::create_dir_all(&data_dir);
            let _ = std::fs::create_dir_all(&cache_dir);

            let session = NetworkSession::new(data_dir.to_str(), cache_dir.to_str());
            if let Some(cm) = session.cookie_manager() {
                cm.set_accept_policy(CookieAcceptPolicy::Always);
                let cookie_file = data_dir.join("cookies.sqlite");
                cm.set_persistent_storage(
                    &cookie_file.to_string_lossy(),
                    CookiePersistentStorage::Sqlite,
                );

                // Safe automatic cookie persistence listener
                cm.connect_changed(|_| {
                    persist_session_cookies();
                });
            }
            *opt = Some(session.clone());
            session
        }
    })
}
