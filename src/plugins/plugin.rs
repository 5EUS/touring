use anyhow::{anyhow, Result};
use std::fs;
use std::path::Path;
use std::sync::{atomic::{AtomicU64, Ordering}, Arc};
use std::time::{Duration, Instant};
use url::Url;
use wasmtime::{component::*, Engine, Store};
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wasi_http;

use crate::plugins::*; // bindgen types (Media, Unit, Asset, MediaType, UnitKind, AssetKind, ProviderCapabilities)
use crate::plugins::host::Host;
use crate::plugins::config::PluginConfig;

pub(crate) struct Plugin {
    pub(crate) name: String,
    pub(crate) store: Store<Host>,
    pub(crate) bindings: Library,
    pub(crate) caps: Option<ProviderCapabilities>,
    pub(crate) rate_limit: Duration,
    pub(crate) slow_warn: Duration,
    pub(crate) call_timeout: Duration,
    pub(crate) last_call: Option<Instant>,
    pub(crate) epoch_ticks: Arc<AtomicU64>,
    pub(crate) epoch_interval: Duration,
    pub(crate) allowed_hosts: Option<Vec<String>>,
    pub(crate) _instance: wasmtime::component::Instance,
    pub(crate) _component: Component,
}

impl Plugin {
    pub fn new(engine: &Engine, plugin_path: &Path, epoch_ticks: Arc<AtomicU64>, epoch_interval: Duration) -> Result<Self> {
        let component = Component::from_file(engine, plugin_path)?;

        // Load plugin config (<name>.toml next to wasm)
        let cfg_path = plugin_path.with_extension("toml");
        let cfg: PluginConfig = fs::read_to_string(&cfg_path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        let allowed_hosts: Option<Vec<String>> = cfg.allowed_hosts.as_ref().map(|v|
            v.iter().map(|h| h.trim().to_ascii_lowercase()).filter(|h| !h.is_empty()).collect()
        );

        // Initialize WASI context for the plugin
        let mut builder = WasiCtxBuilder::new();
        builder.inherit_stdout().inherit_stderr().inherit_env();
        if let Some(list) = &allowed_hosts {
            builder.env("TOURING_ALLOWED_HOSTS", list.join(","));
        }
        let wasi = builder.build();

        // Build HTTP context
        let http = wasmtime_wasi_http::WasiHttpCtx::new();

        let host = Host { 
            wasi,
            table: wasmtime_wasi::ResourceTable::new(),
            http,
        };
        let mut store = Store::new(engine, host);

        // Safe far-future deadline
        let now = epoch_ticks.load(Ordering::Relaxed);
        let far = now.saturating_add(1_000_000_000);
        store.set_epoch_deadline(far);

        // Create linker + instance
        let mut linker = Linker::new(engine);
        wasmtime_wasi::add_to_linker_sync(&mut linker)?;
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;
        let instance = linker.instantiate(&mut store, &component)?;
        let bindings = Library::new(&mut store, &instance)?;

        // Prefetch capabilities
        let caps = match bindings.call_getcapabilities(&mut store) {
            Ok(c) => Some(c),
            Err(e) => { eprintln!("Failed to get capabilities for {}: {}", plugin_path.display(), e); None }
        };

        Ok(Self {
            name: plugin_path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_string(),
            store,
            bindings,
            caps,
            rate_limit: Duration::from_millis(cfg.rate_limit_ms.unwrap_or(150)),
            slow_warn: Duration::from_secs(5),
            call_timeout: Duration::from_millis(cfg.call_timeout_ms.unwrap_or(15_000)),
            last_call: None,
            epoch_ticks,
            epoch_interval,
            allowed_hosts,
            _instance: instance,
            _component: component,
        })
    }

    pub(crate) fn set_deadline(&mut self) {
        let now = self.epoch_ticks.load(Ordering::Relaxed);
        let per_tick_ms = self.epoch_interval.as_millis().max(1) as u128;
        let need = ((self.call_timeout.as_millis() + per_tick_ms - 1) / per_tick_ms) as u64;
        let deadline = now.saturating_add(need);
        self.store.set_epoch_deadline(deadline);
    }

    pub(crate) fn clear_deadline(&mut self) {
        let now = self.epoch_ticks.load(Ordering::Relaxed);
        let far = now.saturating_add(1_000_000_000);
        self.store.set_epoch_deadline(far);
    }

    pub(crate) fn throttle(&mut self) {
        if let Some(last) = self.last_call {
            let elapsed = last.elapsed();
            if elapsed < self.rate_limit {
                std::thread::sleep(self.rate_limit - elapsed);
            }
        }
        self.last_call = Some(Instant::now());
    }

    pub(crate) fn warn_if_slow(&self, start: Instant, op: &str) {
        let elapsed = start.elapsed();
        if elapsed > self.slow_warn {
            eprintln!("Plugin {} {} took {:?}", self.name, op, elapsed);
        }
    }

    pub(crate) fn retry_once<T, F>(&mut self, mut f: F, op: &str) -> Result<T>
    where F: FnMut(&mut Self) -> Result<T> {
        for attempt in 0..2 {
            self.set_deadline();
            let res = f(self);
            self.clear_deadline();
            match res {
                Ok(v) => return Ok(v),
                Err(e1) if attempt == 0 => {
                    eprintln!("Plugin {} {} failed: {}. Retrying...", self.name, op, e1);
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
                Err(e) => return Err(anyhow!("{} after retry: {}", op, e)),
            }
        }
        unreachable!();
    }

    pub(crate) fn url_allowed(&self, url: &str) -> bool {
        match &self.allowed_hosts {
            None => true,
            Some(list) => {
                if list.is_empty() { return false; }
                let Ok(parsed) = Url::parse(url) else { return false; };
                match parsed.scheme() { "http" | "https" => {}, _ => return false }
                let Some(host) = parsed.host_str() else { return false; };
                let host = host.to_ascii_lowercase();
                list.iter().any(|allowed| {
                    let a = allowed.as_str();
                    if let Some(stripped) = a.strip_prefix("*.") {
                        host == stripped || host.ends_with(&format!(".{}", stripped))
                    } else { host == a }
                })
            }
        }
    }

    pub(crate) fn fetch_media_list(&mut self, kind: MediaType, query: &str) -> Result<Vec<Media>> {
        if matches!(&self.allowed_hosts, Some(v) if v.is_empty()) { return Ok(Vec::new()); }
        self.throttle();
        self.set_deadline();
        let start = Instant::now();
        let res = self.retry_once(|this| {
            this.bindings
                .call_fetchmedialist(&mut this.store, &kind, query)
                .map_err(|e| anyhow!("Failed to call fetchmedialist: {}", e))
        }, "fetchmedialist");
        self.clear_deadline();
        self.warn_if_slow(start, "fetchmedialist");
        let mut list = res?;
        for m in &mut list {
            if let Some(u) = &m.url { if !self.url_allowed(u) { m.url = None; } }
            if let Some(c) = &m.cover_url { if !self.url_allowed(c) { m.cover_url = None; } }
        }
        Ok(list)
    }

    pub(crate) fn fetch_units(&mut self, media_id: &str) -> Result<Vec<Unit>> {
        if matches!(&self.allowed_hosts, Some(v) if v.is_empty()) { return Ok(Vec::new()); }
        self.throttle();
        self.set_deadline();
        let start = Instant::now();
        let res = self.retry_once(|this| {
            this.bindings
                .call_fetchunits(&mut this.store, media_id)
                .map_err(|e| anyhow!("Failed to call fetchunits: {}", e))
        }, "fetchunits");
        self.clear_deadline();
        self.warn_if_slow(start, "fetchunits");
        let mut units = res?;
        for u in &mut units {
            if let Some(uurl) = &u.url { if !self.url_allowed(uurl) { u.url = None; } }
        }
        Ok(units)
    }

    pub(crate) fn fetch_assets(&mut self, unit_id: &str) -> Result<Vec<Asset>> {
        if matches!(&self.allowed_hosts, Some(v) if v.is_empty()) { return Ok(Vec::new()); }
        self.throttle();
        self.set_deadline();
        let start = Instant::now();
        let res = self.retry_once(|this| {
            this.bindings
                .call_fetchassets(&mut this.store, unit_id)
                .map_err(|e| anyhow!("Failed to call fetchassets: {}", e))
        }, "fetchassets");
        self.clear_deadline();
        self.warn_if_slow(start, "fetchassets");
        let assets = res?;
        let filtered: Vec<Asset> = assets.into_iter().filter(|a| self.url_allowed(&a.url)).collect();
        Ok(filtered)
    }

    pub(crate) fn get_capabilities_refresh(&mut self) -> Result<ProviderCapabilities> {
        self.set_deadline();
        let caps = self.bindings
            .call_getcapabilities(&mut self.store)
            .map_err(|e| anyhow!("Failed to call getcapabilities: {}", e));
        self.clear_deadline();
        let caps = caps?;
        self.caps = Some(caps.clone());
        Ok(caps)
    }

    pub(crate) fn get_capabilities_cached(&mut self) -> Result<ProviderCapabilities> {
        if let Some(c) = self.caps.clone() { return Ok(c); }
        self.get_capabilities_refresh()
    }
}
