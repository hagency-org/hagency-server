//! Rich Matrix replies retain the original Markdown body and exact relations.
use palpo_core::{events::room::message::FormattedBody, html::{HtmlSanitizerMode, RemoveReplyFallback}};
use serde_json::Value;

pub(crate) fn add_markdown(content: &mut Value) {
    let Some(body) = content["body"].as_str() else { return; };
    let Some(mut formatted) = FormattedBody::markdown(body) else { return; };
    formatted.sanitize_html(HtmlSanitizerMode::Strict, RemoveReplyFallback::No);
    content["format"] = Value::String("org.matrix.custom.html".into());
    content["formatted_body"] = Value::String(formatted.body);
    // Matrix events have a 64 KiB envelope. Keep room for event metadata and
    // never turn a completed, settled answer into a failed delivery just because
    // a second representation exceeds the envelope. Desktop also renders legacy
    // unformatted Agent Markdown, including this bounded plain-text case.
    if serde_json::to_vec(content).is_ok_and(|v| v.len() > 60 * 1024) {
        content.as_object_mut().unwrap().remove("format");
        content.as_object_mut().unwrap().remove("formatted_body");
        tracing::warn!("Agent rich reply exceeds Matrix envelope; retaining complete Markdown body");
    }
}

#[cfg(test)]
mod tests {
    use crate::workers::reply_content;

    #[test]
    fn agent_markdown_reply_preserves_body_and_thread_with_safe_rich_text() {
        let body = "## 调研\n\n**结论**\n\n- 第一条\n\n| 型号 | 显存 |\n|---|---|\n| RTX | 96GB |\n\n```rust\nlet x = 1 < 2;\n```\n\n[来源](https://example.org/a?q=1&b=2)\n<script>alert(1)</script>\n[bad](javascript:alert)";
        let c = reply_content(false, "!room:x", "$thread", body);
        assert_eq!(c["body"], body);
        assert_eq!(c["m.relates_to"]["event_id"], "$thread");
        assert_eq!(c["format"], "org.matrix.custom.html");
        let html = c["formatted_body"].as_str().unwrap();
        for tag in ["<h2>", "<strong>", "<ul>", "<table>", "<pre>", "<a href=\"https://example.org/"] { assert!(html.contains(tag), "missing {tag}: {html}"); }
        assert!(!html.contains("<script") && !html.contains("href=\"javascript:"));
    }

    #[test]
    fn rich_reply_capacity_preserves_complete_plain_body() {
        let body = "**<&>**\n".repeat(4000);
        let c = reply_content(false, "!room:x", "$thread", &body);
        assert_eq!(c["body"], body);
        assert!(c.get("formatted_body").is_none());
        assert!(serde_json::to_vec(&c).unwrap().len() < 64 * 1024);
        assert_eq!(c["m.relates_to"]["event_id"], "$thread");
    }
}
