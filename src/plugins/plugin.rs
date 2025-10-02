use anyhow::{anyhow, Result};
use std::path::Path;
use std::sync::{atomic::{AtomicU64, Ordering}, Arc};
use std::time::{Duration, Instant};
use url::Url;
use wasmtime::{component::*, Engine, Store};
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wasi_http;
use tracing::{debug, warn, error};

use crate::plugins::*; // bindgen types (Media, Unit, Asset, MediaType, UnitKind, AssetKind, ProviderCapabilities)
use crate::plugins::host::Host;
use crate::plugins::config::PluginConfig;
use tokio::runtime::Runtime;
use std::sync::Arc as StdArc;

#[allow(dead_code)] // Some fields retained for future lifecycle / metrics usage
pub(crate) struct Plugin {
    pub(crate) name: String,
    pub(crate) store: Store<Host>,
    // Keep bindings alive in case future generated code relies on Drop; underscore silences unused warning.
    pub(crate) _bindings: Library,
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
    rt: StdArc<Runtime>,
}

impl Plugin {
    pub async fn new_async(engine: &Engine, plugin_path: &Path, epoch_ticks: Arc<AtomicU64>, epoch_interval: Duration, rt: StdArc<Runtime>) -> Result<Self> {
        let component = Component::from_file(engine, plugin_path)?;
        let cfg_path = plugin_path.with_extension("toml");
        let cfg: PluginConfig = std::fs::read_to_string(&cfg_path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        let allowed_hosts: Option<Vec<String>> = cfg.allowed_hosts.as_ref().map(|v|
            v.iter().map(|h| h.trim().to_ascii_lowercase()).filter(|h| !h.is_empty()).collect()
        );
        let mut builder = WasiCtxBuilder::new();
        builder.inherit_stdout().inherit_stderr().inherit_env();
        if let Some(list) = &allowed_hosts { builder.env("TOURING_ALLOWED_HOSTS", list.join(",")); }
        let wasi = builder.build();
    let http = wasmtime_wasi_http::WasiHttpCtx::new();
    let host = Host { wasi, table: wasmtime_wasi::ResourceTable::new(), http };
        let mut store = Store::new(engine, host);
        let now = epoch_ticks.load(Ordering::Relaxed);
        let far = now.saturating_add(1_000_000_000);
        store.set_epoch_deadline(far);
        let mut linker = Linker::<Host>::new(engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;
    // (If sockets support was required explicitly it would be added here; current API couples http to sockets internally when NetworkCtx present.)
    let instance = linker.instantiate_async(&mut store, &component).await?;
    let bindings = Library::new(&mut store, &instance)?;
        // Defer initial getcapabilities call until explicitly requested to avoid synchronous call on async-enabled engine
        let caps = None;
    // Use a multi-thread runtime so async HTTP tasks can execute even after moving the Plugin to a different thread.
        Ok(Self { name: plugin_path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_string(), store, _bindings: bindings, caps, rate_limit: Duration::from_millis(cfg.rate_limit_ms.unwrap_or(150)), slow_warn: Duration::from_secs(5), call_timeout: Duration::from_millis(cfg.call_timeout_ms.unwrap_or(15_000)), last_call: None, epoch_ticks, epoch_interval, allowed_hosts, _instance: instance, _component: component, rt })
    }
    // Removed legacy synchronous constructor; async instantiation `new_async` is the only path now.

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
            warn!(plugin=%self.name, op, ?elapsed, "plugin operation slow");
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
                    warn!(plugin=%self.name, op, error=%e1, "plugin op failed - retrying");
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
    debug!(plugin=%self.name, ?kind, query, "fetch_media_list start");
        let res = self.retry_once(|this| {
            // Try plain export name first, then prefixed variant
            let func = this._instance.get_func(&mut this.store, "fetchmedialist")
                .or_else(|| this._instance.get_func(&mut this.store, "library#fetchmedialist"))
                .ok_or_else(|| anyhow!("missing export fetchmedialist (tried 'fetchmedialist' and 'library#fetchmedialist')"))?;
            let typed = func.typed::<(MediaType, String), (Vec<Media>,)>(&this.store)?;
            let (result_vec,) = this.rt.block_on(typed.call_async(&mut this.store, (kind.clone(), query.to_string())))
                .map_err(|e| anyhow!("Failed to call fetchmedialist async: {}", e))?;
            Ok(result_vec)
        }, "fetchmedialist");
        self.clear_deadline();
        self.warn_if_slow(start, "fetchmedialist");
        let mut list = match res {
            Ok(v) => {
                // Inspect and log sentinel error entries before filtering them out
                let mut filtered: Vec<Media> = Vec::with_capacity(v.len());
                let mut suppressed = 0usize;
                for m in v.into_iter() {
                    if m.id == "error" || m.title.starts_with("HTTP Error:") { suppressed += 1; continue; }
                    filtered.push(m);
                }
                if suppressed > 0 { debug!(plugin=%self.name, query, suppressed, "suppressed sentinel error entries"); }
                filtered
            }
            Err(e) => {
                error!(plugin=%self.name, error=%e, "fetchmedialist failed");
                Vec::new()
            }
        };
        debug!(plugin=%self.name, query, count=list.len(), "fetch_media_list done");
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
            let func = this._instance.get_func(&mut this.store, "fetchunits")
                .or_else(|| this._instance.get_func(&mut this.store, "library#fetchunits"))
                .ok_or_else(|| anyhow!("missing export fetchunits (tried 'fetchunits' and 'library#fetchunits')"))?;
            let typed = func.typed::<(String,), (Vec<Unit>,)>(&this.store)?;
            let (result_vec,) = this.rt.block_on(typed.call_async(&mut this.store, (media_id.to_string(),)))
                .map_err(|e| anyhow!("Failed to call fetchunits async: {}", e))?;
            Ok(result_vec)
        }, "fetchunits");
        self.clear_deadline();
        self.warn_if_slow(start, "fetchunits");
        let mut units = match res {
            Ok(v) => v,
            Err(e) => {
                error!(plugin=%self.name, error=%e, "fetchunits failed");
                Vec::new()
            }
        };
        for u in &mut units { if let Some(uurl) = &u.url { if !self.url_allowed(uurl) { u.url = None; } } }
        Ok(units)
    }

    pub(crate) fn fetch_assets(&mut self, unit_id: &str) -> Result<Vec<Asset>> {
        if matches!(&self.allowed_hosts, Some(v) if v.is_empty()) { return Ok(Vec::new()); }
        self.throttle();
        self.set_deadline();
        let start = Instant::now();
        let res = self.retry_once(|this| {
            let func = this._instance.get_func(&mut this.store, "fetchassets")
                .or_else(|| this._instance.get_func(&mut this.store, "library#fetchassets"))
                .ok_or_else(|| anyhow!("missing export fetchassets (tried 'fetchassets' and 'library#fetchassets')"))?;
            let typed = func.typed::<(String,), (Vec<Asset>,)>(&this.store)?;
            let (result_vec,) = this.rt.block_on(typed.call_async(&mut this.store, (unit_id.to_string(),)))
                .map_err(|e| anyhow!("Failed to call fetchassets async: {}", e))?;
            Ok(result_vec)
        }, "fetchassets");
        self.clear_deadline();
        self.warn_if_slow(start, "fetchassets");
        let assets = match res {
            Ok(v) => v,
            Err(e) => {
                error!(plugin=%self.name, error=%e, "fetchassets failed");
                Vec::new()
            }
        };
        let filtered: Vec<Asset> = assets.into_iter().filter(|a| self.url_allowed(&a.url)).collect();
        Ok(filtered)
    }

    pub(crate) fn get_capabilities_refresh(&mut self) -> Result<ProviderCapabilities> {
        self.throttle();
        self.set_deadline();
        let start = Instant::now();
        let res = self.retry_once(|this| {
            let func = this._instance.get_func(&mut this.store, "getcapabilities")
                .or_else(|| this._instance.get_func(&mut this.store, "library#getcapabilities"))
                .ok_or_else(|| anyhow!("missing export getcapabilities (tried 'getcapabilities' and 'library#getcapabilities')"))?;
            let typed = func.typed::<(), (ProviderCapabilities,)>(&this.store)?;
            let (caps,) = this.rt.block_on(typed.call_async(&mut this.store, ()))
                .map_err(|e| anyhow!("Failed to call getcapabilities async: {}", e))?;
            Ok(caps)
        }, "getcapabilities");
        self.clear_deadline();
        self.warn_if_slow(start, "getcapabilities");
        if let Ok(c) = &res { self.caps = Some(c.clone()); }
        res
    }

    pub(crate) fn get_capabilities_cached(&mut self) -> Result<ProviderCapabilities> {
        if let Some(c) = &self.caps { return Ok(c.clone()); }
        self.get_capabilities_refresh()
    }
}

