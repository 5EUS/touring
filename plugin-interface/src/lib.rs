pub use wit_bindgen;

// Common utilities and helper functions
pub mod utils {
    use std::collections::HashMap;
    
    /// Pick the first available language from a map, preferring the given languages in order
    pub fn pick_lang(map: &HashMap<String, String>, langs: &[&str]) -> Option<String> {
        for lang in langs {
            if let Some(v) = map.get(*lang) {
                if !v.is_empty() {
                    return Some(v.clone());
                }
            }
        }
        map.values().find(|v| !v.is_empty()).cloned()
    }
}

// Re-export bindings based on feature flags
#[cfg(feature = "host")]
pub mod host {
    // Host bindings will be generated here by the Makefile
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host.rs"));
}

#[cfg(feature = "guest")]
pub mod guest {
    // Guest bindings will be generated here by the Makefile
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/guest.rs"));
}
