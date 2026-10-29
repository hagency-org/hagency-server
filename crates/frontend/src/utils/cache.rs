use serde::{Serialize, de::DeserializeOwned};
use std::{cell::RefCell, collections::HashMap, future::Future};

const MAX_ENTRIES: usize = 256;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 256 * 1024;
const MAX_AGE_MS: f64 = 5.0 * 60.0 * 1000.0;

#[derive(Default)]
struct Cache {
    entries: HashMap<String, (f64, String)>,
    bytes: usize,
    revision: u64,
    generation: u64,
}
impl Cache {
    fn sync_session(&mut self, generation: u64) {
        if self.generation != generation {
            self.invalidate("");
            self.generation = generation;
        }
    }
    fn retain(&mut self, keep: impl Fn(&str, f64) -> bool) {
        self.entries.retain(|key, (time, _)| keep(key, *time));
        self.bytes = self
            .entries
            .iter()
            .map(|(key, (_, value))| key.len() + value.len())
            .sum();
    }
    fn prune(&mut self, now: f64) {
        self.retain(|_, time| (0.0..MAX_AGE_MS).contains(&(now - time)));
    }
    fn get(&mut self, key: &str, ttl: f64, now: f64) -> Option<String> {
        self.prune(now);
        if !ttl.is_finite() || ttl <= 0.0 {
            return None;
        }
        if self
            .entries
            .get(key)
            .is_some_and(|(time, _)| now - time >= ttl)
        {
            self.retain(|entry, _| entry != key);
        }
        self.entries.get(key).map(|(_, value)| value.clone())
    }
    fn insert(&mut self, key: &str, value: &str, now: f64) {
        self.prune(now);
        let size = key.len().saturating_add(value.len());
        self.retain(|entry, _| entry != key);
        if size > MAX_ENTRY_BYTES {
            return;
        }
        while self.entries.len() >= MAX_ENTRIES || self.bytes + size > MAX_BYTES {
            let oldest = self
                .entries
                .iter()
                .min_by(|a, b| a.1.0.total_cmp(&b.1.0))
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else {
                break;
            };
            self.retain(|entry, _| entry != oldest);
        }
        self.bytes += size;
        self.entries.insert(key.into(), (now, value.into()));
    }
    fn invalidate(&mut self, prefix: &str) {
        self.revision = self.revision.wrapping_add(1);
        self.retain(|key, _| !key.starts_with(prefix));
    }
}
thread_local! { static CACHE: RefCell<Cache> = RefCell::new(Cache::default()); }

fn now() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
            * 1000.0
    }
}

pub fn get_cached(key: &str, ttl_ms: f64) -> Option<String> {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.sync_session(super::storage::session_generation());
        cache.get(key, ttl_ms, now())
    })
}

pub fn set_cached(key: &str, value: &str) {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.sync_session(super::storage::session_generation());
        cache.insert(key, value, now());
    });
}

pub fn clear_cache() {
    invalidate_cached_prefix("");
}

pub fn remove_cached(key: &str) {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.revision = cache.revision.wrapping_add(1);
        cache.retain(|entry, _| entry != key);
    });
}

pub fn invalidate_cached_prefix(prefix: &str) {
    CACHE.with(|cache| cache.borrow_mut().invalidate(prefix));
}

/// Bounded per-session read-through cache. An old in-flight response cannot
/// refill entries after logout, account replacement or mutation invalidation.
pub async fn cached<T, E, Fut>(key: &str, ttl_ms: f64, fetch: Fut) -> Result<T, E>
where
    T: Serialize + DeserializeOwned,
    Fut: Future<Output = Result<T, E>>,
{
    if let Some(hit) = get_cached(key, ttl_ms)
        && let Ok(value) = serde_json::from_str::<T>(&hit)
    {
        return Ok(value);
    }
    let generation = super::storage::session_generation();
    let revision = CACHE.with(|cache| cache.borrow().revision);
    let value = fetch.await?;
    if super::storage::session_generation() == generation
        && CACHE.with(|cache| cache.borrow().revision == revision)
        && let Ok(serialized) = serde_json::to_string(&value)
    {
        set_cached(key, &serialized);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_enforces_entry_byte_and_age_bounds() {
        let mut cache = Cache::default();
        for index in 0..MAX_ENTRIES + 10 {
            cache.insert(&index.to_string(), "value", index as f64);
        }
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert!(!cache.entries.contains_key("0"));
        cache.insert("too_large", &"x".repeat(MAX_ENTRY_BYTES), 1000.0);
        assert!(!cache.entries.contains_key("too_large"));
        for index in 0..100 {
            cache.insert(
                &format!("big{index}"),
                &"x".repeat(128 * 1024),
                2000.0 + index as f64,
            );
        }
        assert!(cache.bytes <= MAX_BYTES);
        assert!(cache.entries.len() <= MAX_ENTRIES);
        assert!(cache.get("big99", 10.0, 2110.0).is_none());
        cache.prune(MAX_AGE_MS + 3000.0);
        assert!(cache.entries.is_empty());
        assert_eq!(cache.bytes, 0);
    }
    #[test]
    fn invalidation_during_fetch_prevents_stale_refill() {
        clear_cache();
        let value: Result<u64, ()> =
            futures::executor::block_on(cached("account", 1000.0, async {
                invalidate_cached_prefix("account");
                Ok(7)
            }));
        assert_eq!(value, Ok(7));
        assert!(get_cached("account", 1000.0).is_none());
    }
    #[test]
    fn replacing_session_during_fetch_prevents_stale_refill() {
        clear_cache();
        let state = super::super::storage::session_state();
        state.set("access_token", "old");
        let value: Result<u64, ()> =
            futures::executor::block_on(cached("account", 1000.0, async {
                state.set("access_token", "new");
                Ok(8)
            }));
        assert_eq!(value, Ok(8));
        assert!(get_cached("account", 1000.0).is_none());
    }
    #[test]
    fn memory_session_replacement_drops_existing_account_cache() {
        set_cached("permissions", "old-account");
        super::super::storage::session_state().set("access_token", "other-account");
        assert!(get_cached("permissions", 1000.0).is_none());
    }
}
