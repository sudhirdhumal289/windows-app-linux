use std::sync::Arc;
use windows_app_linux::api::{CloudPcService, GraphApiClient};
use windows_app_linux::auth::AuthManager;
use windows_app_linux::config::{DEFAULT_AUTHORITY, OFFICIAL_CLIENT_ID};

#[tokio::test]
#[ignore = "requires live network and active token"]
async fn test_live_cloud_pc_fetch() {
    let auth = Arc::new(AuthManager::new(DEFAULT_AUTHORITY, OFFICIAL_CLIENT_ID));

    if let Ok(token) = auth.get_valid_token().await {
        println!("Found valid token (len: {})", token.len());
        let client = GraphApiClient::new(auth.clone(), "commercial");
        let service = CloudPcService::new(client);

        let pcs = service
            .list_user_cloud_pcs()
            .await
            .expect("Failed to list cloud pcs");
        assert!(!pcs.is_empty(), "Should have retrieved at least 1 Cloud PC");
        println!("Cloud PC: {:?}", pcs[0]);

        match service.fetch_rdp_file_and_token(&pcs[0]).await {
            Ok((rdp, avd_token)) => {
                std::fs::write("/tmp/test_cloudpc.rdp", &rdp).unwrap();
                std::fs::write("/tmp/test_avd_token.txt", &avd_token).unwrap();
                println!("Saved /tmp/test_cloudpc.rdp and /tmp/test_avd_token.txt");
            }
            Err(e) => {
                println!("Failed to fetch rdp: {e}");
            }
        }
    }
}
