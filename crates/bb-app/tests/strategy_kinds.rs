//! Every strategy kind the API accepts must be storable and runnable end to end against a real
//! Postgres (`BLACKBOX_TEST_DATABASE_URL`). Each kind is submitted through its preset (the
//! catalogue is the source of the list, and the exhaustive match below fails to compile when a
//! `StrategyKind` variant is added without a preset), so a schema CHECK that lags the Rust enum
//! surfaces here as a non-202 instead of as an HTTP 500 in production (0011 added `glft`).

mod common;

use std::collections::BTreeSet;

use bb_core::config::StrategyKind;
use bb_db::Scope;
use common::*;
use serde_json::json;

/// Position of each variant; exhaustive on purpose (no wildcard arm).
fn variant(kind: &StrategyKind) -> usize {
    match kind {
        StrategyKind::TouchMaker(_) => 0,
        StrategyKind::AvellanedaStoikov(_) => 1,
        StrategyKind::Glft(_) => 2,
        StrategyKind::ImbalanceTaker(_) => 3,
        StrategyKind::Scripted(_) => 4,
    }
}
const VARIANTS: usize = 5;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_strategy_kind_is_accepted_stored_and_run() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let manifest = register_dataset(&server.db, dir.path(), 10).await;
    let (_, key) = workspace_key(&server.db, "kinds", &[Scope::Read, Scope::Run]).await;

    let presets = bb_app::presets::presets(&manifest);
    let covered: BTreeSet<usize> = presets
        .iter()
        .map(|p| variant(&p.config.strategy.kind))
        .collect();
    assert_eq!(
        covered,
        (0..VARIANTS).collect::<BTreeSet<_>>(),
        "every StrategyKind variant needs a preset"
    );

    let mut seen = BTreeSet::new();
    for preset in presets {
        let label = preset.config.strategy.kind.label();
        if !seen.insert(label) {
            continue;
        }
        let mut config = serde_json::to_value(&preset.config).unwrap();
        bb_app::wire::to_wire(&mut config).unwrap();
        let (status, _, body) = server
            .post(
                "/api/v1/experiments",
                Some(&key),
                &json!({"config": config, "label": preset.id}),
            )
            .await;
        assert_eq!(status, 202, "{label} ({}): {body}", preset.id);
        let job_id = body["job"]["id"].as_str().expect("job id").to_string();
        let job = wait_job(&server, Some(&key), &job_id).await;
        assert_eq!(job["state"], "completed", "{label}: {job}");

        let stored: String = sqlx::query_scalar(
            "SELECT c.strategy_kind FROM bb.jobs j JOIN bb.experiment_configs c \
             ON c.id = j.experiment_config_id WHERE j.id = $1::uuid",
        )
        .bind(&job_id)
        .fetch_one(server.db.pool())
        .await
        .unwrap();
        assert_eq!(stored, label);
    }
    assert_eq!(seen.len(), VARIANTS);
    server.stop().await;
    tdb.drop_db().await;
}
