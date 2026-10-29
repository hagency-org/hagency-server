//! In-memory session fencing and one refresh per rejected access token.
//! Kept independent of browser APIs so the actual asynchronous coordinator is testable.
use futures::{
    FutureExt,
    future::{LocalBoxFuture, Shared},
};
use std::{cell::RefCell, future::Future, rc::Rc};

#[derive(Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub generation: u64,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    Retry,
    Expired(u64),
    Superseded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorCheck {
    Current,
    Changed(u64),
    Superseded,
}

type Refresh = Shared<LocalBoxFuture<'static, Recovery>>;
struct Inner {
    generation: u64,
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_at: Option<String>,
    flight: Option<(Snapshot, Refresh)>,
    allow_browser_session: bool,
    actor: Option<String>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            generation: 0,
            access_token: None,
            refresh_token: None,
            expires_at: None,
            flight: None,
            allow_browser_session: true,
            actor: None,
        }
    }
}

#[derive(Clone, Default)]
pub struct SessionState(Rc<RefCell<Inner>>);

impl SessionState {
    pub fn snapshot(&self) -> Snapshot {
        let inner = self.0.borrow();
        Snapshot {
            generation: inner.generation,
            access_token: inner.access_token.clone(),
            refresh_token: inner.refresh_token.clone(),
        }
    }

    pub fn generation(&self) -> u64 {
        self.0.borrow().generation
    }

    pub fn allows_browser_session(&self) -> bool {
        self.0.borrow().allow_browser_session
    }

    pub fn observe_actor(&self, generation: u64, observed: &str) -> ActorCheck {
        let original = self.0.borrow().actor.clone();
        match original {
            Some(original) => self.check_actor(generation, &original, observed),
            None if self.generation() == generation => {
                self.0.borrow_mut().actor = Some(observed.into());
                ActorCheck::Current
            }
            None => ActorCheck::Superseded,
        }
    }

    pub fn check_actor(&self, generation: u64, original: &str, observed: &str) -> ActorCheck {
        if self.generation() != generation {
            return ActorCheck::Superseded;
        }
        if original == observed {
            return ActorCheck::Current;
        }
        self.clear();
        ActorCheck::Changed(self.generation())
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let inner = self.0.borrow();
        match key {
            "access_token" => inner.access_token.clone(),
            "refresh_token" => inner.refresh_token.clone(),
            "access_token_expires_at" => inner.expires_at.clone(),
            _ => None,
        }
    }

    pub fn set(&self, key: &str, value: &str) {
        if key == "access_token" {
            self.clear();
            let mut inner = self.0.borrow_mut();
            inner.access_token = Some(value.into());
            inner.allow_browser_session = true;
        } else {
            let mut inner = self.0.borrow_mut();
            match key {
                "refresh_token" => inner.refresh_token = Some(value.into()),
                "access_token_expires_at" => inner.expires_at = Some(value.into()),
                _ => (),
            }
        }
    }

    pub fn remove(&self, key: &str) {
        if key == "access_token" {
            self.clear();
        } else {
            let mut inner = self.0.borrow_mut();
            match key {
                "refresh_token" => inner.refresh_token = None,
                "access_token_expires_at" => inner.expires_at = None,
                _ => (),
            }
        }
    }

    pub fn clear(&self) {
        let mut inner = self.0.borrow_mut();
        inner.generation = inner.generation.wrapping_add(1);
        inner.access_token = None;
        inner.refresh_token = None;
        inner.expires_at = None;
        inner.flight = None;
        inner.allow_browser_session = false;
        inner.actor = None;
    }

    pub fn expire(&self, rejected: &Snapshot) -> Recovery {
        let now = self.snapshot();
        if now.generation != rejected.generation {
            return Recovery::Superseded;
        }
        if now.access_token != rejected.access_token {
            return Recovery::Retry;
        }
        self.clear();
        Recovery::Expired(self.generation())
    }

    pub async fn recover<F, Fut>(&self, rejected: Snapshot, refresh: F) -> Recovery
    where
        F: FnOnce(String) -> Fut + 'static,
        Fut: Future<Output = Result<Tokens, ()>> + 'static,
    {
        let now = self.snapshot();
        if now.generation != rejected.generation {
            return Recovery::Superseded;
        }
        if now.access_token != rejected.access_token {
            return Recovery::Retry;
        }
        let existing = self
            .0
            .borrow()
            .flight
            .as_ref()
            .filter(|(s, _)| s == &now)
            .map(|(_, f)| f.clone());
        let flight = if let Some(existing) = existing {
            existing
        } else {
            let weak = Rc::downgrade(&self.0);
            let original = now.clone();
            let future = async move {
                let result = match original.refresh_token.clone() {
                    Some(token) => refresh(token).await,
                    None => Err(()),
                };
                let Some(inner) = weak.upgrade() else {
                    return Recovery::Superseded;
                };
                let state = SessionState(inner);
                let current = state.snapshot();
                if current.generation != original.generation {
                    return Recovery::Superseded;
                }
                if current.access_token != original.access_token {
                    return Recovery::Retry;
                }
                state.0.borrow_mut().flight = None;
                match result {
                    Ok(tokens) if !tokens.access_token.is_empty() => {
                        let mut inner = state.0.borrow_mut();
                        inner.access_token = Some(tokens.access_token);
                        if let Some(token) = tokens.refresh_token {
                            inner.refresh_token = Some(token);
                        }
                        Recovery::Retry
                    }
                    _ => state.expire(&original),
                }
            }
            .boxed_local()
            .shared();
            self.0.borrow_mut().flight = Some((now, future.clone()));
            future
        };
        flight.await
    }

    /// Every API family uses this path: fence responses, coalesce 401 refreshes,
    /// retry once with the current token, and expire only that exact session.
    pub async fn request<T, E, Send, Sent, RefreshFn, Refreshed, Unauthorized, Stale, Expired>(
        &self,
        mut send: Send,
        refresh: RefreshFn,
        unauthorized: Unauthorized,
        stale: Stale,
        expired: Expired,
    ) -> Result<T, E>
    where
        Send: FnMut(Snapshot) -> Sent,
        Sent: Future<Output = Result<T, E>>,
        RefreshFn: FnOnce(String) -> Refreshed + 'static,
        Refreshed: Future<Output = Result<Tokens, ()>> + 'static,
        Unauthorized: Fn(&E) -> bool,
        Stale: Fn() -> E,
        Expired: Fn(u64),
    {
        let original = self.snapshot();
        let result = send(original.clone()).await;
        if self.generation() != original.generation {
            return Err(stale());
        }
        if !result.as_ref().is_err_and(&unauthorized) {
            return result;
        }
        match self.recover(original.clone(), refresh).await {
            Recovery::Retry if self.generation() == original.generation => {
                let retried = self.snapshot();
                let result = send(retried.clone()).await;
                if self.generation() != original.generation {
                    return Err(stale());
                }
                if result.as_ref().is_err_and(&unauthorized)
                    && let Recovery::Expired(generation) = self.expire(&retried)
                {
                    expired(generation);
                }
                result
            }
            Recovery::Expired(generation) => {
                expired(generation);
                result
            }
            _ => Err(stale()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{channel::oneshot, executor::LocalPool, task::LocalSpawnExt};
    use std::cell::Cell;

    fn session() -> SessionState {
        let state = SessionState::default();
        state.set("access_token", "old");
        state.set("refresh_token", "refresh-old");
        state
    }
    fn tokens() -> Tokens {
        Tokens {
            access_token: "new".into(),
            refresh_token: Some("refresh-new".into()),
        }
    }
    fn unauthorized(error: &u16) -> bool {
        *error == 401
    }
    fn stale() -> u16 {
        409
    }

    #[test]
    fn matrix_pasion_and_bff_401s_join_one_refresh_and_retry() {
        let state = session();
        let mut pool = LocalPool::new();
        let results = Rc::new(RefCell::new(Vec::new()));
        let starts = Rc::new(Cell::new(0));
        let (tx, rx) = oneshot::channel();
        let reply = Rc::new(RefCell::new(Some(rx)));
        for family in ["matrix", "pasion", "bff"] {
            let (state, results, starts, reply) = (
                state.clone(),
                results.clone(),
                starts.clone(),
                reply.clone(),
            );
            pool.spawner()
                .spawn_local(async move {
                    let result = state
                        .request(
                            |snapshot| async move {
                                if snapshot.access_token.as_deref() == Some("old") {
                                    Err(401)
                                } else {
                                    Ok(family)
                                }
                            },
                            move |token| {
                                assert_eq!(token, "refresh-old");
                                starts.set(starts.get() + 1);
                                let rx = reply.borrow_mut().take().expect("only one refresh");
                                async move { rx.await.unwrap() }
                            },
                            unauthorized,
                            stale,
                            |_| panic!("must not expire"),
                        )
                        .await;
                    results.borrow_mut().push(result);
                })
                .unwrap();
        }
        pool.run_until_stalled();
        assert_eq!(starts.get(), 1);
        assert!(results.borrow().is_empty());
        assert!(tx.send(Ok(tokens())).is_ok());
        pool.run();
        assert_eq!(results.borrow().len(), 3);
        assert!(results.borrow().iter().all(Result::is_ok));
        assert_eq!(state.get("access_token").as_deref(), Some("new"));
        assert_eq!(state.get("refresh_token").as_deref(), Some("refresh-new"));
    }

    #[test]
    fn late_old_token_401_reuses_new_token_without_refreshing_again() {
        let state = session();
        let mut pool = LocalPool::new();
        let (tx, rx) = oneshot::channel();
        let rx = Rc::new(RefCell::new(Some(rx)));
        let result = Rc::new(RefCell::new(None));
        let (running, observed) = (state.clone(), result.clone());
        pool.spawner()
            .spawn_local(async move {
                let answer = running
                    .request(
                        move |snapshot| {
                            let reply = rx.borrow_mut().take();
                            async move {
                                match reply {
                                    Some(reply) => reply.await.unwrap(),
                                    None => Ok(snapshot.access_token.unwrap()),
                                }
                            }
                        },
                        |_| async { panic!("late 401 must not rotate again") },
                        unauthorized,
                        stale,
                        |_| panic!("must not expire"),
                    )
                    .await;
                *observed.borrow_mut() = Some(answer);
            })
            .unwrap();
        pool.run_until_stalled();
        let refreshed = pool.run_until(state.request(
            |snapshot| async move {
                if snapshot.access_token.as_deref() == Some("old") {
                    Err(401)
                } else {
                    Ok(())
                }
            },
            |_| async { Ok(tokens()) },
            unauthorized,
            stale,
            |_| panic!("must not expire"),
        ));
        assert_eq!(refreshed, Ok(()));
        tx.send(Err(401)).unwrap();
        pool.run();
        assert_eq!(*result.borrow(), Some(Ok("new".into())));
    }

    #[test]
    fn logout_and_account_switch_fence_both_successful_and_failed_old_refreshes() {
        for success in [true, false] {
            for next_account in [None, Some("other-account")] {
                let state = session();
                let mut pool = LocalPool::new();
                let (tx, rx) = oneshot::channel();
                let rx = Rc::new(RefCell::new(Some(rx)));
                let result = Rc::new(RefCell::new(None));
                let (running, observed) = (state.clone(), result.clone());
                pool.spawner()
                    .spawn_local(async move {
                        *observed.borrow_mut() = Some(
                            running
                                .request(
                                    |_| async { Err::<(), _>(401) },
                                    move |_| {
                                        let rx = rx.borrow_mut().take().unwrap();
                                        async move { rx.await.unwrap() }
                                    },
                                    unauthorized,
                                    stale,
                                    |_| panic!("old failure must not expire new account"),
                                )
                                .await,
                        );
                    })
                    .unwrap();
                pool.run_until_stalled();
                state.clear();
                assert!(state.get("access_token").is_none());
                assert!(!state.allows_browser_session());
                if let Some(account) = next_account {
                    state.set("access_token", account);
                    state.set("refresh_token", "other-refresh");
                }
                assert!(
                    tx.send(if success { Ok(tokens()) } else { Err(()) })
                        .is_ok()
                );
                pool.run();
                assert_eq!(*result.borrow(), Some(Err(409)));
                assert_eq!(state.get("access_token").as_deref(), next_account);
                assert_eq!(
                    state.get("refresh_token").as_deref(),
                    next_account.map(|_| "other-refresh")
                );
            }
        }
    }

    #[test]
    fn cancelling_first_waiter_does_not_strand_other_refresh_waiters() {
        let state = session();
        let mut pool = LocalPool::new();
        let (tx, rx) = oneshot::channel();
        let first = state.recover(state.snapshot(), move |_| async move { rx.await.unwrap() });
        let mut first = Box::pin(first);
        assert!(first.as_mut().now_or_never().is_none());
        let second = state.recover(state.snapshot(), |_| async { panic!("must join existing") });
        drop(first);
        assert!(tx.send(Ok(tokens())).is_ok());
        assert_eq!(pool.run_until(second), Recovery::Retry);
    }

    #[test]
    fn a_second_401_expires_only_the_retried_session() {
        let state = session();
        let expired = Rc::new(Cell::new(None));
        let seen = expired.clone();
        let result = futures::executor::block_on(state.request(
            |_| async { Err::<(), _>(401) },
            |_| async { Ok(tokens()) },
            unauthorized,
            stale,
            move |generation| seen.set(Some(generation)),
        ));
        assert_eq!(result, Err(401));
        assert_eq!(expired.get(), Some(state.generation()));
        assert!(state.get("access_token").is_none());
        assert!(state.get("refresh_token").is_none());
    }

    #[test]
    fn actor_pin_rejects_shared_cookie_account_switch_and_retires_only_current_tab() {
        let state = session();
        let generation = state.generation();
        assert_eq!(
            state.observe_actor(generation, "@alice:test"),
            ActorCheck::Current
        );
        // A different tab's shared localStorage identity is deliberately not an
        // input to this decision: only the original verified in-memory actor is.
        let shared_identity = "@bob:test";
        assert_eq!(
            state.check_actor(generation, "@alice:test", shared_identity),
            ActorCheck::Changed(generation + 1)
        );
        assert!(state.get("access_token").is_none());
        assert!(!state.allows_browser_session());
    }

    #[test]
    fn actor_pin_keeps_same_actor_forbidden_and_ignores_stale_probes_after_login() {
        let state = session();
        let generation = state.generation();
        assert_eq!(
            state.observe_actor(generation, "@alice:test"),
            ActorCheck::Current
        );
        assert_eq!(
            state.check_actor(generation, "@alice:test", "@alice:test"),
            ActorCheck::Current
        );
        assert_eq!(state.get("access_token").as_deref(), Some("old"));
        state.set("access_token", "new-account");
        assert_eq!(
            state.check_actor(generation, "@alice:test", "@bob:test"),
            ActorCheck::Superseded
        );
        assert_eq!(state.get("access_token").as_deref(), Some("new-account"));
    }

    #[test]
    fn background_session_probe_cannot_replace_the_pinned_actor() {
        let state = session();
        let generation = state.generation();
        assert_eq!(
            state.observe_actor(generation, "@alice:test"),
            ActorCheck::Current
        );
        assert_eq!(
            state.observe_actor(generation, "@bob:test"),
            ActorCheck::Changed(generation + 1)
        );
        assert!(state.get("access_token").is_none());
    }
}
