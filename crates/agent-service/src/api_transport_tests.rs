use super::start_with_context;
use crate::{Error, room_context::RoomContext};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[tokio::test]
async fn caller_timeout_during_context_read_leaves_execution_uncommitted() {
    let committed = AtomicBool::new(false);
    let start = async {
        committed.store(true, Ordering::SeqCst);
        Err(Error::Conflict("unreachable"))
    };
    // The caller gives up while room context is still being read.
    let call = start_with_context(std::future::pending::<RoomContext>(), start);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), call)
            .await
            .is_err()
    );
    assert!(
        !committed.load(Ordering::SeqCst),
        "start must not commit before context is in hand, or a retry is unrecoverable"
    );
}

#[tokio::test]
async fn start_commits_after_context_and_surfaces_its_error() {
    let committed = AtomicBool::new(false);
    let start = async {
        committed.store(true, Ordering::SeqCst);
        Err(Error::Conflict("execution_not_startable"))
    };
    let context = async { RoomContext::unavailable("room", "unreadable") };
    let result = start_with_context(context, start).await;
    assert!(matches!(
        result,
        Err(Error::Conflict("execution_not_startable"))
    ));
    assert!(committed.load(Ordering::SeqCst));
}
