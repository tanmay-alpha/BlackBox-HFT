//! Regression coverage for HTTP cache validators and rejected query strings.
mod common;

use bb_db::Scope;
use common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anonymous_cache_hits_honor_conditional_requests() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let (_, key) = workspace_key(&server.db, "demo", &[Scope::Read]).await;
    for caller in [None, Some(key.as_str())] {
        let path = "/api/v1/datasets";
        let (status, headers, _) = server.get(path, caller).await;
        assert_eq!(status, 200);
        let etag = headers["etag"].to_str().unwrap();
        for validator in [
            etag.to_string(),
            format!("W/{etag}"),
            format!("\"different\", W/{etag}"),
            "*".into(),
        ] {
            let mut request = server
                .http
                .get(server.url(path))
                .header("if-none-match", &validator);
            if let Some(key) = caller {
                request = request.bearer_auth(key);
            }
            let response = request.send().await.unwrap();
            assert_eq!(
                response.status(),
                304,
                "caller keyed={} validator={validator}",
                caller.is_some()
            );
            assert_eq!(response.headers()["etag"], etag);
            assert!(response.headers().contains_key("cache-control"));
            let vary: Vec<_> = response
                .headers()
                .get_all("vary")
                .iter()
                .map(|v| v.to_str().unwrap())
                .collect();
            assert!(vary.iter().any(|v| v.contains("Authorization")));
            assert!(vary.iter().any(|v| v.contains("Origin")));
            assert!(response.bytes().await.unwrap().is_empty());
        }
        let mut request = server
            .http
            .get(server.url(path))
            .header("if-none-match", "\"different\"");
        if let Some(key) = caller {
            request = request.bearer_auth(key);
        }
        assert_eq!(request.send().await.unwrap().status(), 200);
    }
    let (_, private_key) = workspace_key(&server.db, "cache-private", &[Scope::Read]).await;
    for caller in [None, Some(private_key.as_str()), Some(key.as_str()), None] {
        let (status, _, identity) = server.get("/api/v1/whoami", caller).await;
        assert_eq!(status, 200);
        assert_eq!(identity["key"].is_null(), caller.is_none());
        assert_eq!(
            identity["workspace"]["slug"],
            if caller == Some(private_key.as_str()) {
                "cache-private"
            } else {
                "demo"
            }
        );
    }
    server.stop().await;
    tdb.drop_db().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_query_values_use_the_error_envelope() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let (_, key) = workspace_key(&server.db, "demo", &[Scope::Read]).await;
    for caller in [None, Some(key.as_str())] {
        for path in [
            "/api/v1/datasets?limit=99999999999999999999999999999999",
            "/api/v1/datasets?limit=%E2%98%83",
            "/api/v1/datasets?limit=-1",
            "/api/v1/datasets?limit=0",
            "/api/v1/datasets?cursor=%E2%98%83",
            "/api/v1/datasets?limit=1&limit=2",
            "/api/v1/runs/0000000000000000/orders?limit=-99999999999999999999999999999999",
            "/api/v1/runs/0000000000000000/equity?max_points=%E2%98%83",
            "/api/v1/runs/0000000000000000/analytics/series?offset=0",
            "/api/v1/market/datasets/l3-demo/book?event_index=99999999999999999999999999",
            "/api/v1/market/datasets/l3-demo/events?limit=%E2%98%83",
        ] {
            let (status, headers, body) = server.get(path, caller).await;
            assert_eq!(status, 400, "{path}: {body}");
            assert_eq!(body["error"]["code"], "invalid_request", "{path}: {body}");
            assert_eq!(headers["content-type"], "application/json", "{path}");
            assert_eq!(headers["cache-control"], "no-store", "{path}");
            assert_eq!(
                body["error"]["request_id"],
                headers["x-request-id"].to_str().unwrap()
            );
        }
    }
    server.stop().await;
    tdb.drop_db().await;
}
