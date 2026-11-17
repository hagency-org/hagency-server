//! Only for a freshly issued OAuth token whose Matrix identity is provisioning.
//! Callers supply their platform's cancellable timer and an overall deadline.
use std::future::Future;

pub const DEADLINE_MS: u32 = 8_000;
const RETRY_DELAYS_MS: [u32; 8] = [100, 200, 400, 800, 1_000, 1_000, 1_000, 1_000];

pub async fn wait<R, A, F, P, S, W>(mut attempt: A, retry: P, mut sleep: S) -> R
where
    A: FnMut() -> F,
    F: Future<Output = R>,
    P: Fn(&R) -> bool,
    S: FnMut(u32) -> W,
    W: Future<Output = ()>,
{
    let mut result = attempt().await;
    for delay in RETRY_DELAYS_MS {
        if !retry(&result) {
            return result;
        }
        sleep(delay).await;
        result = attempt().await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        future::{pending, ready},
        pin::pin,
        task::{Context, Poll, Waker},
    };

    fn complete<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("test uses immediately ready operations"),
        }
    }

    #[test]
    fn transient_unauthorized_rechecks_the_same_token_without_another_exchange() {
        let exchanges = Cell::new(0);
        let calls = RefCell::new(Vec::new());
        let delays = RefCell::new(Vec::new());
        let exchange = || {
            exchanges.set(exchanges.get() + 1);
            "issued-token"
        };
        let token = exchange();
        let result = complete(wait(
            || {
                calls.borrow_mut().push(token);
                ready(if calls.borrow().len() < 4 { 401 } else { 200 })
            },
            |status| *status == 401,
            |delay| {
                delays.borrow_mut().push(delay);
                ready(())
            },
        ));
        assert_eq!(result, 200);
        assert_eq!(exchanges.get(), 1);
        assert_eq!(*calls.borrow(), vec!["issued-token"; 4]);
        assert_eq!(*delays.borrow(), vec![100, 200, 400]);
    }

    #[test]
    fn permanent_unauthorized_has_bounded_attempts_and_other_failures_stop_immediately() {
        for status in [401, 403, 429, 500, 502] {
            let calls = Cell::new(0);
            let delay = Cell::new(0);
            assert_eq!(
                complete(wait(
                    || {
                        calls.set(calls.get() + 1);
                        ready(status)
                    },
                    |status| *status == 401,
                    |ms| {
                        delay.set(delay.get() + ms);
                        ready(())
                    },
                )),
                status
            );
            assert_eq!(calls.get(), if status == 401 { 9 } else { 1 });
            assert_eq!(delay.get(), if status == 401 { 5_500 } else { 0 });
        }
    }

    #[test]
    fn cancellation_drops_the_wait_without_another_identity_request() {
        let calls = Cell::new(0);
        let mut future = Box::pin(wait(
            || {
                calls.set(calls.get() + 1);
                ready(401)
            },
            |status| *status == 401,
            |_| pending(),
        ));
        assert!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        drop(future);
        assert_eq!(calls.get(), 1);
    }
}
