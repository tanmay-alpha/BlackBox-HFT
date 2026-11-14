//! Requires PostgreSQL 18 and a local test role with CREATEDB. Never points at Neon or Blob.
//! A single serial scenario owns a fresh, random database; concurrent tasks exercise races within it.
use bb_db::*;
use chrono::Utc;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{
    Connection, PgConnection,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{str::FromStr, sync::Arc};
use tokio::sync::Barrier;
use uuid::Uuid;

const PLATFORM: &str = "linux-x86_64";

#[test]
fn legal_state_machine_and_cursors() {
    use JobState::*;
    let states = [
        Created, Queued, Running, Cancelling, Completed, Failed, Cancelled, Expired,
    ];
    let edges = [
        (Created, Queued, Actor::Api),
        (Created, Cancelled, Actor::Api),
        (Created, Expired, Actor::Reaper),
        (Queued, Running, Actor::Worker),
        (Queued, Cancelled, Actor::Api),
        (Queued, Expired, Actor::Reaper),
        (Running, Completed, Actor::Worker),
        (Running, Failed, Actor::Worker),
        (Running, Failed, Actor::Reaper),
        (Running, Queued, Actor::Worker),
        (Running, Queued, Actor::Reaper),
        (Running, Cancelling, Actor::Api),
        (Running, Expired, Actor::Worker),
        (Cancelling, Cancelled, Actor::Worker),
        (Cancelling, Cancelled, Actor::Reaper),
    ];
    for from in states {
        for to in states {
            for actor in [Actor::Api, Actor::Worker, Actor::Reaper] {
                assert_eq!(
                    from.transition(to, actor).is_ok(),
                    edges.contains(&(from, to, actor)),
                    "{from:?} -> {to:?} by {actor:?}"
                );
            }
        }
    }
    let c = Cursor {
        created_at: Utc::now(),
        id: Uuid::new_v4(),
    };
    let encoded = c.encode("workspace=alpha").unwrap();
    assert_eq!(
        Cursor::decode(&encoded, "workspace=alpha").unwrap().id,
        c.id
    );
    assert!(Cursor::decode(&encoded, "workspace=beta").is_err());
    assert!(Cursor::decode("not a cursor", "workspace=alpha").is_err());
    let cfg = DatabaseConfig::from_urls("postgres://unused", "postgres://unused");
    assert_eq!(cfg.max_connections, 5);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn postgres_repositories_and_queue_races() {
    let Ok(url) = std::env::var("BLACKBOX_TEST_DATABASE_URL") else {
        eprintln!(
            "SKIP postgres_repositories_and_queue_races: BLACKBOX_TEST_DATABASE_URL is unset (requires disposable local PostgreSQL 18 with CREATEDB)"
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
        // Embedded migration replay must be idempotent.
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
fn submission(c: &ExperimentConfig, w: Uuid) -> Submit {
    let mut s = Submit::new(c.id, w, PLATFORM);
    s.label = "first label".into();
    s
}
fn result(c: &ExperimentConfig) -> (NewRun, NewArtifact) {
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
        metrics: json!({"net_pnl":1.0,"fees":0.1,"max_drawdown":0.2,"orders_submitted":2}),
        engine_check: json!({"agrees":true}),
    };
    let a = NewArtifact {
        format_version: 1,
        storage_backend: "fs".into(),
        storage_key: format!("runs/{}/{PLATFORM}/{id}.bbz", hex::encode(&c.fingerprint)),
        sha256: vec![5; 32],
        size_bytes: 100,
        uncompressed_bytes: 200,
        page_index: json!({"header":{"offset":0,"len":10,"sha256":"header"},"trailer":{"offset":90,"len":10,"sha256":"trailer"},"manifest_sha256":"manifest","page_index":{"pages":[]},"result_sha256":"science"}),
        page_index_sha256: vec![6; 32],
    };
    (r, a)
}
async fn force_lease_expired(db: &Database, id: Uuid) {
    sqlx::query("UPDATE bb.jobs SET lease_expires_at=now()-interval '1 second' WHERE id=$1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
}
async fn runnable(db: &Database, id: Uuid) {
    sqlx::query("UPDATE bb.jobs SET run_after=now() WHERE id=$1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
}
async fn scenario(db: Database) {
    assert_eq!(db.workspaces().await.unwrap().len(), 2);
    let a = db.create_workspace("alpha", "Alpha").await.unwrap();
    let b = db.create_workspace("beta", "Beta").await.unwrap();
    assert_eq!(db.workspace("alpha").await.unwrap().id, a.id);
    let edited = db
        .update_workspace(a.id, a.version, "Research A", 100, 5, 50)
        .await
        .unwrap();
    assert_eq!(edited.version, 2);
    assert!(matches!(
        db.update_workspace(a.id, a.version, "Stale", 100, 5, 50)
            .await,
        Err(Error::Conflict)
    ));
    let minted = db
        .mint_key(a.id, "runner", &[Scope::Run], None)
        .await
        .unwrap();
    assert_eq!(minted.plaintext.len(), 47);
    assert!(minted.key.permits(Scope::Read));
    assert!(minted.key.permits(Scope::Run));
    assert!(!minted.key.permits(Scope::Admin));
    let stored: Vec<u8> = sqlx::query_scalar("SELECT key_sha256 FROM bb.api_keys WHERE id=$1")
        .bind(minted.key.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(stored, Sha256::digest(minted.plaintext.as_bytes()).to_vec());
    let columns:Vec<String>=sqlx::query_scalar("SELECT column_name::text FROM information_schema.columns WHERE table_schema='bb' AND table_name='api_keys'").fetch_all(db.pool()).await.unwrap();
    assert!(
        !columns
            .iter()
            .any(|s| s == "plaintext" || s == "secret" || s == "key")
    );
    assert!(
        !serde_json::to_string(&minted.key)
            .unwrap()
            .contains("key_sha256")
    );
    let auth = db.authenticate(&minted.plaintext).await.unwrap().unwrap();
    assert_eq!(auth.id, minted.key.id);
    let usage = db.api_keys(a.id).await.unwrap()[0].last_used_at;
    db.authenticate(&minted.plaintext).await.unwrap();
    assert_eq!(db.api_keys(a.id).await.unwrap()[0].last_used_at, usage);
    assert!(db.authenticate("bbk_invalid").await.unwrap().is_none());
    db.revoke_key(a.id, minted.key.id).await.unwrap();
    assert!(db.authenticate(&minted.plaintext).await.unwrap().is_none());
    let admin = db
        .mint_key(a.id, "admin", &[Scope::Admin], None)
        .await
        .unwrap();
    assert!(admin.key.permits(Scope::Run));

    let src = db
        .register_source(NewDataSource {
            id: "test-synthetic",
            vendor: "Test",
            licence: "MIT",
            licence_url: None,
            redistribution: true,
            hostable: true,
            notes: "local fixtures",
        })
        .await
        .unwrap();
    let inst = db
        .register_instrument("synthetic", "BTCUSD", "synthetic")
        .await
        .unwrap();
    assert_eq!(
        db.register_instrument("synthetic", "BTCUSD", "synthetic")
            .await
            .unwrap()
            .id,
        inst.id
    );
    let ds = db
        .register_dataset(NewDataset {
            id: "test-data",
            owner: None,
            title: "Test data",
            kind: "synthetic",
            instrument: inst.id,
            source: &src.id,
            hosted: true,
        })
        .await
        .unwrap();
    assert_eq!(db.dataset(b.id, &ds.id).await.unwrap().id, ds.id);
    assert_eq!(db.datasets(a.id, None, 10).await.unwrap().len(), 1);
    let nv = NewDatasetVersion {
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
    };
    let version = db.register_version(&nv).await.unwrap();
    assert_eq!(db.register_version(&nv).await.unwrap().id, version.id);
    assert_eq!(db.current_version(&ds.id).await.unwrap().id, version.id);
    assert_eq!(db.dataset_version(&ds.id, 1).await.unwrap().id, version.id);
    assert_eq!(
        db.dataset_versions(&ds.id, None, 10).await.unwrap().len(),
        1
    );
    let quality = QualityReport {
        dataset_version_id: version.id,
        checker_version: 1,
        passed: true,
        error_count: 0,
        warning_count: 0,
        book_valid_fraction: 1.0,
        report: json!({"ok":true}),
        report_sha256: vec![2; 32],
        created_at: Utc::now(),
    };
    db.put_quality_report(&quality).await.unwrap();
    assert!(db.quality_report(version.id, None).await.unwrap().passed);
    let err = sqlx::query("UPDATE bb.dataset_versions SET row_count=2 WHERE id=$1")
        .bind(version.id)
        .execute(db.pool())
        .await
        .unwrap_err();
    assert!(matches!(Error::from(err), Error::Immutable));
    let c = config(&db, &version, 7).await;
    assert_eq!(config(&db, &version, 7).await.id, c.id);
    assert_eq!(db.config(c.id).await.unwrap().fingerprint, c.fingerprint);

    // Eight simultaneous identical submissions: one config, job, experiment and first label.
    let barrier = Arc::new(Barrier::new(8));
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let db = db.clone();
        let barrier = barrier.clone();
        let mut s = submission(&c, a.id);
        s.api_key_id = Some(admin.key.id);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            db.submit(&s).await.unwrap()
        }));
    }
    let mut submissions = Vec::new();
    for task in tasks {
        submissions.push(task.await.unwrap());
    }
    let job_id = submissions[0].job.as_ref().unwrap().id;
    let exp_id = submissions[0].experiment.id;
    assert_eq!(submissions.iter().filter(|s| s.created).count(), 1);
    assert!(
        submissions
            .iter()
            .all(|s| s.job.as_ref().unwrap().id == job_id && s.experiment.id == exp_id)
    );
    let mut repeat = submission(&c, a.id);
    repeat.label = "ignored label".into();
    assert_eq!(
        db.submit(&repeat).await.unwrap().experiment.label,
        "first label"
    );
    assert_eq!(db.queue_position(job_id).await.unwrap(), Some(1));
    assert!(db.next_wake(PLATFORM).await.unwrap().is_some());
    let shared = db.submit(&submission(&c, b.id)).await.unwrap();
    assert_eq!(shared.job.unwrap().id, job_id);
    assert_eq!(db.visible_job(b.id, job_id).await.unwrap().id, job_id);
    let err = sqlx::query("UPDATE bb.jobs SET state='completed' WHERE id=$1")
        .bind(job_id)
        .execute(db.pool())
        .await
        .unwrap_err();
    assert!(matches!(Error::from(err), Error::IllegalTransition));
    assert!(
        db.claim("wrong-platform", "windows-x86_64", 60)
            .await
            .unwrap()
            .is_none()
    );

    // Four competing claimers never receive the same live job.
    let barrier = Arc::new(Barrier::new(4));
    let mut tasks = Vec::new();
    for i in 0..4 {
        let db = db.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            db.claim(&format!("worker-{i}"), PLATFORM, 60)
                .await
                .unwrap()
        }));
    }
    let mut claims = Vec::new();
    for task in tasks {
        if let Some(job) = task.await.unwrap() {
            claims.push(job);
        }
    }
    assert_eq!(claims.len(), 1);
    let claimed = &claims[0];
    let lease = claimed.lease().unwrap();
    let bad = Lease {
        worker_id: "intruder".into(),
        ..lease.clone()
    };
    assert!(matches!(
        db.heartbeat(&bad, 60, 1, 1).await,
        Err(Error::LeaseLost)
    ));
    assert!(matches!(
        db.fail(&bad, "internal", "bad", "bad").await,
        Err(Error::LeaseLost)
    ));
    assert!(matches!(db.release(&bad).await, Err(Error::LeaseLost)));
    assert_eq!(db.heartbeat(&lease, 60, 3, 4).await.unwrap().ticks_done, 3);
    let (r, artifact) = result(&c);
    db.upload_intent("fs", &artifact.storage_key).await.unwrap();
    assert!(matches!(
        db.complete(&bad, &r, &artifact).await,
        Err(Error::LeaseLost)
    ));
    let absent: i64 = sqlx::query_scalar("SELECT count(*) FROM bb.runs")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(absent, 0);
    let completed = db.complete(&lease, &r, &artifact).await.unwrap();
    assert!(!completed.reused);
    assert_eq!(db.run(r.id).await.unwrap().result_sha256, r.result_sha256);
    let hashes:(Vec<u8>,Vec<u8>,Vec<u8>,Vec<u8>)=sqlx::query_as("SELECT r.result_sha256,r.summary_sha256,a.sha256,a.page_index_sha256 FROM bb.runs r JOIN bb.run_artifacts a ON a.run_id=r.id WHERE r.id=$1").bind(r.id).fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        hashes,
        (
            r.result_sha256.clone(),
            r.summary_sha256.clone(),
            artifact.sha256.clone(),
            artifact.page_index_sha256.clone()
        )
    );
    assert_eq!(
        db.artifact(r.id).await.unwrap().storage_key,
        artifact.storage_key
    );
    assert_eq!(
        db.artifact(r.id).await.unwrap().page_index,
        artifact.page_index
    );
    assert_eq!(
        db.attempts(job_id).await.unwrap()[0].outcome.as_deref(),
        Some("completed")
    );
    assert!(matches!(
        db.request_cancel(a.id, job_id, &Audit::default()).await,
        Err(Error::Conflict)
    ));
    assert!(db.gc_due(10).await.unwrap().is_empty());
    let fast = db.submit(&submission(&c, a.id)).await.unwrap();
    assert!(fast.job.is_none());
    assert_eq!(fast.run.unwrap().id, r.id);
    assert_eq!(
        db.visible_run(a.id, &c.short_id, PLATFORM)
            .await
            .unwrap()
            .id,
        r.id
    );
    assert_eq!(
        db.visible_run(b.id, &hex::encode(&c.fingerprint), PLATFORM)
            .await
            .unwrap()
            .id,
        r.id
    );
    assert_eq!(db.runs(a.id, PLATFORM, None, 10).await.unwrap().len(), 1);
    let err = sqlx::query("UPDATE bb.runs SET ticks=5 WHERE id=$1")
        .bind(r.id)
        .execute(db.pool())
        .await
        .unwrap_err();
    assert!(matches!(Error::from(err), Error::Immutable));
    let err = sqlx::query("UPDATE bb.jobs SET state='running' WHERE id=$1")
        .bind(job_id)
        .execute(db.pool())
        .await
        .unwrap_err();
    assert!(matches!(Error::from(err), Error::IllegalTransition));

    // Cancellation is per interested workspace, then cooperatively acknowledged.
    let c2 = config(&db, &version, 8).await;
    let first = db.submit(&submission(&c2, a.id)).await.unwrap();
    let j2 = first.job.unwrap();
    db.submit(&submission(&c2, b.id)).await.unwrap();
    let running = db
        .claim("cancel-worker", PLATFORM, 60)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(running.id, j2.id);
    let l2 = running.lease().unwrap();
    assert!(
        db.request_cancel(a.id, j2.id, &Audit::default())
            .await
            .unwrap()
            .detached
    );
    assert_eq!(db.job(j2.id).await.unwrap().state, "running");
    assert!(
        !db.request_cancel(b.id, j2.id, &Audit::default())
            .await
            .unwrap()
            .detached
    );
    assert_eq!(
        db.heartbeat(&l2, 60, 1, 1).await.unwrap().state,
        "cancelling"
    );
    let (r2, a2) = result(&c2);
    db.upload_intent("fs", &a2.storage_key).await.unwrap();
    assert!(matches!(
        db.complete(&l2, &r2, &a2).await,
        Err(Error::LeaseLost)
    ));
    let err = sqlx::query("UPDATE bb.jobs SET state='completed' WHERE id=$1")
        .bind(j2.id)
        .execute(db.pool())
        .await
        .unwrap_err();
    assert!(matches!(Error::from(err), Error::IllegalTransition));
    assert_eq!(db.acknowledge_cancel(&l2).await.unwrap().state, "cancelled");
    assert_eq!(
        db.request_cancel(b.id, j2.id, &Audit::default())
            .await
            .unwrap()
            .job
            .state,
        "cancelled"
    );
    assert!(db.existing_run(c2.id, PLATFORM).await.unwrap().is_none());
    let fresh = db.submit(&submission(&c2, a.id)).await.unwrap();
    assert_ne!(fresh.job.as_ref().unwrap().id, j2.id);
    let queued = fresh.job.unwrap();
    assert_eq!(
        db.request_cancel(a.id, queued.id, &Audit::default())
            .await
            .unwrap()
            .job
            .state,
        "cancelled"
    );

    // Graceful release does not consume a lease loss. Expiry does; stale tokens are fenced.
    let c3 = config(&db, &version, 9).await;
    let j3 = db
        .submit(&submission(&c3, a.id))
        .await
        .unwrap()
        .job
        .unwrap();
    let running = db
        .claim("same-worker", PLATFORM, 60)
        .await
        .unwrap()
        .unwrap();
    let old = running.lease().unwrap();
    assert_eq!(db.release(&old).await.unwrap().lease_losses, 0);
    let running = db
        .claim("same-worker", PLATFORM, 60)
        .await
        .unwrap()
        .unwrap();
    let current = running.lease().unwrap();
    assert_ne!(old.attempt_no, current.attempt_no);
    assert!(matches!(
        db.heartbeat(&old, 60, 0, 0).await,
        Err(Error::LeaseLost)
    ));
    force_lease_expired(&db, j3.id).await;
    assert!(matches!(
        db.heartbeat(&current, 60, 0, 0).await,
        Err(Error::LeaseLost)
    ));
    let (late_run, late_artifact) = result(&c3);
    db.upload_intent("fs", &late_artifact.storage_key)
        .await
        .unwrap();
    assert!(matches!(
        db.complete(&current, &late_run, &late_artifact).await,
        Err(Error::LeaseLost)
    ));
    assert!(db.existing_run(c3.id, PLATFORM).await.unwrap().is_none());
    db.reap().await.unwrap();
    assert_eq!(db.job(j3.id).await.unwrap().lease_losses, 1);
    assert!(db.claim("too-soon", PLATFORM, 60).await.unwrap().is_none());
    for loss in 2..=3 {
        runnable(&db, j3.id).await;
        let running = db.claim("retry", PLATFORM, 60).await.unwrap().unwrap();
        force_lease_expired(&db, running.id).await;
        db.reap().await.unwrap();
        assert_eq!(db.job(j3.id).await.unwrap().lease_losses, loss);
    }
    assert_eq!(
        db.job(j3.id).await.unwrap().error_category.as_deref(),
        Some("lease_expired")
    );
    assert_eq!(db.attempts(j3.id).await.unwrap().len(), 4);
    let c4 = config(&db, &version, 10).await;
    let j4 = db
        .submit(&submission(&c4, a.id))
        .await
        .unwrap()
        .job
        .unwrap();
    sqlx::query("UPDATE bb.jobs SET expires_at=now()-interval '1 second' WHERE id=$1")
        .bind(j4.id)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(db.claim("ttl", PLATFORM, 60).await.unwrap().is_none());
    db.reap().await.unwrap();
    assert_eq!(db.job(j4.id).await.unwrap().state, "expired");

    // Lists and optimistic metadata do not read object storage.
    let listed = db
        .experiments(a.id, &ExperimentFilter::default(), None, 2)
        .await
        .unwrap();
    assert_eq!(listed.len(), 2);
    let cursor = Cursor {
        created_at: listed[1].created_at,
        id: listed[1].id,
    };
    let page2 = db
        .experiments(a.id, &ExperimentFilter::default(), Some(&cursor), 100)
        .await
        .unwrap();
    assert!(page2.iter().all(|e| !listed.iter().any(|x| x.id == e.id)));
    let e = db.experiment(a.id, exp_id).await.unwrap();
    let renamed = db
        .rename_experiment(a.id, e.id, e.version, "renamed", &Audit::default())
        .await
        .unwrap();
    assert!(renamed.version > e.version);
    assert!(matches!(
        db.rename_experiment(a.id, e.id, e.version, "stale", &Audit::default())
            .await,
        Err(Error::Conflict)
    ));
    let pinned = db
        .pin_experiment(a.id, e.id, renamed.version, true)
        .await
        .unwrap();
    assert!(pinned.pinned);
    assert!(
        !db.jobs(a.id, &[], None, None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !db.queue_depth()
            .await
            .unwrap()
            .iter()
            .any(|d| d.state == "completed")
    );
    assert!(db.oldest_queued_at().await.unwrap().is_none());

    // Quarantine frees identity, preserving immutable data and emitting an alarm.
    db.quarantine_run(r.id, "test integrity failure")
        .await
        .unwrap();
    assert!(matches!(db.run(r.id).await, Err(Error::NotFound)));
    let replacement = db.submit(&submission(&c, a.id)).await.unwrap().job.unwrap();
    let rr = db
        .claim("replacement", PLATFORM, 60)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rr.id, replacement.id);
    let (replacement_run, replacement_artifact) = result(&c);
    db.upload_intent("fs", &replacement_artifact.storage_key)
        .await
        .unwrap();
    db.complete(
        &rr.lease().unwrap(),
        &replacement_run,
        &replacement_artifact,
    )
    .await
    .unwrap();

    // Concurrent identical completions: second worker loses its token, no duplicate rows.
    let c5 = config(&db, &version, 11).await;
    db.submit(&submission(&c5, a.id)).await.unwrap();
    let running = db
        .claim("complete-race", PLATFORM, 60)
        .await
        .unwrap()
        .unwrap();
    let (race_run, race_artifact) = result(&c5);
    db.upload_intent("fs", &race_artifact.storage_key)
        .await
        .unwrap();
    let l = running.lease().unwrap();
    let (left, right) = tokio::join!(
        db.complete(&l, &race_run, &race_artifact),
        db.complete(&l, &race_run, &race_artifact)
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM bb.runs WHERE experiment_config_id=$1 AND quarantined_at IS NULL",
    )
    .bind(c5.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(count, 1);

    // Completion conflicts with a separately committed canonical result: equal reuses,
    // unequal raises a durable scientific alarm and fails only the current job.
    let c6 = config(&db, &version, 12).await;
    db.submit(&submission(&c6, a.id)).await.unwrap();
    let running = db.claim("dedup", PLATFORM, 60).await.unwrap().unwrap();
    let (existing, existing_artifact) = result(&c6);
    insert_science(&db, &c6, &existing, &existing_artifact).await;
    let (duplicate, duplicate_artifact) = result(&c6);
    db.upload_intent("fs", &duplicate_artifact.storage_key)
        .await
        .unwrap();
    let dedup = db
        .complete(&running.lease().unwrap(), &duplicate, &duplicate_artifact)
        .await
        .unwrap();
    assert!(dedup.reused);
    assert_eq!(dedup.run.id, existing.id);
    let c7 = config(&db, &version, 13).await;
    db.submit(&submission(&c7, a.id)).await.unwrap();
    let running = db.claim("alarm", PLATFORM, 60).await.unwrap().unwrap();
    let (existing, existing_artifact) = result(&c7);
    insert_science(&db, &c7, &existing, &existing_artifact).await;
    let (mut conflicting, conflicting_artifact) = result(&c7);
    conflicting.result_sha256 = vec![99; 32];
    db.upload_intent("fs", &conflicting_artifact.storage_key)
        .await
        .unwrap();
    assert!(matches!(
        db.complete(
            &running.lease().unwrap(),
            &conflicting,
            &conflicting_artifact
        )
        .await,
        Err(Error::Nondeterminism)
    ));
    assert_eq!(
        db.job(running.id).await.unwrap().error_code.as_deref(),
        Some("nondeterminism_detected")
    );
    assert_eq!(
        db.events(None, Some("nondeterminism_detected"), None, 10)
            .await
            .unwrap()
            .len(),
        1
    );

    lease_and_cancel_edges(&db, a.id, &version).await;
    research_and_operations(&db, a.id, b.id, &version, &replacement_run, &race_run).await;
    // Retirement is the only mutable scientific input property.
    db.retire_version(version.id).await.unwrap();
    assert!(matches!(
        db.current_version(&version.dataset_id).await,
        Err(Error::NotFound)
    ));
}

async fn insert_science(db: &Database, c: &ExperimentConfig, r: &NewRun, a: &NewArtifact) {
    // Real SQL inserts model an independently committed winner, not a mocked repository.
    sqlx::query("INSERT INTO bb.runs (id,experiment_config_id,platform,blackbox_version,build_profile,worker_id,result_sha256,summary_sha256,ticks,order_count,fill_count,tape_len,equity_len,wall_time_ms,metrics,engine_check,is_canonical) VALUES ($1,$2,$3,'test','release','other',$4,$5,10,2,1,3,2,1,$6,$7,false)").bind(r.id).bind(c.id).bind(PLATFORM).bind(&r.result_sha256).bind(&r.summary_sha256).bind(&r.metrics).bind(&r.engine_check).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO bb.run_artifacts (run_id,kind,format_version,storage_backend,storage_key,sha256,size_bytes,uncompressed_bytes,page_index,page_index_sha256) VALUES ($1,'bundle',1,'fs',$2,$3,100,200,$4,$5)").bind(r.id).bind(&a.storage_key).bind(&a.sha256).bind(&a.page_index).bind(&a.page_index_sha256).execute(db.pool()).await.unwrap();
}

async fn research_and_operations(
    db: &Database,
    a: Uuid,
    b: Uuid,
    v: &DatasetVersion,
    r1: &NewRun,
    r2: &NewRun,
) {
    let sweep = db
        .create_sweep(
            &NewSweep {
                workspace_id: a,
                spec: json!({"x":{"type":"order_latency_ns"},"y":{"type":"queue_model"}}),
                spec_fingerprint: vec![9; 32],
                x_len: 1,
                y_len: 1,
                label: "grid".into(),
                created_by_key_id: None,
            },
            false,
        )
        .await
        .unwrap();
    let c = config(db, v, 30).await;
    let mut s = submission(&c, a);
    s.sweep = Some((sweep.id, 0, 0));
    let cell = db.submit(&s).await.unwrap();
    assert_eq!(db.sweep_cells(a, sweep.id).await.unwrap().len(), 1);
    assert_eq!(db.sweeps(a, None, 10).await.unwrap().len(), 1);
    let edited = db
        .rename_sweep(a, sweep.id, sweep.version, "renamed grid")
        .await
        .unwrap();
    assert!(edited.version > sweep.version);
    db.submit(&submission(&c, b)).await.unwrap();
    db.cancel_sweep(a, sweep.id).await.unwrap();
    assert_eq!(db.job(cell.job.unwrap().id).await.unwrap().state, "queued");
    let comp = db
        .create_comparison(
            &NewComparison {
                workspace_id: a,
                run_a_id: r1.id,
                run_b_id: r2.id,
                comparator_version: 2,
                eligibility: "aligned".into(),
                reasons: json!([]),
                summary: json!({"same":true}),
                summary_sha256: vec![8; 32],
                label: "pair".into(),
                created_by_key_id: None,
            },
            &Audit::default(),
        )
        .await
        .unwrap();
    assert_eq!(db.comparison(a, comp.id).await.unwrap().id, comp.id);
    assert_eq!(db.comparisons(a, None, 10).await.unwrap().len(), 1);
    db.rename_comparison(a, comp.id, comp.version, "renamed pair", &Audit::default())
        .await
        .unwrap();
    let body = "# Research\n";
    let report = db
        .save_report(
            &NewReport {
                workspace_id: a,
                kind: "comparison".into(),
                run_id: None,
                comparison_id: Some(comp.id),
                sweep_id: None,
                title: "Saved research".into(),
                generator_version: 1,
                body: body.into(),
                body_sha256: Sha256::digest(body.as_bytes()).to_vec(),
                created_by_key_id: None,
            },
            &Audit::default(),
        )
        .await
        .unwrap();
    assert_eq!(db.report(a, report.id).await.unwrap().body, body);
    assert_eq!(db.reports(a, None, 10).await.unwrap().len(), 1);
    db.delete_comparison(a, comp.id, &Audit::default())
        .await
        .unwrap();
    assert!(
        db.report(a, report.id)
            .await
            .unwrap()
            .comparison_id
            .is_none()
    );
    db.delete_report(a, report.id, &Audit::default())
        .await
        .unwrap();
    db.audit(NewAuditEvent {
        workspace: Some(a),
        key: None,
        action: "experiment.create",
        outcome: "ok",
        target_type: Some("experiment"),
        target_id: Some("test"),
        request_id: Some("request-1234"),
        client_ip: Some("127.0.0.1"),
        detail: &json!({}),
    })
    .await
    .unwrap();
    assert_eq!(
        db.audit_events(a, None, 10).await.unwrap()[0]
            .client_ip
            .as_deref(),
        Some("127.0.0.1")
    );
    db.emit_event(NewSystemEvent {
        level: "info",
        kind: "test_event",
        worker: Some("test-worker"),
        job: None,
        run: None,
        message: "test",
        detail: &json!({}),
    })
    .await
    .unwrap();
    assert!(!db.events(None, None, None, 100).await.unwrap().is_empty());
    let worker = WorkerHeartbeat {
        worker_id: "test-worker".into(),
        hostname: "local".into(),
        pid: 1,
        blackbox_version: "0.2.0".into(),
        platform: PLATFORM.into(),
        mode: "standalone".into(),
        slots: 1,
        state: "idle".into(),
        current_job_id: None,
        heartbeat_interval_ms: 15000,
        started_at: Utc::now(),
        last_heartbeat_at: Utc::now(),
        stopped_at: None,
    };
    db.worker_heartbeat(&worker).await.unwrap();
    let mut stopped = worker;
    stopped.state = "stopped".into();
    assert!(
        db.worker_heartbeat(&stopped)
            .await
            .unwrap()
            .stopped_at
            .is_some()
    );
    assert_eq!(db.workers().await.unwrap().len(), 1);
    db.record_storage_read("fs").await.unwrap();
    assert!(db.storage_usage().await.unwrap()[0].uploads > 0);
    db.upload_intent("fs", "orphan").await.unwrap();
    sqlx::query(
        "UPDATE bb.blob_gc SET not_before=now()-interval '1 second' WHERE storage_key='orphan'",
    )
    .execute(db.pool())
    .await
    .unwrap();
    assert!(
        db.gc_due(100)
            .await
            .unwrap()
            .iter()
            .any(|g| g.storage_key == "orphan")
    );
    db.gc_failed("fs", "orphan", "test error").await.unwrap();
    db.gc_ack("fs", "orphan").await.unwrap();
    db.delete_sweep(a, sweep.id).await.unwrap();
    let e = db
        .experiments(
            b,
            &ExperimentFilter {
                dataset_id: Some(v.dataset_id.clone()),
                ..Default::default()
            },
            None,
            100,
        )
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.experiment_config_id == c.id)
        .unwrap();
    db.delete_experiment(b, e.id, Some(e.version), &Audit::default())
        .await
        .unwrap();
    assert!(db.existing_run(c.id, PLATFORM).await.unwrap().is_none());
}

async fn lease_and_cancel_edges(db: &Database, workspace: Uuid, v: &DatasetVersion) {
    // Prerequisites, deterministic failure and per-attempt deadline are separate legal edges.
    let c = config(db, v, 40).await;
    let mut s = submission(&c, workspace);
    s.prerequisites_ready = false;
    let j = db.submit(&s).await.unwrap().job.unwrap();
    assert_eq!(j.state, "created");
    assert!(db.claim("pending", PLATFORM, 60).await.unwrap().is_none());
    db.queue_created(j.id).await.unwrap();
    let running = db.claim("failure", PLATFORM, 60).await.unwrap().unwrap();
    assert_eq!(
        db.fail(
            &running.lease().unwrap(),
            "budget_exceeded",
            "max_ticks",
            "budget reached"
        )
        .await
        .unwrap()
        .state,
        "failed"
    );
    let fresh = db
        .submit(&submission(&c, workspace))
        .await
        .unwrap()
        .job
        .unwrap();
    assert_ne!(fresh.id, j.id);
    let running = db.claim("deadline", PLATFORM, 60).await.unwrap().unwrap();
    assert_eq!(
        db.expire_attempt(&running.lease().unwrap(), "per-attempt deadline exceeded")
            .await
            .unwrap()
            .state,
        "expired"
    );

    let c = config(db, v, 41).await;
    let mut s = submission(&c, workspace);
    s.prerequisites_ready = false;
    let j = db.submit(&s).await.unwrap().job.unwrap();
    assert_eq!(
        db.request_cancel(workspace, j.id, &Audit::default())
            .await
            .unwrap()
            .job
            .state,
        "cancelled"
    );
    let c = config(db, v, 42).await;
    db.submit(&submission(&c, workspace)).await.unwrap();
    let running = db
        .claim("cancel-expiry", PLATFORM, 60)
        .await
        .unwrap()
        .unwrap();
    db.request_cancel(workspace, running.id, &Audit::default())
        .await
        .unwrap();
    let bad = Lease {
        worker_id: "nonholder".into(),
        ..running.lease().unwrap()
    };
    assert!(matches!(
        db.acknowledge_cancel(&bad).await,
        Err(Error::LeaseLost)
    ));
    force_lease_expired(db, running.id).await;
    db.reap().await.unwrap();
    assert_eq!(db.job(running.id).await.unwrap().state, "cancelled");

    // A real cancellation/completion race has exactly one legal outcome and no orphan run row.
    let c = config(db, v, 43).await;
    db.submit(&submission(&c, workspace)).await.unwrap();
    let running = db
        .claim("cancel-race", PLATFORM, 60)
        .await
        .unwrap()
        .unwrap();
    let lease = running.lease().unwrap();
    let (r, a) = result(&c);
    db.upload_intent("fs", &a.storage_key).await.unwrap();
    let audit = Audit::default();
    let (cancel, complete) = tokio::join!(
        db.request_cancel(workspace, running.id, &audit),
        db.complete(&lease, &r, &a)
    );
    match (cancel, complete) {
        (Err(Error::Conflict), Ok(_)) => {
            assert_eq!(db.job(running.id).await.unwrap().state, "completed")
        }
        (Ok(cancelled), Err(Error::LeaseLost)) => {
            assert_eq!(cancelled.job.state, "cancelling");
            db.acknowledge_cancel(&lease).await.unwrap();
            assert!(db.existing_run(c.id, PLATFORM).await.unwrap().is_none());
        }
        unexpected => panic!("invalid cancel/complete race result: {unexpected:?}"),
    }
    let c = config(db, v, 44).await;
    db.submit(&submission(&c, workspace)).await.unwrap();
    let running = db.claim("existing", PLATFORM, 60).await.unwrap().unwrap();
    let (r, a) = result(&c);
    insert_science(db, &c, &r, &a).await;
    assert_eq!(
        db.complete_existing(&running.lease().unwrap(), r.id)
            .await
            .unwrap()
            .id,
        r.id
    );
    // Artifact deletion triggers enqueue the object, and two GC claimers cannot share a row.
    sqlx::query("DELETE FROM bb.run_artifacts WHERE run_id=$1")
        .bind(r.id)
        .execute(db.pool())
        .await
        .unwrap();
    let mut batch = db.claim_gc(100).await.unwrap();
    assert!(
        batch
            .rows()
            .iter()
            .any(|g| g.storage_key == a.storage_key && g.reason == "artifact_deleted")
    );
    let other = db.claim_gc(100).await.unwrap();
    assert!(other.rows().iter().all(|g| {
        !batch
            .rows()
            .iter()
            .any(|x| x.storage_key == g.storage_key && x.storage_backend == g.storage_backend)
    }));
    other.commit().await.unwrap();
    batch.acknowledge("fs", &a.storage_key).await.unwrap();
    batch.commit().await.unwrap();
}
