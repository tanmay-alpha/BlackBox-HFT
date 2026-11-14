//! /api/v1 read models, tagged keys, bootstrap, fair claim and budgets against a throwaway
//! PostgreSQL 18 database (same pattern as `postgres.rs`).
use bb_db::*;
use chrono::{NaiveDate, Utc};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{
    Connection, PgConnection,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{str::FromStr, time::Duration};
use uuid::Uuid;

const PLATFORM: &str = "linux-x86_64";
const FAIR_PLATFORM: &str = "linux-aarch64";
const PEPPER: &[u8] = b"test-pepper-0123456789";

#[test]
fn key_tag_rejects_malformed_keys() {
    assert!(!key_tag_valid("", PEPPER));
    assert!(!key_tag_valid("bbk_short", PEPPER));
    assert!(!key_tag_valid(&format!("bbx_{}", "A".repeat(43)), PEPPER));
    assert!(!key_tag_valid(&format!("bbk_{}", "A".repeat(44)), PEPPER));
    assert!(!key_tag_valid(&format!("bbk_{}", "A".repeat(42)), PEPPER));
    assert!(!key_tag_valid(&format!("bbk_{}+", "A".repeat(42)), PEPPER));
    // Well-formed but random: the tag does not match.
    assert!(!key_tag_valid(&format!("bbk_{}", "A".repeat(43)), PEPPER));
    assert_eq!(
        key_sha256("bbk_x").to_vec(),
        Sha256::digest(b"bbk_x").to_vec()
    );
    assert_eq!(KEY_TAG_LEN, 8);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn postgres_api_reads() {
    let Ok(url) = std::env::var("BLACKBOX_TEST_DATABASE_URL") else {
        eprintln!(
            "SKIP postgres_api_reads: BLACKBOX_TEST_DATABASE_URL is unset (requires disposable local PostgreSQL 18 with CREATEDB)"
        );
        return;
    };
    let options = PgConnectOptions::from_str(&url).expect("valid test URL");
    let mut admin = PgConnection::connect_with(&options)
        .await
        .expect("connect local test admin");
    let name = format!("bb_test_{}", Uuid::new_v4().simple());
    // Identifier is generated solely from a fixed ASCII prefix and a hex UUID, never user input.
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\"")))
        .execute(&mut admin)
        .await
        .expect("test role needs CREATEDB");
    let opts = options.database(&name);
    let task = tokio::spawn(async move {
        let pool = PgPoolOptions::new()
            .min_connections(0)
            .max_connections(5)
            .connect_with(opts)
            .await
            .unwrap();
        MIGRATOR.run(&pool).await.unwrap();
        let db = Database::new(pool.clone());
        scenario(db).await;
        pool.close().await;
    });
    let result = task.await;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE \"{name}\" WITH (FORCE)"
    )))
    .execute(&mut admin)
    .await
    .expect("remove throwaway test database");
    result.expect("PostgreSQL scenario failed");
}

async fn config(db: &Database, version: &DatasetVersion, seed: u64) -> ExperimentConfig {
    let value = json!({"dataset_id":version.dataset_id,"seed":seed.to_string(),"strategy":{"kind":{"type":"touch_maker"},"interval_ns":"1000"}});
    db.register_config(&NewConfig {
        fingerprint: Sha256::digest(serde_json::to_vec(&value).unwrap()).to_vec(),
        dataset_version_id: version.id,
        dataset_id: version.dataset_id.clone(),
        contract_version: 2,
        simulation_revision: 2,
        engine_name: "hftbacktest".into(),
        engine_version: "0.9.4".into(),
        config: value,
        est_ticks: 10,
        est_tape_events_upper: 100,
        est_output_bytes_upper: 10000,
    })
    .await
    .unwrap()
}

/// Submits, claims and completes `c` in `ws`, returning the experiment, job and live run.
async fn make_run(
    db: &Database,
    c: &ExperimentConfig,
    ws: Uuid,
    pnl: f64,
    dd: f64,
) -> (Experiment, Job, Run) {
    let mut s = Submit::new(c.id, ws, PLATFORM);
    s.label = format!("run {pnl}");
    s.bypass_quotas = true;
    let sub = db.submit(&s).await.unwrap();
    let job = db.claim("worker", PLATFORM, 60).await.unwrap().unwrap();
    assert_eq!(Some(job.id), sub.job.as_ref().map(|j| j.id));
    let id = Uuid::new_v4();
    let r = NewRun {
        id,
        blackbox_version: "0.2.0".into(),
        build_profile: "release".into(),
        result_sha256: vec![3; 32],
        summary_sha256: vec![4; 32],
        ticks: 10,
        order_count: 2,
        fill_count: 1,
        tape_len: 3,
        equity_len: 2,
        wall_time_ms: 1,
        peak_rss_bytes: Some(4096),
        metrics: json!({"net_pnl":pnl,"fees":0.1,"max_drawdown":dd,"orders_submitted":2}),
        engine_check: json!({"agrees":true}),
    };
    let a = NewArtifact {
        format_version: 1,
        storage_backend: "fs".into(),
        storage_key: format!("runs/{}/{PLATFORM}/{id}.bbz", hex::encode(&c.fingerprint)),
        sha256: vec![5; 32],
        size_bytes: 100,
        uncompressed_bytes: 200,
        page_index: json!({}),
        page_index_sha256: vec![6; 32],
    };
    db.upload_intent("fs", &a.storage_key).await.unwrap();
    let done = db.complete(&job.lease().unwrap(), &r, &a).await.unwrap();
    (sub.experiment, job, done.run)
}

fn not_found<T: std::fmt::Debug>(r: Result<T>) {
    assert!(
        matches!(r, Err(Error::NotFound)),
        "expected NotFound: {r:?}"
    );
}

async fn scenario(db: Database) {
    keys_and_bootstrap(&db).await;
    budgets(&db).await;

    let a = db.create_workspace("alpha", "Alpha").await.unwrap();
    let b = db.create_workspace("beta", "Beta").await.unwrap();
    let demo = db.workspace("demo").await.unwrap();
    assert_eq!(db.workspace_by_id(a.id).await.unwrap().slug, "alpha");
    not_found(db.workspace_by_id(Uuid::new_v4()).await);
    assert_eq!(
        db.data_source("blackbox-synthetic").await.unwrap().id,
        "blackbox-synthetic"
    );
    not_found(db.data_source("absent").await);

    let inst = db
        .register_instrument("synthetic", "BTCUSD", "synthetic")
        .await
        .unwrap();
    assert_eq!(db.instrument(inst.id).await.unwrap().symbol, "BTCUSD");
    not_found(db.instrument(Uuid::new_v4()).await);
    let ds = db
        .register_dataset(NewDataset {
            id: "test-data",
            owner: None,
            title: "Test data",
            kind: "synthetic",
            instrument: inst.id,
            source: "blackbox-synthetic",
            hosted: true,
        })
        .await
        .unwrap();
    let version = db
        .register_version(&NewDatasetVersion {
            dataset_id: ds.id.clone(),
            version_no: 1,
            events_sha256: vec![1; 32],
            normalization_version: 1,
            contract_version: 2,
            tick_size: 0.1,
            lot_size: 0.01,
            contract_size: 1.0,
            origin_ns: 0,
            start_rel_ns: 0,
            end_rel_ns: 10000,
            timestamp_unit: "ns".into(),
            row_count: 1,
            events_bytes: 32,
            storage: "regenerate".into(),
            storage_key: None,
            manifest: json!({}),
        })
        .await
        .unwrap();
    assert_eq!(
        db.dataset_version_by_id(version.id)
            .await
            .unwrap()
            .version_no,
        1
    );
    not_found(db.dataset_version_by_id(Uuid::new_v4()).await);

    // ----- visibility -----
    let c1 = config(&db, &version, 1).await;
    let (exp_a, job_a, run_a) = make_run(&db, &c1, a.id, 1.0, 0.5).await;
    let cd = config(&db, &version, 2).await;
    let (exp_demo, job_demo, run_demo) = make_run(&db, &cd, demo.id, 2.0, 0.1).await;

    let view = db.run_view(a.id, &c1.short_id, PLATFORM).await.unwrap();
    assert_eq!(view.run.id, run_a.id);
    assert_eq!(view.view_experiment_id, Some(exp_a.id));
    assert_eq!(view.dataset_version_no, 1);
    let full = hex::encode(&c1.fingerprint);
    assert_eq!(
        db.run_view(a.id, &full, PLATFORM).await.unwrap().run.id,
        run_a.id
    );
    not_found(db.run_view(b.id, &c1.short_id, PLATFORM).await);
    not_found(db.run_view(a.id, &c1.short_id, "windows-x86_64").await);
    let public = db.run_view(b.id, &cd.short_id, PLATFORM).await.unwrap();
    assert_eq!(public.view_experiment_id, Some(exp_demo.id));
    assert_eq!(
        db.run_view_by_id(a.id, run_a.id).await.unwrap().run.id,
        run_a.id
    );
    not_found(db.run_view_by_id(b.id, run_a.id).await);
    assert_eq!(
        db.run_view_by_id(b.id, run_demo.id).await.unwrap().run.id,
        run_demo.id
    );
    let by_ids = db
        .run_views_by_ids(b.id, &[run_a.id, run_demo.id])
        .await
        .unwrap();
    assert_eq!(by_ids.len(), 1);
    assert_eq!(by_ids[0].run.id, run_demo.id);

    let ev = db.experiment_view(a.id, exp_a.id).await.unwrap();
    assert_eq!(ev.status, "completed");
    assert_eq!(ev.live_run_id, Some(run_a.id));
    assert_eq!(ev.workspace_slug, "alpha");
    not_found(db.experiment_view(b.id, exp_a.id).await);
    assert_eq!(
        db.experiment_view(b.id, exp_demo.id)
            .await
            .unwrap()
            .workspace_slug,
        "demo"
    );

    let jv = db.job_view(a.id, job_a.id).await.unwrap();
    assert_eq!(jv.run_short_id.as_deref(), Some(c1.short_id.as_str()));
    assert_eq!(jv.short_id, c1.short_id);
    not_found(db.job_view(b.id, job_a.id).await);
    assert_eq!(
        db.job_view(b.id, job_demo.id).await.unwrap().job.id,
        job_demo.id
    );
    let jobs = db.job_views_by_ids(&[job_a.id, job_demo.id]).await.unwrap();
    assert_eq!(jobs.len(), 2);

    let vis = db
        .visible_identities(
            b.id,
            &[c1.fingerprint.clone(), cd.fingerprint.clone()],
            PLATFORM,
        )
        .await
        .unwrap();
    assert_eq!(vis.len(), 1);
    assert_eq!(vis[0].short_id, cd.short_id);
    assert_eq!(vis[0].experiment_id, exp_demo.id);
    assert_eq!(
        db.visible_identities(a.id, std::slice::from_ref(&c1.fingerprint), PLATFORM)
            .await
            .unwrap()
            .len(),
        1
    );

    // ----- keyset paging of run listings -----
    for (seed, pnl, dd) in [
        (10, 3.0, 0.5),
        (11, -1.0, 0.1),
        (12, 3.0, 0.3),
        (13, 0.0, 0.2),
        (14, 2.0, 0.5),
    ] {
        let c = config(&db, &version, seed).await;
        make_run(&db, &c, a.id, pnl, dd).await;
    }
    let f = ExperimentQuery::default();
    for sort in [
        RunSort::CreatedDesc,
        RunSort::NetPnlDesc,
        RunSort::NetPnlAsc,
        RunSort::MaxDrawdownAsc,
    ] {
        let all = db
            .run_views(a.id, PLATFORM, &f, sort, None, 100)
            .await
            .unwrap();
        assert_eq!(all.len(), 6, "{sort:?}");
        for w in all.windows(2) {
            let (x, y) = (&w[0].run, &w[1].run);
            let ordered = match sort {
                RunSort::CreatedDesc => (x.created_at, x.id) > (y.created_at, y.id),
                RunSort::NetPnlDesc => (x.net_pnl.unwrap(), x.id) > (y.net_pnl.unwrap(), y.id),
                RunSort::NetPnlAsc => (x.net_pnl.unwrap(), x.id) < (y.net_pnl.unwrap(), y.id),
                RunSort::MaxDrawdownAsc => {
                    (x.max_drawdown.unwrap(), x.id) < (y.max_drawdown.unwrap(), y.id)
                }
            };
            assert!(ordered, "{sort:?} order");
        }
        let mut paged = Vec::new();
        let mut after: Option<RunKey> = None;
        loop {
            let page = db
                .run_views(a.id, PLATFORM, &f, sort, after.as_ref(), 4)
                .await
                .unwrap();
            if page.is_empty() {
                break;
            }
            after = Some(page.last().unwrap().key(sort));
            paged.extend(page.into_iter().map(|v| v.run.id));
        }
        let expected: Vec<Uuid> = all.iter().map(|v| v.run.id).collect();
        assert_eq!(paged, expected, "{sort:?} paging");
    }
    let filtered = ExperimentQuery {
        run_identity: Some(c1.short_id.clone()),
        ..Default::default()
    };
    let one = db
        .run_views(a.id, PLATFORM, &filtered, RunSort::CreatedDesc, None, 10)
        .await
        .unwrap();
    assert_eq!(one.len(), 1);
    assert!(
        db.run_views(b.id, PLATFORM, &f, RunSort::CreatedDesc, None, 10)
            .await
            .unwrap()
            .is_empty()
    );

    // ----- quarantine -----
    let cq = config(&db, &version, 20).await;
    let (_, _, bad) = make_run(&db, &cq, a.id, 1.0, 0.1).await;
    assert!(
        db.quarantined_run(a.id, &cq.short_id, PLATFORM)
            .await
            .unwrap()
            .is_none()
    );
    db.quarantine_run(bad.id, "bundle hash mismatch")
        .await
        .unwrap();
    assert_eq!(
        db.quarantined_run(a.id, &cq.short_id, PLATFORM)
            .await
            .unwrap()
            .unwrap()
            .id,
        bad.id
    );
    assert!(
        db.quarantined_run(b.id, &cq.short_id, PLATFORM)
            .await
            .unwrap()
            .is_none()
    );
    not_found(db.run_view(a.id, &cq.short_id, PLATFORM).await);
    let (_, _, good) = make_run(&db, &cq, a.id, 1.0, 0.1).await;
    assert!(
        db.quarantined_run(a.id, &cq.short_id, PLATFORM)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        db.run_view(a.id, &cq.short_id, PLATFORM)
            .await
            .unwrap()
            .run
            .id,
        good.id
    );

    // ----- experiments, sweeps, quotas and queue -----
    let key = db
        .mint_key(a.id, "runner", &[Scope::Run], None)
        .await
        .unwrap();
    let before = db.quota_usage(a.id, None).await.unwrap();
    assert_eq!(
        before,
        QuotaUsage {
            experiments_today: 7,
            sweeps_today: 0,
            active_jobs: 0
        }
    );
    let ck = config(&db, &version, 30).await;
    let mut s = Submit::new(ck.id, a.id, PLATFORM);
    s.api_key_id = Some(key.key.id);
    s.label = "keyed one".into();
    let keyed = db.submit(&s).await.unwrap();
    let keyed_job = keyed.job.clone().unwrap();
    let sweep = db
        .create_sweep(
            &NewSweep {
                workspace_id: a.id,
                spec: json!({"x":{"type":"spread"},"y":{"type":"size"}}),
                spec_fingerprint: vec![9; 32],
                x_len: 1,
                y_len: 1,
                label: "grid".into(),
                created_by_key_id: Some(key.key.id),
            },
            false,
        )
        .await
        .unwrap();
    let cs = config(&db, &version, 31).await;
    let mut s = Submit::new(cs.id, a.id, PLATFORM);
    s.sweep = Some((sweep.id, 0, 0));
    let cell = db.submit(&s).await.unwrap();
    assert_eq!(
        db.quota_usage(a.id, None).await.unwrap(),
        QuotaUsage {
            experiments_today: 9,
            sweeps_today: 1,
            active_jobs: 2
        }
    );
    assert_eq!(
        db.quota_usage(a.id, Some(key.key.id)).await.unwrap(),
        QuotaUsage {
            experiments_today: 1,
            sweeps_today: 1,
            active_jobs: 2
        }
    );
    assert_eq!(db.quota_usage(b.id, None).await.unwrap().active_jobs, 0);

    let cells = db.sweep_cell_views(sweep.id).await.unwrap();
    assert_eq!(cells.len(), 1);
    assert_eq!(cells[0].experiment_id, cell.experiment.id);
    assert_eq!(cells[0].job_state.as_deref(), Some("queued"));
    assert!(cells[0].run_short_id.is_none());

    let all = db
        .experiment_views(a.id, &ExperimentQuery::default(), None, 100)
        .await
        .unwrap();
    assert_eq!(all.len(), 9);
    let queued = db
        .experiment_views(
            a.id,
            &ExperimentQuery {
                states: vec!["queued".into()],
                ..Default::default()
            },
            None,
            100,
        )
        .await
        .unwrap();
    assert_eq!(queued.len(), 2);
    let labelled = db
        .experiment_views(
            a.id,
            &ExperimentQuery {
                label_prefix: Some("keyed".into()),
                ..Default::default()
            },
            None,
            100,
        )
        .await
        .unwrap();
    assert_eq!(labelled.len(), 1);
    assert_eq!(
        labelled[0].key_prefix.as_deref(),
        Some(key.key.display_prefix.as_str())
    );
    let mut paged = Vec::new();
    let mut cursor: Option<Cursor> = None;
    loop {
        let page = db
            .experiment_views(a.id, &ExperimentQuery::default(), cursor.as_ref(), 4)
            .await
            .unwrap();
        let Some(last) = page.last() else { break };
        cursor = Some(Cursor {
            created_at: last.experiment.created_at,
            id: last.experiment.id,
        });
        paged.extend(page.iter().map(|e| e.experiment.id));
    }
    assert_eq!(
        paged,
        all.iter().map(|e| e.experiment.id).collect::<Vec<_>>()
    );

    let mine = db
        .job_views(a.id, &[], Some(key.key.id), None, 10)
        .await
        .unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].job.id, keyed_job.id);
    let queued_jobs = db
        .job_views(a.id, &["queued".to_string()], None, None, 10)
        .await
        .unwrap();
    assert_eq!(queued_jobs.len(), 2);
    assert_eq!(
        db.job_views(a.id, &[], None, None, 100)
            .await
            .unwrap()
            .len(),
        9
    );

    db.emit_event(NewSystemEvent {
        level: "warn",
        kind: "lease_reclaimed",
        worker: Some("worker"),
        job: Some(keyed_job.id),
        run: None,
        message: "lease expired",
        detail: &json!({}),
    })
    .await
    .unwrap();
    let stats = db.queue_stats(Some(PLATFORM)).await.unwrap();
    assert_eq!((stats.queued, stats.running), (2, 0));
    assert_eq!(stats.oldest_queued_at, keyed_job.queued_at);
    assert_eq!(stats.lease_reclaims_24h, 1);
    let other = db.queue_stats(Some("windows-x86_64")).await.unwrap();
    assert_eq!((other.queued, other.oldest_queued_at), (0, None));

    // ----- analytics -----
    let sha = Sha256::digest(b"{\"e1\":1}").to_vec();
    let row = db
        .put_run_analytics(run_a.id, "1", "{\"e1\":1}", &sha)
        .await
        .unwrap();
    assert_eq!(row.report_json, "{\"e1\":1}");
    assert_eq!(
        db.put_run_analytics(run_a.id, "1", "{\"e1\":1}", &sha)
            .await
            .unwrap(),
        row
    );
    assert!(matches!(
        db.put_run_analytics(run_a.id, "1", "{\"e1\":2}", &[7; 32])
            .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        db.put_run_analytics(run_a.id, "1", "{}", &[1; 3]).await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(db.run_analytics(run_a.id).await.unwrap(), Some(row));
    assert!(db.run_analytics(run_demo.id).await.unwrap().is_none());
    let drow = db
        .put_dataset_analytics(version.id, "1", "{\"m1\":1}", &sha)
        .await
        .unwrap();
    assert_eq!(
        db.put_dataset_analytics(version.id, "1", "{\"m1\":1}", &sha)
            .await
            .unwrap(),
        drow
    );
    assert!(matches!(
        db.put_dataset_analytics(version.id, "1", "{\"m1\":2}", &[7; 32])
            .await,
        Err(Error::Conflict)
    ));
    db.put_dataset_analytics(version.id, "2", "{\"m1\":2}", &[7; 32])
        .await
        .unwrap();
    assert_eq!(
        db.dataset_analytics(version.id, "1").await.unwrap(),
        Some(drow)
    );
    assert!(
        db.dataset_analytics(version.id, "3")
            .await
            .unwrap()
            .is_none()
    );

    fair_claim(&db, &version).await;
}

async fn audit_count(db: &Database, action: &str, target: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM bb.audit_events WHERE action=$1 AND target_id=$2 AND outcome='ok'",
    )
    .bind(action)
    .bind(target.to_string())
    .fetch_one(db.pool())
    .await
    .unwrap()
}

async fn keys_and_bootstrap(db: &Database) {
    let ops = db.workspace("system").await.unwrap();
    let ws = db.create_workspace("keys", "Keys").await.unwrap();

    // Bootstrap with no active admin key anywhere: created, 7 days, audited.
    let digest = key_sha256("bbk_bootstrap-plaintext-is-never-sent-to-the-server");
    let week = Duration::from_secs(7 * 24 * 3600);
    let BootstrapOutcome::Created(boot) = db
        .ensure_bootstrap_admin_key(ops.id, &digest, week)
        .await
        .unwrap()
    else {
        panic!("bootstrap key must be created");
    };
    assert_eq!(boot.scopes, vec!["admin".to_string()]);
    assert_eq!(boot.label, "bootstrap");
    assert_eq!(boot.display_prefix, "bbk_boot");
    assert_eq!(boot.key_sha256, digest.to_vec());
    assert_eq!(
        boot.expires_at.unwrap() - boot.created_at,
        chrono::Duration::days(7)
    );
    assert_eq!(audit_count(db, "key.bootstrap", boot.id).await, 1);
    assert!(matches!(
        db.ensure_bootstrap_admin_key(ops.id, &digest, week)
            .await
            .unwrap(),
        BootstrapOutcome::ActiveAdminExists
    ));
    assert!(matches!(
        db.ensure_bootstrap_admin_key(ops.id, &[8; 32], week)
            .await
            .unwrap(),
        BootstrapOutcome::ActiveAdminExists
    ));
    not_found(
        db.ensure_bootstrap_admin_key(Uuid::new_v4(), &digest, week)
            .await,
    );
    assert!(matches!(
        db.ensure_bootstrap_admin_key(ops.id, &digest, Duration::ZERO)
            .await,
        Err(Error::Invalid(_))
    ));

    // Operator revoke: idempotent, audited per call.
    let revoked = db.revoke_key_any(boot.id, &Audit::default()).await.unwrap();
    let first = revoked.revoked_at.unwrap();
    assert_eq!(
        db.revoke_key_any(boot.id, &Audit::default())
            .await
            .unwrap()
            .revoked_at,
        Some(first)
    );
    assert_eq!(audit_count(db, "key.revoke", boot.id).await, 2);
    not_found(db.revoke_key_any(Uuid::new_v4(), &Audit::default()).await);

    // A revoked digest is never resurrected.
    let BootstrapOutcome::Inactive(again) = db
        .ensure_bootstrap_admin_key(ops.id, &digest, week)
        .await
        .unwrap()
    else {
        panic!("revoked bootstrap digest must stay inactive");
    };
    assert_eq!(again.id, boot.id);
    assert_eq!(again.revoked_at, Some(first));
    let boot_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM bb.api_keys")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(boot_rows, 1);

    // Tagged minting.
    let minted = db
        .mint_key_tagged(ws.id, "tagged admin", &[Scope::Admin], None, PEPPER)
        .await
        .unwrap();
    assert_eq!(minted.plaintext.len(), 47);
    assert_eq!(minted.key.display_prefix, minted.plaintext[..8]);
    assert_eq!(
        minted.key.key_sha256,
        key_sha256(&minted.plaintext).to_vec()
    );
    assert!(key_tag_valid(&minted.plaintext, PEPPER));
    assert!(!key_tag_valid(&minted.plaintext, b"other pepper"));
    assert!(!key_tag_valid(&minted.plaintext, b""));
    let mut forged = minted.plaintext.clone().into_bytes();
    let last = forged.len() - 1;
    forged[last] = if forged[last] == b'A' { b'Q' } else { b'A' };
    assert!(!key_tag_valid(
        std::str::from_utf8(&forged).unwrap(),
        PEPPER
    ));
    let mut flipped = minted.plaintext.clone().into_bytes();
    flipped[5] = if flipped[5] == b'A' { b'B' } else { b'A' };
    assert!(!key_tag_valid(
        std::str::from_utf8(&flipped).unwrap(),
        PEPPER
    ));
    let untagged = db
        .mint_key(ws.id, "untagged", &[Scope::Read], None)
        .await
        .unwrap();
    assert!(!key_tag_valid(&untagged.plaintext, PEPPER));
    assert_eq!(
        db.authenticate(&minted.plaintext)
            .await
            .unwrap()
            .unwrap()
            .id,
        minted.key.id
    );
    assert_eq!(audit_count(db, "key.mint", minted.key.id).await, 1);
    for (label, scopes, pepper) in [
        ("", &[Scope::Read][..], PEPPER),
        ("x".repeat(101).as_str(), &[Scope::Read][..], PEPPER),
        ("bad\nlabel", &[Scope::Read][..], PEPPER),
        ("ok", &[][..], PEPPER),
        ("ok", &[Scope::Read][..], b"".as_slice()),
    ] {
        assert!(matches!(
            db.mint_key_tagged(ws.id, label, scopes, None, pepper).await,
            Err(Error::Invalid(_))
        ));
    }
    let expiring = Utc::now() + chrono::Duration::days(30);
    let read = db
        .mint_key_tagged(ws.id, "reader é", &[Scope::Read], Some(expiring), PEPPER)
        .await
        .unwrap();
    assert!(key_tag_valid(&read.plaintext, PEPPER));
    assert_eq!(read.key.scopes, vec!["read".to_string()]);

    // An active admin exists (tagged admin): a fresh digest creates nothing.
    assert!(matches!(
        db.ensure_bootstrap_admin_key(ops.id, &[7; 32], week)
            .await
            .unwrap(),
        BootstrapOutcome::ActiveAdminExists
    ));

    // Key reads.
    assert_eq!(db.api_key(read.key.id).await.unwrap().label, "reader é");
    not_found(db.api_key(Uuid::new_v4()).await);
    let all = db.all_api_keys().await.unwrap();
    assert_eq!(all.len(), 4);
    assert!(
        all.windows(2)
            .all(|w| (w[0].created_at, w[0].id) > (w[1].created_at, w[1].id))
    );
}

async fn budgets(db: &Database) {
    // Day 1 of a 30-day month, 9,000 reads/month: pacing allows 300 reads today.
    let nov1 = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    for i in 0..300 {
        assert!(
            db.charge_storage_read_on("vercel_blob", 10, 9_000, i64::MAX, nov1)
                .await
                .unwrap(),
            "read {i}"
        );
    }
    assert!(
        !db.charge_storage_read_on("vercel_blob", 10, 9_000, i64::MAX, nov1)
            .await
            .unwrap()
    );
    let (reads, bytes): (i32, i64) = sqlx::query_as("SELECT reads,read_bytes FROM bb.storage_usage_monthly WHERE month='2026-11-01' AND storage_backend='vercel_blob'").fetch_one(db.pool()).await.unwrap();
    assert_eq!((reads, bytes), (300, 3000));
    // Next day: (9,000 - 300) / 29 = 300 more.
    let nov2 = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
    for _ in 0..300 {
        assert!(
            db.charge_storage_read_on("vercel_blob", 0, 9_000, i64::MAX, nov2)
                .await
                .unwrap()
        );
    }
    assert!(
        !db.charge_storage_read_on("vercel_blob", 0, 9_000, i64::MAX, nov2)
            .await
            .unwrap()
    );

    // Last day of the month: the remaining monthly reads, and the byte cap, bound the charge.
    let nov30 = NaiveDate::from_ymd_opt(2026, 11, 30).unwrap();
    assert!(
        db.charge_storage_read_on("fs", 600, 5, 1000, nov30)
            .await
            .unwrap()
    );
    assert!(
        !db.charge_storage_read_on("fs", 500, 5, 1000, nov30)
            .await
            .unwrap()
    );
    assert!(
        db.charge_storage_read_on("fs", 400, 5, 1000, nov30)
            .await
            .unwrap()
    );
    assert!(
        !db.charge_storage_read_on("fs", 1, 5, 1000, nov30)
            .await
            .unwrap()
    );
    for _ in 0..3 {
        assert!(
            db.charge_storage_read_on("fs", 0, 5, 1000, nov30)
                .await
                .unwrap()
        );
    }
    assert!(
        !db.charge_storage_read_on("fs", 0, 5, 1000, nov30)
            .await
            .unwrap()
    );
    let (reads, bytes): (i32, i64) = sqlx::query_as("SELECT reads,read_bytes FROM bb.storage_usage_monthly WHERE month='2026-11-01' AND storage_backend='fs'").fetch_one(db.pool()).await.unwrap();
    assert_eq!((reads, bytes), (5, 1000));
    // A new month starts from zero.
    let dec1 = NaiveDate::from_ymd_opt(2026, 12, 1).unwrap();
    assert!(
        db.charge_storage_read_on("fs", 0, 5, 1000, dec1)
            .await
            .unwrap()
    );
    assert!(matches!(
        db.charge_storage_read_on("fs", -1, 5, 1000, dec1).await,
        Err(Error::Invalid(_))
    ));
    assert!(
        db.charge_storage_read("vercel_blob", 1, 1_000_000, i64::MAX)
            .await
            .unwrap()
    );

    // Egress.
    assert_eq!(db.egress_today("anonymous").await.unwrap(), 0);
    assert_eq!(db.add_egress("anonymous", 100).await.unwrap(), 100);
    assert_eq!(db.add_egress("anonymous", 50).await.unwrap(), 150);
    assert_eq!(db.egress_today("anonymous").await.unwrap(), 150);
    assert_eq!(db.egress_today("keyed").await.unwrap(), 0);
    assert_eq!(db.add_egress("keyed", 7).await.unwrap(), 7);
    assert!(matches!(
        db.add_egress("other", 1).await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        db.add_egress("keyed", -1).await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        db.egress_today("other").await,
        Err(Error::Invalid(_))
    ));
}

/// SEC-8: workspace A queues five jobs, B one job later; one worker claiming sequentially
/// serves B second.
async fn fair_claim(db: &Database, version: &DatasetVersion) {
    let a = db.create_workspace("fair-a", "Fair A").await.unwrap();
    let b = db.create_workspace("fair-b", "Fair B").await.unwrap();
    let mut a_jobs = Vec::new();
    for seed in 100..105 {
        let c = config(db, version, seed).await;
        let job = db
            .submit(&Submit::new(c.id, a.id, FAIR_PLATFORM))
            .await
            .unwrap()
            .job
            .unwrap();
        a_jobs.push(job.id);
    }
    let c = config(db, version, 200).await;
    let b_job = db
        .submit(&Submit::new(c.id, b.id, FAIR_PLATFORM))
        .await
        .unwrap()
        .job
        .unwrap();
    assert!(matches!(
        db.claim_fair("", FAIR_PLATFORM, 60).await,
        Err(Error::Invalid(_))
    ));
    let mut order = Vec::new();
    while let Some(job) = db
        .claim_fair("fair-worker", FAIR_PLATFORM, 60)
        .await
        .unwrap()
    {
        assert_eq!(job.state, "running");
        assert_eq!(job.lease_owner.as_deref(), Some("fair-worker"));
        let attempts = db.attempts(job.id).await.unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].worker_id, "fair-worker");
        db.fail(&job.lease().unwrap(), "internal", "test", "test")
            .await
            .unwrap();
        order.push(job.id);
    }
    assert_eq!(order.len(), 6);
    assert_eq!(order[0], a_jobs[0]);
    assert_eq!(order[1], b_job.id);
    assert_eq!(&order[2..], &a_jobs[1..]);
}
