use serde::Deserialize;

#[derive(Debug, Deserialize, Clone, Default)]
pub(crate) struct PluginConfig {
    #[serde(default)]
    pub(crate) allowed_hosts: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) rate_limit_ms: Option<u64>,
    #[serde(default)]
    pub(crate) call_timeout_ms: Option<u64>,
}
