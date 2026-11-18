//! SEC-10: anonymous detail cache misses use the versioned demo trust rows, not Postgres.
//! Run with BLACKBOX_TEST_DATABASE_URL; all bundles and databases are disposable.

mod common;

use std::time::Duration;

use bb_app::{
    metrics::Metrics,
    v1::snapshot::{MAX_ARTIFACT_BYTES, MAX_ROWS, artifact_metadata_bytes},
};
use bb_db::{Database, Scope};
use common::*;
use serde_json::Value;

const LONG: Duration = Duration::from_secs(3600);

fn acquires(server: &Server) -> u64 {
    Metrics::get(&server.metrics.db_acquires_total)
}

async fn force_recheck(server: &Server) {
    server.state.snapshot.set_recheck(Duration::from_millis(1));
    tokio::time::sleep(Duration::from_millis(10)).await;
    server
        .state
        .snapshot
        .get(&server.state)
        .await
        .expect("snapshot rechecked");
    server.state.snapshot.set_recheck(LONG);
}

async fn mutate_artifact(db: &Database, run: uuid::Uuid, oversized: bool) {
    let mut tx = db.pool().begin().await.unwrap();
    sqlx::query("ALTER TABLE bb.run_artifacts DISABLE TRIGGER run_artifacts_immutable")
        .execute(&mut *tx)
        .await
        .unwrap();
    let statement = if oversized {
        "UPDATE bb.run_artifacts SET page_index=page_index||jsonb_build_object('oversized',repeat('x',65536)) WHERE run_id=$1"
    } else {
        "UPDATE bb.run_artifacts SET sha256=decode(repeat('ab',32),'hex') WHERE run_id=$1"
    };
    sqlx::query(statement)
        .bind(run)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE bb.run_artifacts ENABLE TRIGGER run_artifacts_immutable")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn assert_quarantined(server: &Server, run: &str) {
    let (status, _, body) = server.get(&format!("/api/v1/runs/{run}"), None).await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(body["error"]["code"], "integrity_failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn warm_anonymous_detail_misses_are_database_free_and_trust_rows_refresh() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let manifest = register_dataset(&server.db, dir.path(), 20).await;
    let (_, admin) = workspace_key(&server.db, "demo", &[Scope::Admin]).await;
    let mut ids = Vec::new();
    for seed in 1..=3 {
        let (id, _) = run(
            &server,
            &admin,
            config_json(&manifest, 100, seed),
            &format!("detail-{seed}"),
        )
        .await;
        ids.push(id);
    }
    server.stop().await;

    // No worker polling can contaminate the server's acquisition counter. Keyed reads warm
    // the disk bundles and produce the exact response bodies, without filling the anon cache.
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            slots: 0,
            ..Options::default()
        },
    )
    .await;
    server.state.snapshot.set_recheck(LONG);
    let mut expected: Vec<Value> = Vec::new();
    for id in &ids {
        let (status, _, body) = server
            .get(&format!("/api/v1/runs/{id}"), Some(&admin))
            .await;
        assert_eq!(status, 200, "{body}");
        expected.push(body);
    }
    let snapshot = server
        .state
        .snapshot
        .get(&server.state)
        .await
        .expect("snapshot built");
    assert_eq!(snapshot.artifacts.len(), ids.len());
    assert!(snapshot.artifacts.len() <= MAX_ROWS);
    let mut row_ids = Vec::new();
    let mut paths = Vec::new();
    for (id, body) in ids.iter().zip(&expected) {
        let view = snapshot
            .runs_by_id
            .values()
            .find(|v| &v.short_id == id)
            .unwrap();
        row_ids.push(view.run.id);
        let row = &snapshot.artifacts[&view.run.id];
        let bytes = artifact_metadata_bytes(row).unwrap();
        assert_eq!(bytes, serde_json::to_vec(row.as_ref()).unwrap().len());
        assert!(bytes <= MAX_ARTIFACT_BYTES);
        eprintln!("demo {id}: {bytes} encoded trust-row bytes (cap {MAX_ARTIFACT_BYTES})");
        let fingerprint = hex::encode(&view.fingerprint);
        let mut url = reqwest::Url::parse(&server.url(&format!("/api/v1/runs/{id}"))).unwrap();
        url.query_pairs_mut()
            .append_pair("platform", &view.run.platform);
        let platform_query = format!("?{}", url.query().unwrap());
        // Query strings use form encoding: a literal '+' is a space, not a platform '+'.
        // Check the unencoded request's existing semantics for both caller classes, and use
        // the correctly encoded form in the cold anonymous response-cache matrix below.
        if view.run.platform.contains('+') {
            let raw = format!("/api/v1/runs/{id}?platform={}", view.run.platform);
            for key in [None, Some(admin.as_str())] {
                let (status, _, error) = server.get(&raw, key).await;
                assert_eq!(status, 404, "{raw}: {error}");
            }
        }
        for identity in [
            id.clone(),
            id.to_uppercase(),
            fingerprint.clone(),
            fingerprint.to_uppercase(),
        ] {
            for query in [String::new(), platform_query.clone()] {
                paths.push((format!("/api/v1/runs/{identity}{query}"), body.clone()));
            }
        }
    }
    drop(snapshot);
    assert_eq!(paths.len(), 24);
    assert_eq!(
        paths
            .iter()
            .map(|(path, _)| path)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        paths.len(),
        "every anonymous detail response starts cold"
    );
    // The first anonymous body above 2 KiB intentionally loads the persisted egress budget.
    // Warm that class through the list route, leaving every measured detail URI uncached.
    let (status, _, list) = server.get("/api/v1/runs?limit=100", None).await;
    assert_eq!(status, 200, "{list}");
    assert!(serde_json::to_vec(&list).unwrap().len() > 2 * 1024);
    let before = acquires(&server);
    let hits = Metrics::get(&server.metrics.artifact_cache_hits_total);
    for (path, expected) in &paths {
        let (status, _, body) = server.get(path, None).await;
        assert_eq!(status, 200, "{path}: {body}");
        assert_eq!(&body, expected, "{path}");
    }
    // These routes share run_data's caller-aware resolution and open path. Distinct offsets
    // force additional response-cache misses, also against the already warm disk bundles.
    for id in &ids {
        for offset in 0..3 {
            let (status, _, body) = server
                .get(
                    &format!("/api/v1/runs/{id}/orders?offset={offset}&limit=1"),
                    None,
                )
                .await;
            assert_eq!(status, 200, "{body}");
        }
    }
    for id in &ids {
        for suffix in [
            "fills?limit=1",
            "equity?max_points=2",
            "tape?limit=1",
            "timeline?max_events=0",
            "engine-check",
        ] {
            let (status, _, body) = server
                .get(&format!("/api/v1/runs/{id}/{suffix}"), None)
                .await;
            assert_eq!(status, 200, "{suffix}: {body}");
        }
    }
    assert_eq!(
        acquires(&server) - before,
        0,
        "48 cold-response, warm-bundle reads"
    );
    assert!(Metrics::get(&server.metrics.artifact_cache_hits_total) - hits >= 48);

    // Analytics still queries its stored report once (not retained in the bounded snapshot).
    // Both routes must remove all metadata/trust-row acquisitions, including normalization's
    // trailer check. The keyed route retains DB resolution and the artifact lookup.
    for id in &ids {
        for suffix in [
            "analytics",
            "analytics/series?path=/execution/first_fill/km/curve&limit=1",
        ] {
            let path = format!("/api/v1/runs/{id}/{suffix}");
            let before = acquires(&server);
            let (status, _, expected) = server.get(&path, Some(&admin)).await;
            assert_eq!(status, 200, "{path}: {expected}");
            let keyed_cost = acquires(&server) - before;
            let before = acquires(&server);
            let (status, _, body) = server.get(&path, None).await;
            let anonymous_cost = acquires(&server) - before;
            assert_eq!(status, 200, "{path}: {body}");
            assert_eq!(body, expected, "{path}");
            assert_eq!(anonymous_cost, 1, "{path}: only the stored analytics row");
            assert!(keyed_cost >= anonymous_cost + 2, "{path}: {keyed_cost}");
            eprintln!("{path}: keyed {keyed_cost}, anonymous {anonymous_cost} acquisitions");
        }
    }

    // Keyed reads deliberately retain the DB path, even when the demo snapshot is warm.
    let before = acquires(&server);
    let (status, _, body) = server.get(&paths[0].0, Some(&admin)).await;
    assert_eq!(status, 200, "{body}");
    assert!(acquires(&server) > before);

    // An external trust-root mutation does not invalidate the snapshot until its existing
    // fingerprint check. At that check, even a cached bundle must use the new artefact row;
    // its inconsistent hash quarantines through Postgres rather than serving cached success.
    let external = tdb.pool().await;
    let demo = external.workspace("demo").await.unwrap();
    let version = external.demo_data_version(demo.id).await.unwrap();
    mutate_artifact(&external, row_ids[0], false).await;
    assert_ne!(external.demo_data_version(demo.id).await.unwrap(), version);
    let (status, _, body) = server.get(&paths[0].0, None).await;
    assert_eq!(status, 200, "before recheck: {body}");
    force_recheck(&server).await;
    let snapshot = server.state.snapshot.get(&server.state).await.unwrap();
    assert_eq!(snapshot.artifacts[&row_ids[0]].sha256, vec![0xab; 32]);
    drop(snapshot);
    assert_quarantined(&server, &ids[0]).await;
    assert_eq!(Metrics::get(&server.metrics.runs_quarantined_total), 1);
    assert!(
        external
            .quarantined_run(demo.id, &ids[0], &server.state.cfg.result_platform)
            .await
            .unwrap()
            .is_some()
    );
    assert_quarantined(&server, &ids[0]).await;

    // Quarantine performed by another process also removes previously cached successes at
    // the next fingerprint check, independently of local integrity-failure counters.
    external
        .quarantine_run(row_ids[1], "external quarantine")
        .await
        .unwrap();
    force_recheck(&server).await;
    assert_quarantined(&server, &ids[1]).await;
    let snapshot = server.state.snapshot.get(&server.state).await.unwrap();
    assert!(!snapshot.runs_by_id.contains_key(&row_ids[1]));
    assert!(!snapshot.artifacts.contains_key(&row_ids[1]));
    drop(snapshot);

    // Trust metadata has an explicit per-run cap; oversized rows fall back to DB reads and
    // cannot turn a bounded list snapshot into an unbounded copy of bundle metadata.
    mutate_artifact(&external, row_ids[2], true).await;
    let oversized = external.artifact(row_ids[2]).await.unwrap();
    assert!(artifact_metadata_bytes(&oversized).unwrap() > MAX_ARTIFACT_BYTES);
    force_recheck(&server).await;
    let snapshot = server.state.snapshot.get(&server.state).await.unwrap();
    assert!(snapshot.runs_by_id.contains_key(&row_ids[2]));
    assert!(!snapshot.artifacts.contains_key(&row_ids[2]));
    drop(snapshot);

    server.stop().await;
    external.pool().close().await;
    tdb.drop_db().await;
}
