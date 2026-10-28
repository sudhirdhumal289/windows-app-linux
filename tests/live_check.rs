//! Live end-to-end check against the signed-in account. Run with
//! `cargo test --test live_check -- --ignored --nocapture` after signing in with the app.

use std::sync::Arc;
use windows_app::api::{CloudPcService, GraphApiClient};
use windows_app::auth::AuthManager;
use windows_app::config::constants::{CloudEnvironment, COMMON_AUTHORITY, WINDOWS_APP_CLIENT_ID};
use windows_app::session::{self, SessionOptions};
use windows_app::AppConfig;

#[tokio::test]
#[ignore = "requires live network and a signed-in account"]
async fn live_cloud_pc_launch() {
    let auth = Arc::new(AuthManager::new(COMMON_AUTHORITY, WINDOWS_APP_CLIENT_ID));
    if auth.get_valid_token().await.is_err() {
        eprintln!("Not signed in; skipping");
        return;
    }
    let service = CloudPcService::new(GraphApiClient::new(
        auth,
        "commercial",
        CloudEnvironment::Commercial,
    ));
    let pcs = service
        .list_user_cloud_pcs()
        .await
        .expect("listing Cloud PCs");
    assert!(!pcs.is_empty(), "account has no Cloud PCs");
    println!("Cloud PC: {}", pcs[0].display_name);

    let launch = service
        .fetch_rdp_launch(&pcs[0])
        .await
        .expect("fetching RDP launch info");
    let opts = SessionOptions::from_config(&AppConfig::default(), 1920, 1080, 100, None);
    let prepared = session::prepare(&launch, &opts).expect("preparing session");
    println!("FreeRDP args: {:?}", session::redact_args(&prepared.args));
}
