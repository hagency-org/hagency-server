use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;

use serde::Serialize;
use serde::de::DeserializeOwned;

thread_local! {
    static CACHE: RefCell<HashMap<String, (f64, String)>> = RefCell::new(HashMap::new());
}

pub fn get_cached(key: &str, ttl_ms: f64) -> Option<String> {
    CACHE.with(|c| {
        let cache = c.borrow();
        if let Some((timestamp, value)) = cache.get(key) {
            let now = js_sys::Date::now();
            if now - timestamp < ttl_ms {
                return Some(value.clone());
            }
        }
        None
    })
}

pub fn set_cached(key: &str, value: &str) {
    CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        cache.insert(key.to_string(), (js_sys::Date::now(), value.to_string()));
    });
}

pub fn remove_cached(key: &str) {
    CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        cache.remove(key);
    });
}

pub fn invalidate_cached_prefix(prefix: &str) {
    CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        cache.retain(|key, _| !key.starts_with(prefix));
    });
}

/// Read-through cache for an async fetch.
///
/// Returns the cached value if it exists and is younger than `ttl_ms`.
/// Otherwise awaits `fetch`, stores its successful result under `key`, and
/// returns it. Serialization/deserialization failures fall through to the
/// network so a corrupt cache entry can never wedge a caller.
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

    let value = fetch.await?;

    if let Ok(serialized) = serde_json::to_string(&value) {
        set_cached(key, &serialized);
    }

    Ok(value)
}
