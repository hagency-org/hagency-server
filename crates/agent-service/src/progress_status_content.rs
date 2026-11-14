//! Where the live status sits. A top-level request gets a top-level notice that
//! replies to it, so the main timeline shows the work as it happens (owner
//! report 2026-11-13: a status threaded under the request was only a "1 reply"
//! summary in the desktop). A request inside a real Matrix thread keeps its
//! status in that thread. The final reply's placement is unchanged.
use super::{ProgressStatus, body_of};
use serde_json::{Value, json};

/// The first post's content, before any edit wrapping.
pub(super) fn fresh_content(owner_direct: bool, s: &ProgressStatus, body: &str) -> Value {
    if crate::room_context::thread_of(&s.event_id, &s.room_id, &s.thread_root).is_some() {
        let mut content =
            crate::workers::reply_content(owner_direct, &s.room_id, &s.thread_root, body);
        content["msgtype"] = json!("m.notice");
        return content;
    }
    let mut content = json!({"msgtype":"m.notice","body":body,
        "m.relates_to":{"m.in_reply_to":{"event_id":s.event_id}}});
    crate::reply_format::add_markdown(&mut content);
    content
}

/// First post: the notice from [`fresh_content`]. Later: an `m.replace` edit of it.
pub(crate) fn status_content(owner_direct: bool, s: &ProgressStatus) -> (String, Value) {
    let body = body_of(s);
    let mut fresh = fresh_content(owner_direct, s, &body);
    let Some(original) = &s.status_event_id else {
        return (s.matrix_txn_id.clone(), fresh);
    };
    if let Some(map) = fresh.as_object_mut() {
        map.remove("m.relates_to");
    }
    let edit = json!({"msgtype":"m.notice","body":format!("* {body}"),"m.new_content":fresh,
        "m.relates_to":{"rel_type":"m.replace","event_id":original}});
    (format!("{}-r{}", s.matrix_txn_id, s.revision), edit)
}

#[cfg(test)]
#[path = "progress_status_content_tests.rs"]
mod tests;
