use anyhow::Context;
use reqwest::Url;

use crate::runtime::NetworkPolicy;

#[derive(Debug, Clone)]
pub struct NetworkAccessPolicy {
    policy: NetworkPolicy,
}

impl Default for NetworkAccessPolicy {
    fn default() -> Self {
        Self {
            policy: NetworkPolicy::Disabled,
        }
    }
}

impl NetworkAccessPolicy {
    pub fn new(policy: NetworkPolicy) -> Self {
        Self { policy }
    }

    pub fn policy(&self) -> NetworkPolicy {
        self.policy
    }

    pub fn allows_url(&self, url: &str) -> anyhow::Result<bool> {
        let parsed = Url::parse(url).with_context(|| format!("failed to parse url {url}"))?;
        let host = parsed.host_str().unwrap_or_default();

        Ok(match self.policy {
            NetworkPolicy::Disabled => false,
            NetworkPolicy::LocalOnly => matches!(host, "localhost" | "127.0.0.1"),
            NetworkPolicy::EnabledWithApproval => true,
        })
    }
}
