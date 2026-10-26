//! Microsoft service identifiers, endpoints and scopes shared by every frontend.

/// Application id: D-Bus name, desktop file and icon name.
pub const APP_ID: &str = "com.microsoft.windowsapp.linux";

/// Windows App (MSIX) public client, from its `PublicClientApplicationConfiguration`.
pub const WINDOWS_APP_CLIENT_ID: &str = "4fb5cc57-dbbc-4cdc-9595-748adff5f414";
/// Microsoft Remote Desktop for macOS public client.
pub const MAC_CLIENT_ID: &str = "12a27547-9124-4f81-8064-a78c94625d97";
/// Legacy multi-platform Azure Virtual Desktop public client.
pub const AVD_CLIENT_ID: &str = "a85cf173-4192-42f8-81fa-777a763e6e2c";

/// Multi-tenant authority (work/school and personal accounts).
pub const COMMON_AUTHORITY: &str = "https://login.microsoftonline.com/common";

/// WAM broker redirect URI registered for the Windows App client.
pub const WINDOWS_APP_REDIRECT_URI: &str =
    "ms-appx-web://microsoft.aad.brokerplugin/4fb5cc57-dbbc-4cdc-9595-748adff5f414";
pub const WVD_REDIRECT_URI: &str = "https://www.wvd.microsoft.com/webclient";
pub const NATIVE_CLIENT_REDIRECT_URI: &str =
    "https://login.microsoftonline.com/common/oauth2/nativeclient";

pub const GRAPH_BASE_URL: &str = "https://graph.microsoft.com/beta";
pub const WINDOWS_365_BASE_URL: &str = "https://windows365.microsoft.com";
pub const RDWEB_FEED_URL: &str = "https://rdweb.wvd.microsoft.com/api/arm/feeddiscovery";
pub const DEVICE_LOGIN_URL: &str = "https://login.microsoft.com/device";

/// Headers the Windows App sends to the Windows 365 end-user API.
pub const CLIENT_VERSION: &str = "2.0.1484.0";
pub const SOURCE_CLIENT: &str = "NxtClient";
pub const W365_USER_AGENT: &str = "win365.nxt/2.0.1484.0";
/// User agent the Windows desktop client uses for the RD Web feed.
pub const RDWEB_USER_AGENT: &str = "WindowsDesktop/1.2.5105.0";

/// Browser identities used by the sign-in profiles.
pub const UA_WINDOWS_11_WAM: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; WebView/3.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/70.0.3538.102 Safari/537.36 Edge/18.22631";
pub const UA_LINUX_CHROME: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
pub const UA_MAC_SAFARI: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";
/// Used for the session-host (RDS AAD) sign-in, which Intune treats as a mobile client.
pub const UA_IOS_REMOTE_DESKTOP: &str =
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148";

/// Windows 365 end-user service scope.
pub const WINDOWS_365_IW_SCOPE: &str = "7c0a6aea-533c-458c-9f81-15568f10f6e4/EndUser.Access";
/// Azure Virtual Desktop ARM gateway scope.
pub const AVD_RESOURCE_SCOPE: &str = "9cdead84-a844-4324-93f2-b2e6bb768d07/.default";

/// Microsoft Graph with the permissions consented for the signing-in client. Frontline
/// start/stop additionally needs the delegated `CloudPC.ReadWrite.All` permission.
pub const GRAPH_DEFAULT_SCOPE: &str = "https://graph.microsoft.com/.default";

pub const CLOUD_PC_SCOPES: &[&str] = &["openid", "profile", "offline_access", WINDOWS_365_IW_SCOPE];
