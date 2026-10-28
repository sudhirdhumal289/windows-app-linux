//! Azure Virtual Desktop workspace feed (desktops and RemoteApps published to the user).
//!
//! The feed is discovered at [`RDWEB_FEED_URL`], which lists one feed per tenant; each
//! feed is an RD Web `ResourceCollection` with the resources and their signed `.rdp` URLs.

use crate::api::cloudpc::CloudPcService;
use crate::config::{AVD_RESOURCE_SCOPE, RDWEB_USER_AGENT};
use crate::error::{AppError, Result};
use log::{debug, warn};
use serde::{Deserialize, Serialize};

/// What an AVD resource opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AvdResourceKind {
    Desktop,
    RemoteApp,
}

/// A desktop or RemoteApp published to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvdResource {
    pub id: String,
    pub title: String,
    pub kind: AvdResourceKind,
    /// Workspace (feed publisher) name.
    pub workspace: String,
    pub tenant: String,
    /// Signed `.rdp` file URL.
    pub rdp_url: Option<String>,
    pub folder: Option<String>,
}

impl AvdResource {
    #[must_use]
    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            AvdResourceKind::Desktop => "Desktop",
            AvdResourceKind::RemoteApp => "RemoteApp",
        }
    }
}

/// One tenant feed from the discovery document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantFeed {
    pub tenant: String,
    pub url: String,
}

/// Parses the `TenantFeedURLs` discovery document.
pub fn parse_discovery(xml: &str) -> Result<Vec<TenantFeed>> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| AppError::Config(format!("invalid AVD feed discovery document: {e}")))?;
    Ok(doc
        .descendants()
        .filter(|n| n.tag_name().name() == "TenantFeedURL")
        .filter_map(|n| {
            let url = n
                .attribute("FeedURL")
                .map(str::to_string)
                .or_else(|| n.text().map(|t| t.trim().to_string()))
                .filter(|u| u.starts_with("https://"))?;
            Some(TenantFeed {
                tenant: n
                    .attribute("TenantDisplayName")
                    .unwrap_or_default()
                    .to_string(),
                url,
            })
        })
        .collect())
}

/// Parses an RD Web `ResourceCollection` feed.
pub fn parse_feed(xml: &str, tenant: &str) -> Result<Vec<AvdResource>> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| AppError::Config(format!("invalid AVD feed: {e}")))?;
    let mut resources = Vec::new();
    for publisher in doc
        .descendants()
        .filter(|n| n.tag_name().name() == "Publisher")
    {
        let workspace = publisher.attribute("Name").unwrap_or_default();
        for res in publisher
            .descendants()
            .filter(|n| n.tag_name().name() == "Resource")
        {
            let (Some(id), Some(title)) = (res.attribute("ID"), res.attribute("Title")) else {
                continue;
            };
            let kind = match res.attribute("Type") {
                Some(t) if t.eq_ignore_ascii_case("RemoteApp") => AvdResourceKind::RemoteApp,
                _ => AvdResourceKind::Desktop,
            };
            let rdp_url = descendant(res, "ResourceFile")
                .and_then(|f| f.attribute("URL"))
                .map(str::to_string);
            let folder = descendant(res, "Folder")
                .and_then(|f| f.attribute("Name"))
                .filter(|f| !f.is_empty() && *f != "/")
                .map(|f| f.trim_matches('/').to_string());
            resources.push(AvdResource {
                id: id.to_string(),
                title: title.to_string(),
                kind,
                workspace: workspace.to_string(),
                tenant: tenant.to_string(),
                rdp_url,
                folder,
            });
        }
    }
    Ok(resources)
}

fn descendant<'a, 'i>(
    node: roxmltree::Node<'a, 'i>,
    name: &str,
) -> Option<roxmltree::Node<'a, 'i>> {
    node.descendants().find(|c| c.tag_name().name() == name)
}

impl CloudPcService {
    /// Lists AVD desktops and RemoteApps from every tenant feed. Accounts without AVD
    /// access yield an empty list.
    pub async fn list_avd_resources(&self) -> Result<Vec<AvdResource>> {
        let token = self
            .client()
            .auth()
            .token_for_scope(AVD_RESOURCE_SCOPE)
            .await?;
        let discovery_url = self.client().environment().rdweb_feed_url();
        let Some(discovery) = self
            .fetch_feed_xml(
                discovery_url,
                &token,
                "application/x-msts-radc-discovery+xml, text/xml",
            )
            .await?
        else {
            return Ok(Vec::new());
        };
        let mut resources = Vec::new();
        for feed in parse_discovery(&discovery)? {
            match self
                .fetch_feed_xml(&feed.url, &token, "application/x-msts-radc+xml, text/xml")
                .await
            {
                Ok(Some(xml)) => match parse_feed(&xml, &feed.tenant) {
                    Ok(mut items) => resources.append(&mut items),
                    Err(e) => warn!("AVD feed for {}: {e}", feed.tenant),
                },
                Ok(None) => {}
                Err(e) => warn!("AVD feed for {}: {e}", feed.tenant),
            }
        }
        let mut seen = std::collections::HashSet::new();
        resources.retain(|r| seen.insert(r.id.clone()));
        debug!("Loaded {} AVD resources", resources.len());
        Ok(resources)
    }

    /// GETs an XML feed document; `None` when the account has no access.
    async fn fetch_feed_xml(&self, url: &str, token: &str, accept: &str) -> Result<Option<String>> {
        let res = self
            .client()
            .http()
            .get(url)
            .bearer_auth(token)
            .header("Accept", accept)
            .header("User-Agent", RDWEB_USER_AGENT)
            .header("x-ms-user-agent", RDWEB_USER_AGENT)
            .send()
            .await?;
        let status = res.status();
        if matches!(status.as_u16(), 401 | 403 | 404) {
            debug!("AVD feed {url}: HTTP {status}");
            return Ok(None);
        }
        if !status.is_success() {
            return Err(AppError::GraphApi {
                status: status.as_u16(),
                message: "the Azure Virtual Desktop feed is unavailable".into(),
            });
        }
        Ok(Some(res.text().await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_discovery_document() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
            <TenantFeedURLs xmlns="http://schemas.microsoft.com/ts/2007/05/tswf" ARMResources="true">
              <TenantFeedURL TenantDisplayName="Contoso" TenantId="t1"
                FeedURL="https://rdweb.wvd.microsoft.com/api/arm/feeddiscovery/tenants/t1/feed"/>
              <TenantFeedURL TenantDisplayName="Bad" FeedURL="http://insecure"/>
            </TenantFeedURLs>"#;
        assert_eq!(
            parse_discovery(xml).unwrap(),
            vec![TenantFeed {
                tenant: "Contoso".into(),
                url: "https://rdweb.wvd.microsoft.com/api/arm/feeddiscovery/tenants/t1/feed".into(),
            }]
        );
        assert!(parse_discovery("<oops").is_err());
    }

    #[test]
    fn parses_resource_collection() {
        let xml = r#"<ResourceCollection xmlns="http://schemas.microsoft.com/ts/2007/05/tswf">
          <Publisher Name="Engineering" ID="p1">
            <Resources>
              <Resource ID="r-desk" Title="Dev Desktop" Type="Desktop">
                <Icons><Icon32 FileURL="https://x/icon.png"/></Icons>
                <Folders><Folder Name="/"/></Folders>
                <HostingTerminalServers><HostingTerminalServer>
                  <ResourceFile FileExtension=".rdp" URL="https://rdweb/rdps/r-desk.rdp"/>
                </HostingTerminalServer></HostingTerminalServers>
              </Resource>
              <Resource ID="r-app" Title="Excel" Type="RemoteApp">
                <Folders><Folder Name="/Office"/></Folders>
              </Resource>
              <Resource Title="no id"/>
            </Resources>
          </Publisher>
        </ResourceCollection>"#;
        let items = parse_feed(xml, "Contoso").unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, AvdResourceKind::Desktop);
        assert_eq!(items[0].workspace, "Engineering");
        assert_eq!(
            items[0].rdp_url.as_deref(),
            Some("https://rdweb/rdps/r-desk.rdp")
        );
        assert_eq!(items[0].folder, None);
        assert_eq!(items[1].kind, AvdResourceKind::RemoteApp);
        assert_eq!(items[1].folder.as_deref(), Some("Office"));
        assert_eq!(items[1].rdp_url, None);
    }
}
