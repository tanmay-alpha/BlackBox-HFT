//! Workbench endpoints use the real API middleware, with a fresh test database.
mod common;
use common::*;
use serde_json::Value;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn synthetic_market_is_bounded_paginated_and_private_files_are_unreachable() {
    let Some(db) = TestDb::create().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&db, dir.path(), Options::default()).await;
    let root = "/api/v1/market/datasets/l3-demo";
    let (status, _, catalogue) = server.get("/api/v1/market/datasets", None).await;
    assert_eq!(status, 200, "{catalogue}");
    assert_eq!(catalogue["items"].as_array().unwrap().len(), 1);
    let (status, _, summary) = server.get(root, None).await;
    assert_eq!(status, 200, "{summary}");
    assert_eq!(summary["data_class"], "synthetic");
    assert_eq!(summary["quality"]["quarantined_total"], 0);
    let total = summary["events"].as_u64().unwrap();
    let (status, _, first) = server.get(&format!("{root}/events?limit=3"), None).await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["items"].as_array().unwrap().len(), 3);
    assert_eq!(first["truncated_by"], "limit");
    let cursor = first["next_cursor"].as_str().unwrap();
    let (status, _, second) = server
        .get(&format!("{root}/events?limit=3&cursor={cursor}"), None)
        .await;
    assert_eq!(status, 200, "{second}");
    assert!(
        second["items"][0]["event_index"].as_u64().unwrap()
            > first["items"][2]["event_index"].as_u64().unwrap()
    );
    let (status, _, mismatch) = server
        .get(
            &format!("{root}/events?limit=3&message_type=A&cursor={cursor}"),
            None,
        )
        .await;
    assert_eq!(status, 400, "{mismatch}");
    let (status, _, forged) = server
        .get(&format!("{root}/events?cursor=forged"), None)
        .await;
    assert_eq!(status, 400, "{forged}");
    let (status, _, sought) = server
        .get(&format!("{root}/events?from_event_index=100&limit=2"), None)
        .await;
    assert_eq!(status, 200, "{sought}");
    assert!(sought["items"][0]["event_index"].as_u64().unwrap() >= 100);
    let (status, _, filtered) = server
        .get(&format!("{root}/events?message_type=A&limit=500"), None)
        .await;
    assert_eq!(status, 200, "{filtered}");
    assert!(
        filtered["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["message_type"] == "A")
    );
    assert!(serde_json::to_vec(&filtered).unwrap().len() <= 256 * 1024);
    let (status, _, book) = server
        .get(
            &format!(
                "{root}/book?event_index={}&levels=2&orders_per_level=2",
                total - 1
            ),
            None,
        )
        .await;
    assert_eq!(status, 200, "{book}");
    for side in ["bids", "asks"] {
        assert!(book[side].as_array().unwrap().len() <= 2);
        for level in book[side].as_array().unwrap() {
            let orders = level["orders"].as_array().unwrap();
            assert!(orders.len() <= 2);
            let displayed: u64 = orders.iter().map(|o| o["shares"].as_u64().unwrap()).sum();
            assert!(displayed <= level["shares"].as_u64().unwrap());
            for (index, order) in orders.iter().enumerate() {
                assert_eq!(order["queue_position"], (index + 1) as u64);
                assert!(order["order_ref"].is_string());
                assert!(order["entered_ns"].is_string());
            }
        }
    }
    let (status, _, metrics) = server
        .get(&format!("{root}/metrics?max_points=7"), None)
        .await;
    assert_eq!(status, 200, "{metrics}");
    assert!(metrics["series"].as_array().unwrap().len() <= 7);
    assert_eq!(
        metrics["series"].as_array().unwrap().last().unwrap()["event_index"],
        total - 1
    );
    assert!(metrics["l2"].is_object());
    let (status, _, comparison) = server.get(&format!("{root}/comparison"), None).await;
    assert_eq!(status, 200, "{comparison}");
    assert!(comparison["touch_queue_observations"].as_u64().unwrap() > 0);
    for query in [
        "events?limit=0",
        "events?limit=501",
        "events?from_event_index=100000",
        "events?message_type=AB",
        "events?t0_ns=-1",
        "events?t0_ns=2&t1_ns=1",
        "events?t1_ns=86400000000000",
        "book?event_index=0&levels=21",
        "book?event_index=0&orders_per_level=101",
        "metrics?max_points=1",
        "metrics?max_points=501",
    ] {
        let (status, _, body) = server.get(&format!("{root}/{query}"), None).await;
        assert_eq!(status, 400, "{query}: {body}");
    }
    let (status, _, body) = server
        .get(&format!("{root}/book?event_index={total}"), None)
        .await;
    assert_eq!(status, 404, "{body}");
    // Files/provenance labels cannot opt a private artifact into the public catalogue.
    std::fs::create_dir_all(dir.path().join("market/private")).unwrap();
    std::fs::write(
        dir.path().join("market/private/provenance.json"),
        br#"{"data_class":"synthetic"}"#,
    )
    .unwrap();
    let (status, _, body) = server.get("/api/v1/market/datasets/private", None).await;
    assert_eq!(status, 404, "{body}");
    let response = server
        .http
        .get(format!("{}{root}/events?limit=500", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let bytes = response.bytes().await.unwrap();
    assert!(bytes.len() <= 256 * 1024);
    let _: Value = serde_json::from_slice(&bytes).unwrap();
    server.stop().await;
    db.drop_db().await;
}
