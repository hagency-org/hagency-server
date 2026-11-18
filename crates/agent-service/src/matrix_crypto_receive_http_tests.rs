use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

/// Stream the real response over HTTP; assertions exercise crypto_json's chunk
/// bound as well as projection, not a preconstructed Value passed to a helper.
async fn response_server(value: Value) -> (MatrixClient, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let bytes = serde_json::to_vec(&value).unwrap();
    let worker = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 8192];
        loop {
            let n = socket.read(&mut buffer).await.unwrap();
            assert_ne!(n, 0, "request headers must arrive");
            request.extend_from_slice(&buffer[..n]);
            if request.windows(4).any(|v| v == b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8(request).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        );
        socket.write_all(headers.as_bytes()).await.unwrap();
        for chunk in bytes.chunks(16 * 1024) {
            if socket.write_all(chunk).await.is_err() {
                break;
            }
        }
        request
    });
    (
        MatrixClient::new(
            url::Url::parse(&origin).unwrap(),
            "test-as-token".into(),
            "example.test".into(),
        )
        .unwrap(),
        worker,
    )
}
fn response(padding: usize) -> Value {
    json!({"next_batch":"sync-after-large","to_device":{"events":[{
        "sender":"@owner:example.test","type":"m.room.encrypted",
        "content":{"algorithm":"m.olm.v1.curve25519-aes-sha2","sender_key":"sender-key","ciphertext":{"recipient-key":{"type":0,"body":"x".repeat(padding)}}}
    }]},"device_one_time_keys_count":{"signed_curve25519":3}})
}
fn assert_fixed_request(request: &str) {
    let mut line = request.lines().next().unwrap().split_whitespace();
    assert_eq!(line.next(), Some("GET"));
    let url = url::Url::parse(&format!("http://example.test{}", line.next().unwrap())).unwrap();
    assert_eq!(url.path(), "/_matrix/client/v3/sync");
    let query: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["since"], "sync-before-large");
    assert_eq!(query["timeout"], "0");
    assert_eq!(query["set_presence"], "offline");
    assert_eq!(query["user_id"], "@_hagency_agent:example.test");
    assert_eq!(query["device_id"], "HAGE2EERECEIVE");
    assert_eq!(
        query["filter"],
        crate::domain::agent_crypto::receive::FILTER
    );
}
#[tokio::test]
async fn receive_sync_delivers_a_single_large_message_with_its_original_cursor() {
    let expected = response(1_572_864);
    let (matrix, server) = response_server(expected.clone()).await;
    let (status, received) = matrix
        .crypto_identity_request(
            "@_hagency_agent:example.test",
            "HAGE2EERECEIVE",
            Operation::ReceiveSync,
            json!({"since":"sync-before-large"}),
            crate::api::now_ms() + 10_000,
        )
        .await
        .unwrap();
    assert_eq!(status, 200);
    assert_eq!(
        received, expected,
        "no message content or next_batch is truncated or rewritten"
    );
    assert_fixed_request(&server.await.unwrap());
}
#[tokio::test]
async fn receive_sync_delivers_numeric_content_expanded_by_json_normalization() {
    // Match the upstream's accepted sendToDevice body, then its Value roundtrip.
    // These numbers are accepted by the reviewed Palpo handler; this is not a
    // claim that canonical signing JSON requires support for float literals.
    let numbers = "1e15,".repeat(599_999) + "1e15";
    let compact = format!(
        r#"{{"messages":{{"@_hagency_agent:example.test":{{"HAGE2EERECEIVE":{{"negative_zero":-0,"values":[{numbers}]}}}}}}}}"#
    );
    assert!(
        compact.len() < 8 * 1024 * 1024,
        "upstream ingress permits this body"
    );
    let mut accepted: Value = serde_json::from_str(&compact).unwrap();
    let content = accepted["messages"]["@_hagency_agent:example.test"]["HAGE2EERECEIVE"].take();
    let expected = json!({
        "next_batch":"sync-after-large",
        "to_device":{"events":[{
            "sender":"@owner:example.test",
            "type":"org.hagency.normalization_probe",
            "content":content
        }]},
        "device_one_time_keys_count":{"signed_curve25519":3}
    });
    let normalized_len = serde_json::to_vec(&expected).unwrap().len();
    assert!(
        normalized_len > 9 * 1024 * 1024,
        "the previous bound stalled this mailbox"
    );
    assert!(normalized_len <= crate::domain::agent_crypto::receive::MAX_RESPONSE_BYTES);
    let (matrix, server) = response_server(expected.clone()).await;
    let (status, received) = matrix
        .crypto_identity_request(
            "@_hagency_agent:example.test",
            "HAGE2EERECEIVE",
            Operation::ReceiveSync,
            json!({"since":"sync-before-large"}),
            crate::api::now_ms() + 10_000,
        )
        .await
        .unwrap();
    assert_eq!(status, 200);
    assert_eq!(
        received, expected,
        "normalized content and cursor must remain complete"
    );
    assert_fixed_request(&server.await.unwrap());
}
#[tokio::test]
async fn receive_sync_rejects_an_oversized_stream_without_exposing_an_ack_cursor() {
    let (matrix, server) = response_server(response(
        crate::domain::agent_crypto::receive::MAX_RESPONSE_BYTES,
    ))
    .await;
    let result = matrix
        .crypto_identity_request(
            "@_hagency_agent:example.test",
            "HAGE2EERECEIVE",
            Operation::ReceiveSync,
            json!({"since":"sync-before-large"}),
            crate::api::now_ms() + 10_000,
        )
        .await;
    assert!(
        matches!(
            result,
            Err(Error::Unavailable("agent_crypto_response_too_large"))
        ),
        "the entire response fails, so no partial message or cursor can be returned"
    );
    // The one HTTP request echoes only the caller's previous cursor. This read
    // performs no follow-up sync with the unseen response's next_batch.
    assert_fixed_request(&server.await.unwrap());
}
