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
        wasmtime_wasi_http::add_only_http_to_linker_sync(&mut linker)?;
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
        let mut list = match res {
            Ok(v) => {
                // Filter out sentinel error entries produced by some guests
                let filtered: Vec<Media> = v.into_iter().filter(|m| m.id != "error" && !m.title.starts_with("HTTP Error:")).collect();
                if !filtered.is_empty() { filtered } else if self.name.contains("mangadex_plugin") && matches!(kind, MediaType::Manga) {
                    self.mangadex_fallback_search(query).unwrap_or_default()
                } else { Vec::new() }
            }
            Err(_) if self.name.contains("mangadex_plugin") && matches!(kind, MediaType::Manga) => {
                self.mangadex_fallback_search(query).unwrap_or_default()
            }
            Err(_) => Vec::new(),
        };
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
        let mut units = match res {
            Ok(v) if !v.is_empty() => v,
            _ if self.name.contains("mangadex_plugin") => {
                self.mangadex_fallback_units(media_id).unwrap_or_default()
            }
            _ => Vec::new(),
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
            this.bindings
                .call_fetchassets(&mut this.store, unit_id)
                .map_err(|e| anyhow!("Failed to call fetchassets: {}", e))
        }, "fetchassets");
        self.clear_deadline();
        self.warn_if_slow(start, "fetchassets");
        let assets = match res {
            Ok(v) if !v.is_empty() => v,
            _ if self.name.contains("mangadex_plugin") => {
                self.mangadex_fallback_assets(unit_id).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        let filtered: Vec<Asset> = assets.into_iter().filter(|a| self.url_allowed(&a.url)).collect();
        Ok(filtered)
    }

    pub(crate) fn get_capabilities_refresh(&mut self) -> Result<ProviderCapabilities> {
        self.throttle();
        self.set_deadline();
        let start = Instant::now();
        let res = self.retry_once(|this| {
            this.bindings
                .call_getcapabilities(&mut this.store)
                .map_err(|e| anyhow!("Failed to call getcapabilities: {}", e))
        }, "getcapabilities");
        self.clear_deadline();
        self.warn_if_slow(start, "getcapabilities");
        match res {
            Ok(c) => {
                // Store for next time
                self.caps = Some(c.clone());
                Ok(c)
            }
            Err(e) => Err(e),
        }
    }

    pub(crate) fn get_capabilities_cached(&mut self) -> Result<ProviderCapabilities> {
        if let Some(c) = &self.caps { return Ok(c.clone()); }
        self.get_capabilities_refresh()
    }

    // --- Host-side MangaDex fallbacks ---

    fn rt_block_on<F, T>(&self, fut: F) -> Result<T>
    where F: std::future::Future<Output = Result<T, reqwest::Error>> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()
            .map_err(|e| anyhow!("tokio rt error: {}", e))?;
        rt.block_on(fut).map_err(|e| anyhow!("http error: {}", e))
    }

    fn mangadex_fallback_search(&self, query: &str) -> Result<Vec<Media>> {
        let q = percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC).to_string();
        let url = format!("https://api.mangadex.org/manga?title={}&limit=20&contentRating[]=safe&contentRating[]=suggestive&includes[]=cover_art", q);
        let body = self.rt_block_on(async move {
            let c = reqwest::Client::builder().user_agent("touring/0.1").build()?;
            let r = c.get(url).send().await?.error_for_status()?;
            r.bytes().await.map(|b| b.to_vec())
        })?;
        let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| anyhow!("parse json: {}", e))?;
        let mut list = Vec::new();
        if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
            for item in arr {
                if let Some(id) = item.get("id").and_then(|s| s.as_str()) {
                    let attrs = item.get("attributes").unwrap_or(&serde_json::Value::Null);
                    let title = pick_map_string_host(attrs.get("title"));
                    let desc = pick_map_string_host(attrs.get("description"));
                    list.push(Media { id: id.to_string(), mediatype: MediaType::Manga, title: title.unwrap_or_else(|| "Untitled".into()), description: desc, url: None, cover_url: None });
                }
            }
        }
        Ok(list)
    }

    fn mangadex_fallback_units(&self, manga_id: &str) -> Result<Vec<Unit>> {
        let url = format!("https://api.mangadex.org/manga/{}/feed?limit=100&order[chapter]=asc&translatedLanguage[]=en", manga_id);
        let body = self.rt_block_on(async move {
            let c = reqwest::Client::builder().user_agent("touring/0.1").build()?;
            let r = c.get(url).send().await?.error_for_status()?;
            r.bytes().await.map(|b| b.to_vec())
        })?;
        let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| anyhow!("parse json: {}", e))?;
        let mut out = Vec::new();
        if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
            for item in arr {
                if let Some(id) = item.get("id").and_then(|s| s.as_str()) {
                    let attrs = item.get("attributes").unwrap_or(&serde_json::Value::Null);
                    let raw_title = attrs.get("title").and_then(|s| s.as_str()).unwrap_or("").to_string();
                    let group = attrs.get("volume").and_then(|s| s.as_str()).map(|s| s.to_string());
                    let number_text = attrs.get("chapter").and_then(|s| s.as_str()).map(|s| s.to_string());
                    let number = number_text.as_ref().and_then(|s| s.parse::<f32>().ok());
                    let lang = attrs.get("translatedLanguage").and_then(|s| s.as_str()).map(|s| s.to_string());
                    let published_at = attrs.get("publishAt").or_else(|| attrs.get("readableAt")).or_else(|| attrs.get("createdAt")).and_then(|s| s.as_str()).map(|s| s.to_string());
                    let effective_title = if raw_title.is_empty() {
                        match (&group, &number_text) {
                            (Some(v), Some(c)) => format!("Vol. {} Ch. {}", v, c),
                            (None, Some(c)) => format!("Ch. {}", c),
                            (Some(v), None) => format!("Vol. {}", v),
                            _ => "Untitled Chapter".to_string(),
                        }
                    } else { raw_title };
                    out.push(Unit { id: id.to_string(), title: effective_title, number_text, number, lang, group, url: Some(format!("https://mangadex.org/chapter/{}", id)), published_at, kind: UnitKind::Chapter });
                }
            }
        }
        Ok(out)
    }

    fn mangadex_fallback_assets(&self, chapter_id: &str) -> Result<Vec<Asset>> {
        let url = format!("https://api.mangadex.org/at-home/server/{}", chapter_id);
        let body = self.rt_block_on(async move {
            let c = reqwest::Client::builder().user_agent("touring/0.1").build()?;
            let r = c.get(url).send().await?.error_for_status()?;
            r.bytes().await.map(|b| b.to_vec())
        })?;
        let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| anyhow!("parse json: {}", e))?;
        let base = v.get("baseUrl").and_then(|s| s.as_str()).unwrap_or("");
        let chapter = v.get("chapter").unwrap_or(&serde_json::Value::Null);
        let hash = chapter.get("hash").and_then(|s| s.as_str()).unwrap_or("");
        let files = chapter.get("data").and_then(|a| a.as_array()).cloned().unwrap_or_default();
        if base.is_empty() || hash.is_empty() { return Ok(vec![]); }
        let mut out = Vec::new();
        for f in files {
            if let Some(name) = f.as_str() {
                let url = format!("{}/data/{}/{}", base, hash, name);
                out.push(Asset { url, mime: None, width: None, height: None, kind: AssetKind::Page });
            }
        }
        Ok(out)
    }
}

fn pick_map_string_host(v: Option<&serde_json::Value>) -> Option<String> {
    let map = v?.as_object()?;
    if let Some(s) = map.get("en").and_then(|s| s.as_str()) { if !s.is_empty() { return Some(s.to_string()); } }
    for (_k, val) in map.iter() { if let Some(s) = val.as_str() { if !s.is_empty() { return Some(s.to_string()); } } }
    None
}
