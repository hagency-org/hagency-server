//! Exercise encoded Matrix routes with the host's resolved Salvo dependency graph.
use salvo::prelude::*;
use salvo::test::{ResponseExt, TestClient};

#[handler]
fn capture_user(req: &mut Request) -> String {
    format!(
        "{}|{}",
        req.param::<String>("user").unwrap(),
        req.uri().path_and_query().unwrap()
    )
}

#[tokio::test]
async fn matrix_path_parameters_decode_once_and_preserve_request_target() {
    let service =
        Service::new(Router::with_path("_matrix/client/v3/profile/{user}").get(capture_user));
    for (encoded, expected) in [
        ("%40a%2Fb%3Aexample.org", "@a/b:example.org"),
        ("%40a%252Fb%3Aexample.org", "@a%2Fb:example.org"),
    ] {
        let path = format!("/_matrix/client/v3/profile/{encoded}?probe=%2F");
        let mut response = TestClient::get(format!("http://localhost{path}"))
            .send(&service)
            .await;
        assert_eq!(response.status_code, Some(StatusCode::OK));
        assert_eq!(
            response.take_string().await.unwrap(),
            format!("{expected}|{path}")
        );
    }

    let response = TestClient::get("http://localhost/_matrix/client/v3/profile/@a/b:example.org")
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::NOT_FOUND));
}
