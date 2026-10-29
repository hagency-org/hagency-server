//! Reuse validated JSON projections only for an exact immutable Matrix frame.
//! Every read observes the current frame, including after waiting for a loader.
use hagency_agent_service::{Error, Result, gateway::SharedReadState};
use serde_json::Value;
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

type ReadFrame =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<i64>> + Send>> + Send + Sync>;
type LoadFrame = Arc<
    dyn Fn(String, i64) -> Pin<Box<dyn Future<Output = Result<(Vec<String>, Vec<Value>)>> + Send>>
        + Send
        + Sync,
>;
type Key = (String, i64);

struct Slot {
    value: tokio::sync::Mutex<Option<Arc<Vec<Value>>>>,
    bytes: AtomicUsize,
    touched: AtomicU64,
}
struct StateCache {
    current: ReadFrame,
    load: LoadFrame,
    slots: Mutex<HashMap<Key, Arc<Slot>>>,
    sequence: AtomicU64,
    max_entries: usize,
    max_bytes: usize,
}

fn unavailable() -> Error {
    Error::Unavailable("matrix_state_unavailable")
}

// Budget includes JSON allocation estimates and per-key bookkeeping, rather
// than just the serialized payload length. Returned caller-owned Arcs are not
// retained by the cache after eviction.
fn value_bytes(value: &Value) -> usize {
    std::mem::size_of::<Value>()
        + match value {
            Value::String(text) => text.capacity(),
            Value::Array(values) => values.iter().map(value_bytes).sum::<usize>(),
            Value::Object(values) => values
                .iter()
                .map(|(key, value)| key.capacity() + 64 + value_bytes(value))
                .sum::<usize>(),
            _ => 0,
        }
}
fn key_bytes(key: &Key) -> usize {
    key.0.len() + 256
}
fn total_bytes(slots: &HashMap<Key, Arc<Slot>>) -> usize {
    slots
        .values()
        .map(|slot| slot.bytes.load(Ordering::Relaxed))
        .sum()
}
fn evict(slots: &mut HashMap<Key, Arc<Slot>>, keep: Option<&Key>) -> bool {
    let victim = slots
        .iter()
        .filter(|(key, slot)| {
            Some(*key) != keep && Arc::strong_count(slot) == 1 && slot.value.try_lock().is_ok()
        })
        .min_by_key(|(_, slot)| slot.touched.load(Ordering::Relaxed))
        .map(|(key, _)| key.clone());
    victim.is_some_and(|key| slots.remove(&key).is_some())
}

impl StateCache {
    fn new(current: ReadFrame, load: LoadFrame, max_entries: usize, max_bytes: usize) -> Self {
        Self {
            current,
            load,
            slots: Mutex::new(HashMap::new()),
            sequence: AtomicU64::new(0),
            max_entries,
            max_bytes,
        }
    }
    fn slot(&self, key: &Key) -> Result<Arc<Slot>> {
        let mut slots = self.slots.lock().map_err(|_| unavailable())?;
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        if let Some(slot) = slots.get(key) {
            slot.touched.store(sequence, Ordering::Relaxed);
            return Ok(slot.clone());
        }
        let bytes = key_bytes(key);
        if bytes > self.max_bytes || self.max_entries == 0 {
            return Err(unavailable());
        }
        while slots.len() >= self.max_entries
            || total_bytes(&slots).saturating_add(bytes) > self.max_bytes
        {
            if !evict(&mut slots, None) {
                return Err(unavailable());
            }
        }
        let slot = Arc::new(Slot {
            value: tokio::sync::Mutex::new(None),
            bytes: AtomicUsize::new(bytes),
            touched: AtomicU64::new(sequence),
        });
        slots.insert(key.clone(), slot.clone());
        Ok(slot)
    }
    async fn read(&self, room: String) -> Result<Arc<Vec<Value>>> {
        for _ in 0..3 {
            let frame = (self.current)(room.clone()).await?;
            let key = (room.clone(), frame);
            let slot = self.slot(&key)?;
            let mut value = slot.value.lock().await;
            // Waiting behind a same-frame load must not extend old authority.
            if (self.current)(room.clone()).await? != frame {
                continue;
            }
            if let Some(value) = &*value {
                return Ok(value.clone());
            }
            let (expected, events) = (self.load)(room.clone(), frame).await?;
            crate::agent_appservice::validate_state_completeness(&expected, &events)?;
            if (self.current)(room.clone()).await? != frame {
                continue;
            }
            let events = Arc::new(events);
            let bytes =
                key_bytes(&key).saturating_add(events.iter().map(value_bytes).sum::<usize>());
            let mut slots = self.slots.lock().map_err(|_| unavailable())?;
            while bytes <= self.max_bytes
                && total_bytes(&slots)
                    .saturating_sub(key_bytes(&key))
                    .saturating_add(bytes)
                    > self.max_bytes
            {
                if !evict(&mut slots, Some(&key)) {
                    break;
                }
            }
            if bytes <= self.max_bytes
                && total_bytes(&slots)
                    .saturating_sub(key_bytes(&key))
                    .saturating_add(bytes)
                    <= self.max_bytes
            {
                slot.bytes.store(bytes, Ordering::Relaxed);
                *value = Some(events.clone());
            }
            // An oversized valid frame can be read, but is never retained.
            return Ok(events);
        }
        Err(Error::Unavailable("matrix_state_changed"))
    }
}

pub fn shared_reader() -> SharedReadState {
    let current: ReadFrame = Arc::new(|room| {
        Box::pin(async move {
            let room: palpo::core::OwnedRoomId = room
                .try_into()
                .map_err(|_| Error::Invalid("invalid_room_id"))?;
            palpo::room::get_current_frame_id(&room)
                .await
                .map_err(|_| unavailable())?
                .ok_or_else(unavailable)
        })
    });
    let load: LoadFrame = Arc::new(|_room, frame| {
        Box::pin(async move {
            let expected = palpo::room::state::get_full_state_ids(frame)
                .await
                .map_err(|_| unavailable())?
                .values()
                .map(ToString::to_string)
                .collect();
            let state = palpo::room::state::get_full_state(frame)
                .await
                .map_err(|_| unavailable())?;
            let events = state
                .values()
                .map(|event| serde_json::to_value(event).map_err(|_| unavailable()))
                .collect::<Result<Vec<_>>>()?;
            Ok((expected, events))
        })
    });
    let cache = Arc::new(StateCache::new(current, load, 128, 32 * 1024 * 1024));
    Arc::new(move |room| {
        let cache = cache.clone();
        Box::pin(async move { cache.read(room).await })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicI64, AtomicU8};
    use tokio::sync::{Semaphore, oneshot};

    struct Fixture {
        frame: Arc<AtomicI64>,
        calls: Arc<AtomicUsize>,
        mode: Arc<AtomicU8>,
        gate: Arc<Semaphore>,
        cache: Arc<StateCache>,
    }
    impl Fixture {
        fn new(max_entries: usize, max_bytes: usize) -> Self {
            let frame = Arc::new(AtomicI64::new(1));
            let calls = Arc::new(AtomicUsize::new(0));
            let mode = Arc::new(AtomicU8::new(0));
            let gate = Arc::new(Semaphore::new(0));
            let read_frame = frame.clone();
            let read_mode = mode.clone();
            let current: ReadFrame = Arc::new(move |_| {
                let frame = read_frame.clone();
                let mode = read_mode.clone();
                Box::pin(async move {
                    if mode.load(Ordering::SeqCst) == 4 {
                        return Err(unavailable());
                    }
                    Ok(frame.load(Ordering::SeqCst))
                })
            });
            let (count, behavior, blocker) = (calls.clone(), mode.clone(), gate.clone());
            let moving_frame = frame.clone();
            let load: LoadFrame = Arc::new(move |room, frame| {
                let (count, behavior, blocker) = (count.clone(), behavior.clone(), blocker.clone());
                let moving_frame = moving_frame.clone();
                Box::pin(async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    let mode = behavior.load(Ordering::SeqCst);
                    if mode == 1 {
                        blocker.acquire().await.map_err(|_| unavailable())?.forget();
                    }
                    if mode == 2 {
                        return Err(unavailable());
                    }
                    if mode == 5 {
                        moving_frame.fetch_add(1, Ordering::SeqCst);
                    }
                    let id = format!("${room}-{frame}");
                    let events = vec![
                        json!({"event_id":id,"type":"m.room.member","state_key":"@owner:test","content":{"membership":if frame==1 {"join"} else {"leave"}}}),
                    ];
                    Ok((vec![id], if mode == 3 { vec![] } else { events }))
                })
            });
            let cache = Arc::new(StateCache::new(current, load, max_entries, max_bytes));
            Self {
                frame,
                calls,
                mode,
                gate,
                cache,
            }
        }
        async fn wait_calls(&self, count: usize) {
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while self.calls.load(Ordering::SeqCst) < count {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn matrix_projection_reuses_exact_frame_and_revocation_changes_it() {
        let f = Fixture::new(8, 16384);
        let first = f.cache.read("room".into()).await.unwrap();
        let second = f.cache.read("room".into()).await.unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        f.frame.store(2, Ordering::SeqCst);
        let revoked = f.cache.read("room".into()).await.unwrap();
        assert_eq!(revoked[0]["content"]["membership"], "leave");
        assert!(!Arc::ptr_eq(&first, &revoked));
        assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn matrix_projection_coalesces_concurrent_readers() {
        let f = Fixture::new(8, 16384);
        f.mode.store(1, Ordering::SeqCst);
        let mut readers = Vec::new();
        for _ in 0..8 {
            let cache = f.cache.clone();
            readers.push(tokio::spawn(async move { cache.read("room".into()).await }));
        }
        f.wait_calls(1).await;
        f.gate.add_permits(1);
        let mut values = Vec::new();
        for reader in readers {
            values.push(reader.await.unwrap().unwrap());
        }
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        assert!(values.iter().all(|value| Arc::ptr_eq(value, &values[0])));
    }

    #[tokio::test]
    async fn matrix_projection_cancelled_loader_and_errors_do_not_poison_frame() {
        let f = Fixture::new(8, 16384);
        f.mode.store(1, Ordering::SeqCst);
        let cache = f.cache.clone();
        let leader = tokio::spawn(async move { cache.read("room".into()).await });
        f.wait_calls(1).await;
        let cache = f.cache.clone();
        let waiter = tokio::spawn(async move { cache.read("room".into()).await });
        leader.abort();
        assert!(leader.await.unwrap_err().is_cancelled());
        f.wait_calls(2).await;
        f.gate.add_permits(1);
        assert!(waiter.await.unwrap().is_ok());
        for mode in [2, 3] {
            f.frame.fetch_add(1, Ordering::SeqCst);
            f.mode.store(mode, Ordering::SeqCst);
            assert!(f.cache.read("room".into()).await.is_err());
            let before = f.calls.load(Ordering::SeqCst);
            f.mode.store(0, Ordering::SeqCst);
            assert!(f.cache.read("room".into()).await.is_ok());
            assert_eq!(f.calls.load(Ordering::SeqCst), before + 1);
        }
    }

    #[tokio::test]
    async fn matrix_projection_rechecks_frame_after_load_and_wait() {
        let f = Fixture::new(8, 16384);
        f.mode.store(1, Ordering::SeqCst);
        let cache = f.cache.clone();
        let leader = tokio::spawn(async move { cache.read("room".into()).await });
        f.wait_calls(1).await;
        let cache = f.cache.clone();
        let waiter = tokio::spawn(async move { cache.read("room".into()).await });
        f.frame.store(2, Ordering::SeqCst);
        f.mode.store(0, Ordering::SeqCst);
        f.gate.add_permits(1);
        for result in [leader.await.unwrap(), waiter.await.unwrap()] {
            assert_eq!(result.unwrap()[0]["content"]["membership"], "leave");
        }
        assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn matrix_projection_bounds_cache_and_does_not_evict_inflight_loads() {
        let f = Fixture::new(2, 2048);
        for index in 0..30 {
            f.cache.read(format!("room-{index}")).await.unwrap();
        }
        {
            let slots = f.cache.slots.lock().unwrap();
            assert!(slots.len() <= 2);
            assert!(total_bytes(&slots) <= 2048);
        }
        let tiny = Fixture::new(1, 300);
        assert!(tiny.cache.read("room".into()).await.is_ok());
        assert!(
            tiny.cache.slots.lock().unwrap().values().all(|slot| slot
                .value
                .try_lock()
                .unwrap()
                .is_none())
        );
        let f = Fixture::new(1, 2048);
        f.mode.store(1, Ordering::SeqCst);
        let cache = f.cache.clone();
        let held = tokio::spawn(async move { cache.read("held".into()).await });
        f.wait_calls(1).await;
        assert!(f.cache.read("flood".into()).await.is_err());
        f.gate.add_permits(1);
        assert!(held.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn matrix_projection_rechecks_current_frame_even_for_cached_values() {
        let f = Fixture::new(8, 16384);
        let cached = f.cache.read("room".into()).await.unwrap();
        let slot = f.cache.slot(&("room".into(), 1)).unwrap();
        let held = slot.value.lock().await;
        let cache = f.cache.clone();
        let (started, waiting) = oneshot::channel();
        let reader = tokio::spawn(async move {
            started.send(()).unwrap();
            cache.read("room".into()).await
        });
        waiting.await.unwrap();
        tokio::task::yield_now().await;
        f.frame.store(2, Ordering::SeqCst);
        drop(held);
        let value = reader.await.unwrap().unwrap();
        assert!(!Arc::ptr_eq(&cached, &value));
        assert_eq!(value[0]["content"]["membership"], "leave");
    }

    #[tokio::test]
    async fn matrix_projection_never_falls_back_when_current_frame_is_unavailable() {
        let f = Fixture::new(8, 16384);
        let first = f.cache.read("room".into()).await.unwrap();
        f.mode.store(4, Ordering::SeqCst);
        assert!(f.cache.read("room".into()).await.is_err());
        f.mode.store(0, Ordering::SeqCst);
        let next = f.cache.read("room".into()).await.unwrap();
        assert!(Arc::ptr_eq(&first, &next));
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn matrix_projection_bounds_retries_when_frames_keep_changing() {
        let f = Fixture::new(8, 16384);
        f.mode.store(5, Ordering::SeqCst);
        assert!(matches!(
            f.cache.read("room".into()).await,
            Err(Error::Unavailable("matrix_state_changed"))
        ));
        assert_eq!(f.calls.load(Ordering::SeqCst), 3);
    }
}
