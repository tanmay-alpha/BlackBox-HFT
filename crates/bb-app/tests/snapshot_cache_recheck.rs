//! Cached responses must not hide an externally changed demo snapshot.
mod common;

use common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cached_lists_do_not_hide_due_snapshot_rechecks() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let path = "/api/v1/datasets?limit=50";
    let (status, _, before) = server.get(path, None).await;
    assert_eq!(status, 200);
    assert!(before["items"].as_array().unwrap().is_empty());

    register_dataset(&server.db, dir.path(), 20).await;
    server
        .state
        .snapshot
        .set_recheck(std::time::Duration::from_millis(1));
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let (status, _, after) = server.get(path, None).await;
    assert_eq!(status, 200);
    assert_eq!(after["items"].as_array().unwrap().len(), 1);
    assert_eq!(after["items"][0]["id"], DATASET);
    server.stop().await;
    tdb.drop_db().await;
}
