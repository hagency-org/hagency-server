//! Rich Matrix replies retain the original Markdown body and exact relations.
use palpo_core::events::room::message::FormattedBody;
use palpo_core::html::{HtmlSanitizerMode, RemoveReplyFallback};
use serde_json::Value;

pub(crate) fn add_markdown(content: &mut Value) {
    let Some(body) = content["body"].as_str() else {
        return;
    };
    let Some(mut formatted) = FormattedBody::markdown(body) else {
        return;
    };
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
        tracing::warn!(
            "Agent rich reply exceeds Matrix envelope; retaining complete Markdown body"
        );
    }
}

/// This is a content budget, not a proof about the homeserver's signed PDU.
/// Leave envelope space and reject before committing an immutable send intent.
pub(crate) fn validate_content(content: &Value) -> crate::Result<()> {
    if serde_json::to_vec(content)
        .map_err(|_| crate::Error::Invalid("invalid_reply"))?
        .len()
        > 60 * 1024
    {
        return Err(crate::Error::Invalid("matrix_message_too_large"));
    }
    Ok(())
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
        for tag in [
            "<h2>",
            "<strong>",
            "<ul>",
            "<table>",
            "<pre>",
            "<a href=\"https://example.org/",
        ] {
            assert!(html.contains(tag), "missing {tag}: {html}");
        }
        assert!(!html.contains("<script") && !html.contains("href=\"javascript:"));
    }

    #[test]
    fn plaintext_reply_budget_counts_json_escaping_and_relations() {
        let content = reply_content(false, "!room:x", "$thread", &"x".repeat(64 * 1024));
        assert!(super::validate_content(&content).is_err());
        let escaped = reply_content(false, "!room:x", "$thread", &"\t".repeat(32 * 1024));
        assert!(super::validate_content(&escaped).is_err());
        assert!(
            super::validate_content(&reply_content(false, "!room:x", "$thread", "answer")).is_ok()
        );
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
