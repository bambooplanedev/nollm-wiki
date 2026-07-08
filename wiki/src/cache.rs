use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const CACHE_VERSION: u32 = 1;
const HASH_ALGO: &str = "blake3";

#[derive(Serialize, Deserialize)]
pub struct Cache {
    pub version: u32,
    pub hash_algo: String,
    pub tool_version: String,
    pub pages: BTreeMap<String, String>, // id -> fingerprint hex
}

impl Cache {
    pub fn fresh() -> Self {
        Cache {
            version: CACHE_VERSION,
            hash_algo: HASH_ALGO.to_string(),
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            pages: BTreeMap::new(),
        }
    }

    pub fn needs_render(&self, id: &str, fingerprint_hex: &str) -> bool {
        self.pages
            .get(id)
            .map(|f| f != fingerprint_hex)
            .unwrap_or(true)
    }

    pub fn set(&mut self, id: &str, fingerprint_hex: &str) {
        self.pages
            .insert(id.to_string(), fingerprint_hex.to_string());
    }

    pub fn retain_ids(&mut self, live: &BTreeSet<String>) {
        self.pages.retain(|id, _| live.contains(id));
    }
}

fn cache_path(dir: &Path) -> std::path::PathBuf {
    dir.join(".wiki").join("cache.json")
}

pub fn load(dir: &Path) -> Cache {
    let path = cache_path(dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Cache::fresh();
    };
    let Ok(cache) = serde_json::from_str::<Cache>(&text) else {
        return Cache::fresh();
    };
    if cache.version != CACHE_VERSION
        || cache.hash_algo != HASH_ALGO
        || cache.tool_version != env!("CARGO_PKG_VERSION")
    {
        return Cache::fresh();
    }
    cache
}

/// The page ids recorded in `dir`'s cache, IGNORING the version guard — for
/// migration diagnostics (files the compiler previously wrote). Empty if the
/// cache is absent or unparseable.
pub fn prior_page_ids(dir: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(cache_path(dir))
        .ok()
        .and_then(|t| serde_json::from_str::<Cache>(&t).ok())
        .map(|c| c.pages.into_keys().collect())
        .unwrap_or_default()
}

pub fn save(dir: &Path, cache: &Cache) -> std::io::Result<()> {
    let path = cache_path(dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cache)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use tempfile::tempdir;

    #[test]
    fn fresh_cache_forces_render_then_skips_when_unchanged() {
        let mut c = Cache::fresh();
        assert!(c.needs_render("a", "deadbeef"));
        c.set("a", "deadbeef");
        assert!(!c.needs_render("a", "deadbeef"));
        assert!(c.needs_render("a", "feedface")); // fingerprint changed
    }

    #[test]
    fn roundtrip_and_version_mismatch_resets() {
        let dir = tempdir().unwrap();
        let mut c = Cache::fresh();
        c.set("a", "abc");
        save(dir.path(), &c).unwrap();
        let loaded = load(dir.path());
        assert!(!loaded.needs_render("a", "abc"));

        // Corrupt the version → load returns fresh (everything re-renders).
        let mut bad = load(dir.path());
        bad.version = 999;
        save(dir.path(), &bad).unwrap();
        let reset = load(dir.path());
        assert!(reset.needs_render("a", "abc"));
    }

    #[test]
    fn retain_prunes_deleted_pages() {
        let mut c = Cache::fresh();
        c.set("a", "1");
        c.set("b", "2");
        let live: BTreeSet<String> = ["a".to_string()].into_iter().collect();
        c.retain_ids(&live);
        assert!(!c.pages.contains_key("b"));
    }

    #[test]
    fn prior_page_ids_reads_ignoring_version() {
        let dir = tempdir().unwrap();
        let mut c = Cache::fresh();
        c.version = 999; // a version `load` would reject
        c.set("a", "1");
        c.set("b", "2");
        save(dir.path(), &c).unwrap();
        let ids = prior_page_ids(dir.path());
        assert!(ids.contains("a") && ids.contains("b"));
        // Sanity: `load` would have reset to empty for this bad version.
        assert!(load(dir.path()).pages.is_empty());
    }
}
