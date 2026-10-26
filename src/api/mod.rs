//! Windows 365 / Microsoft Graph / RD Web service clients.

pub mod avd;
pub mod client;
pub mod cloudpc;

pub use avd::{AvdResource, AvdResourceKind};
pub use client::GraphApiClient;
pub use cloudpc::{launch_resource_id, CloudPcService, RdpLaunch, Snapshot, TroubleshootOutcome};
