//! Recent room (or thread) text handed to an owned Agent's executing device at
//! execution start, so a request like "summarize this room so far" can work.
//!
//! The timeline is read through the Appservice *as the Agent's puppet*, so the
//! homeserver applies its own history-visibility rules for exactly that user.
//! On top of that, nothing older than the puppet's current membership event is
//! quoted, the triggering request is never quoted, redacted events and edit
//! events are dropped (an edit by the original sender replaces the quoted body,
//! as in shared thread context v2), and the result is bounded by message count
//! and bytes. Encrypted rooms yield no text and an explicit `unavailable` flag.
use crate::{Result, gateway::Gateway, matrix_client::MatrixClient, transport::DeliveryFacts};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

pub const MAX_CONTEXT_MESSAGES: usize = 50;
pub const MAX_CONTEXT_BYTES: usize = 24 * 1024;
pub const MAX_MESSAGE_BYTES: usize = 2 * 1024;
const PAGE_LIMIT: &str = "100";
const MAX_PAGES: usize = 3;
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
pub const UNAVAILABLE_ENCRYPTED: &str = "encrypted";
pub const UNAVAILABLE_UNREADABLE: &str = "unreadable";
const TEXT_MSGTYPES: [&str; 3] = ["m.text", "m.notice", "m.emote"];

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomContextMessage {
    pub event_id: String,
    pub sender: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_display_name: Option<String>,
    pub body: String,
    pub origin_server_ts: i64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub edited: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomContext {
    /// "room" for the main timeline, "thread" for an explicit Matrix thread.
    pub scope: &'static str,
    /// Oldest first; never contains the triggering request.
    pub messages: Vec<RoomContextMessage>,
    /// Older visible messages exist but were not included (budget or page cap).
    pub omitted_older: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<&'static str>,
}
impl RoomContext {
    pub fn unavailable(scope: &'static str, reason: &'static str) -> Self {
        Self {
            scope,
            messages: Vec::new(),
            omitted_older: false,
            unavailable: Some(reason),
        }
    }
}
/// What a dispatch asks to be quoted: an explicit thread root that is not the
/// request itself selects the thread; otherwise the room timeline.
pub fn thread_of<'a>(event_id: &str, room_id: &str, thread_root: &'a str) -> Option<&'a str> {
    (thread_root.starts_with('$') && thread_root != event_id && thread_root != room_id)
        .then_some(thread_root)
}
/// Inputs to the pure selection; `events` is newest first, as Matrix returns
/// a backward page. `complete` is true when the scan reached the oldest event.
pub struct Timeline<'a> {
    pub events: &'a [Value],
    pub trigger: &'a str,
    pub visible_from_ms: i64,
    pub names: &'a BTreeMap<String, String>,
    pub complete: bool,
}
pub fn select(scope: &'static str, t: &Timeline<'_>) -> RoomContext {
    let edits = latest_edits(t.events);
    let Some(start) = t.events.iter().position(|e| e["event_id"] == t.trigger) else {
        // Without the request's position nothing can be proven to precede it.
        return RoomContext {
            scope,
            messages: Vec::new(),
            omitted_older: true,
            unavailable: None,
        };
    };
    let (mut messages, mut seen, mut bytes) = (Vec::new(), HashSet::new(), 0usize);
    let mut omitted_older = !t.complete;
    for event in &t.events[start + 1..] {
        let Some(mut message) = quotable(event, t.visible_from_ms) else {
            continue;
        };
        if !seen.insert(message.event_id.clone()) {
            continue;
        }
        if let Some((_, body)) = edits
            .get(message.event_id.as_str())
            .and_then(|e| e.iter().find(|(sender, _)| *sender == message.sender))
        {
            message.body = clip(&reply_stripped(body));
            message.edited = true;
        }
        message.sender_display_name = t.names.get(&message.sender).cloned();
        let size = message.body.len() + message.sender.len();
        if messages.len() == MAX_CONTEXT_MESSAGES || bytes + size > MAX_CONTEXT_BYTES {
            omitted_older = true;
            break;
        }
        bytes += size;
        messages.push(message);
    }
    messages.reverse();
    RoomContext {
        scope,
        messages,
        omitted_older,
        unavailable: None,
    }
}
fn is_redacted(event: &Value) -> bool {
    !event["unsigned"]["redacted_because"].is_null()
        || event["content"].as_object().is_none_or(|c| c.is_empty())
}
fn quotable(event: &Value, visible_from_ms: i64) -> Option<RoomContextMessage> {
    let content = &event["content"];
    let ts = event["origin_server_ts"].as_i64()?;
    if event["type"] != "m.room.message"
        || is_redacted(event)
        || ts < visible_from_ms
        || content["m.relates_to"]["rel_type"] == "m.replace"
        || !TEXT_MSGTYPES.contains(&content["msgtype"].as_str()?)
    {
        return None;
    }
    let body = reply_stripped(content["body"].as_str()?);
    if body.trim().is_empty() {
        return None;
    }
    Some(RoomContextMessage {
        event_id: event["event_id"].as_str()?.to_owned(),
        sender: event["sender"].as_str()?.to_owned(),
        sender_display_name: None,
        body: clip(&body),
        origin_server_ts: ts,
        edited: false,
    })
}
/// Valid m.replace edits per target as (edit sender, new body), newest first
/// because `events` is; only an edit by the original sender is ever applied.
fn latest_edits(events: &[Value]) -> HashMap<&str, Vec<(String, String)>> {
    let mut edits = HashMap::new();
    for e in events {
        let c = &e["content"];
        if e["type"] != "m.room.message"
            || is_redacted(e)
            || c["m.relates_to"]["rel_type"] != "m.replace"
        {
            continue;
        }
        let (Some(target), Some(sender), Some(body)) = (
            c["m.relates_to"]["event_id"].as_str(),
            e["sender"].as_str(),
            c["m.new_content"]["body"].as_str(),
        ) else {
            continue;
        };
        edits
            .entry(target)
            .or_insert_with(Vec::new)
            .push((sender.to_owned(), body.to_owned()));
    }
    edits
}
/// Matrix reply fallbacks quote the parent as leading "> " lines.
fn reply_stripped(body: &str) -> String {
    let mut lines = body.lines().peekable();
    if lines.peek().is_some_and(|l| l.starts_with("> ")) {
        while lines.peek().is_some_and(|l| l.starts_with('>')) {
            lines.next();
        }
        while lines.peek().is_some_and(|l| l.is_empty()) {
            lines.next();
        }
    }
    lines.collect::<Vec<_>>().join("\n")
}
fn clip(body: &str) -> String {
    if body.len() <= MAX_MESSAGE_BYTES {
        return body.to_owned();
    }
    let mut end = MAX_MESSAGE_BYTES - '…'.len_utf8();
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &body[..end])
}
/// Display names and the puppet's current membership timestamp from state.
pub fn membership(state: &[Value], puppet: &str) -> (BTreeMap<String, String>, Option<i64>) {
    let mut names = BTreeMap::new();
    let mut joined = None;
    for e in state.iter().filter(|e| e["type"] == "m.room.member") {
        let Some(user) = e["state_key"].as_str() else {
            continue;
        };
        if let Some(name) = e["content"]["displayname"]
            .as_str()
            .filter(|n| !n.trim().is_empty())
        {
            names.insert(user.to_owned(), name.to_owned());
        }
        if user == puppet && e["content"]["membership"] == "join" {
            joined = e["origin_server_ts"].as_i64();
        }
    }
    (names, joined)
}
/// Builds the context for one starting execution. Never fails the start: a
/// read failure is reported to the runtime as `unavailable: "unreadable"`.
pub async fn for_dispatch(
    matrix: Option<&MatrixClient>,
    gateway: &Gateway,
    f: &DeliveryFacts,
    event_id: &str,
    thread_root: &str,
) -> RoomContext {
    let thread = thread_of(event_id, &f.room_id, thread_root);
    let scope = if thread.is_some() { "thread" } else { "room" };
    if f.encrypted {
        return RoomContext::unavailable(scope, UNAVAILABLE_ENCRYPTED);
    }
    let Some(matrix) = matrix else {
        return RoomContext::unavailable(scope, UNAVAILABLE_UNREADABLE);
    };
    let read = read_context(matrix, gateway, f, event_id, thread, scope);
    match tokio::time::timeout(FETCH_TIMEOUT, read).await {
        Ok(Ok(context)) => context,
        // Context is advisory: the runtime is told it is unreadable and says
        // so instead of guessing; the request itself still runs.
        Ok(Err(error)) => {
            tracing::warn!(code = %error, "Owned Agent room context unavailable");
            RoomContext::unavailable(scope, UNAVAILABLE_UNREADABLE)
        }
        Err(_) => RoomContext::unavailable(scope, UNAVAILABLE_UNREADABLE),
    }
}
async fn read_context(
    matrix: &MatrixClient,
    gateway: &Gateway,
    f: &DeliveryFacts,
    event_id: &str,
    thread: Option<&str>,
    scope: &'static str,
) -> Result<RoomContext> {
    let (names, joined) = membership(&gateway.state(&f.room_id).await?, &f.puppet_mxid);
    let visible_from_ms = joined.ok_or(crate::Error::Unauthorized("puppet_not_joined"))?;
    let (events, complete) = match thread {
        Some(root) => read_thread(matrix, &f.puppet_mxid, &f.room_id, root).await?,
        None => read_room(matrix, &f.puppet_mxid, &f.room_id, visible_from_ms).await?,
    };
    let timeline = Timeline {
        events: &events,
        trigger: event_id,
        visible_from_ms,
        names: &names,
        complete,
    };
    Ok(select(scope, &timeline))
}
async fn read_room(
    matrix: &MatrixClient,
    puppet: &str,
    room: &str,
    from_ms: i64,
) -> Result<(Vec<Value>, bool)> {
    let (mut events, mut from) = (Vec::new(), None::<String>);
    for _ in 0..MAX_PAGES {
        let mut query = vec![("dir", "b"), ("limit", PAGE_LIMIT)];
        if let Some(token) = from.as_deref() {
            query.push(("from", token));
        }
        let page = matrix
            .read_as(
                puppet,
                &["_matrix", "client", "v3", "rooms", room, "messages"],
                &query,
            )
            .await?;
        let chunk = page["chunk"].as_array().cloned().unwrap_or_default();
        let older_than_join = chunk.iter().any(|e| {
            e["origin_server_ts"]
                .as_i64()
                .is_some_and(|ts| ts < from_ms)
        });
        let next = page["end"].as_str().map(str::to_owned);
        let done = chunk.is_empty() || older_than_join || next.is_none() || next == from;
        events.extend(chunk);
        if done {
            return Ok((events, true));
        }
        from = next;
    }
    Ok((events, false))
}
async fn read_thread(
    matrix: &MatrixClient,
    puppet: &str,
    room: &str,
    root: &str,
) -> Result<(Vec<Value>, bool)> {
    let (mut events, mut from) = (Vec::new(), None::<String>);
    let path = [
        "_matrix",
        "client",
        "v1",
        "rooms",
        room,
        "relations",
        root,
        "m.thread",
    ];
    let mut complete = false;
    for _ in 0..MAX_PAGES {
        let mut query = vec![("dir", "b"), ("limit", PAGE_LIMIT)];
        if let Some(token) = from.as_deref() {
            query.push(("from", token));
        }
        let page = matrix.read_as(puppet, &path, &query).await?;
        events.extend(page["chunk"].as_array().cloned().unwrap_or_default());
        from = page["next_batch"].as_str().map(str::to_owned);
        if from.is_none() {
            complete = true;
            break;
        }
    }
    if complete {
        events.push(
            matrix
                .read_as(
                    puppet,
                    &["_matrix", "client", "v3", "rooms", room, "event", root],
                    &[],
                )
                .await?,
        );
    }
    Ok((events, complete))
}
