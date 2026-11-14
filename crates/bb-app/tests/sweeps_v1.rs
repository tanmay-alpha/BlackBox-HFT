//! `/api/v1/sweeps` end to end against a real Postgres (`BLACKBOX_TEST_DATABASE_URL`) and the
//! in-process worker: a partitioned grid and a 3-seed ensemble (AT-12) run to completion through
//! the common job queue, their reports are checked for shape, the sweep budget refuses before
//! anything is stored, a resubmission is idempotent, a legacy `GridSpec` body still works, and
//! the derived slice datasets rebuild from their provenance alone.

mod common;

use std::time::Duration;

use bb_db::Scope;
use common::*;
use serde_json::{Value, json};

macro_rules! db_or_skip {
    () => {
        match TestDb::create().await {
            Some(db) => db,
            None => return,
        }
    };
}

/// Polls a sweep until every run is terminal.
async fn wait_sweep(server: &Server, key: &str, id: &str) -> Value {
    for _ in 0..1200 {
        let (status, _, sweep) = server.get(&format!("/api/v1/sweeps/{id}"), Some(key)).await;
        assert_eq!(status, 200, "{sweep}");
        if sweep["report_ready"] == true {
            return sweep;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("sweep {id} did not finish");
}

fn job_ids(sweep: &Value) -> Vec<String> {
    let mut ids: Vec<String> = sweep["cells"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["runs"].as_array().unwrap().iter())
        .filter_map(|r| r["job_id"].as_str().map(String::from))
        .collect();
    ids.sort();
    ids
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn partitioned_grid_seed_ensemble_budget_and_idempotency() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            slots: 2,
            tweak: Box::new(|c| c.max_sweep_cells = 6),
            ..Options::default()
        },
    )
    .await;
    let manifest = register_dataset(&server.db, dir.path(), 20).await;
    let (_, key) = workspace_key(&server.db, "sweeper", &[Scope::Run]).await;
    let base = config_json(&manifest, 100, 42);
    let s = manifest.start;
    let split = s + 12_000_000_000;
    let end = manifest.end + 1;

    // ----- a 2 × 2 grid with an in-sample / out-of-sample partition and CSCV -----
    let grid = json!({
        "base": base,
        "design": {"type": "grid", "axes": [
            {"type": "order_latency_ns", "values": ["1000000", "20000000"]},
            {"type": "maker_fee", "values": [0.0, 0.0002]}
        ]},
        "partition": {
            "in_sample": {"start": s.to_string(), "end": split.to_string()},
            "out_of_sample": {"start": split.to_string(), "end": end.to_string()}
        },
        "objective": {"metric": "net_pnl", "direction": "maximize"},
        "cscv_blocks": 4
    });
    let (status, _, b) = server
        .post(
            "/api/v1/sweeps",
            Some(&key),
            &json!({"spec": grid, "label": "grid"}),
        )
        .await;
    assert_eq!(status, 202, "{b}");
    assert_eq!(b["created"], true);
    let grid_id = b["sweep"]["id"].as_str().unwrap().to_string();
    assert_eq!(b["sweep"]["design"], "grid");
    assert_eq!(b["sweep"]["progress"]["cells_total"], 4);
    assert_eq!(b["sweep"]["progress"]["runs_total"], 8);
    assert_eq!(b["sweep"]["axes"].as_array().unwrap().len(), 2);
    assert_eq!(
        b["sweep"]["partition"]["in_sample"]["end"],
        json!(split.to_string()),
        "windows travel as ns strings"
    );
    let first_jobs = job_ids(&b["sweep"]);
    assert_eq!(first_jobs.len(), 8, "every run queued: {b}");
    for cell in b["sweep"]["cells"].as_array().unwrap() {
        let segments: Vec<&str> = cell["runs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["segment"].as_str().unwrap())
            .collect();
        assert_eq!(segments, ["in_sample", "out_of_sample"]);
        assert_eq!(cell["coords"].as_array().unwrap().len(), 2);
    }
    assert_wire_safe(&b, "$");

    // Idempotent resubmission: same sweep, no new job.
    let (status, _, again) = server
        .post("/api/v1/sweeps", Some(&key), &json!({"spec": grid}))
        .await;
    assert_eq!(status, 202, "{again}");
    assert_eq!(again["created"], false);
    assert_eq!(again["sweep"]["id"], json!(grid_id));
    assert_eq!(job_ids(&again["sweep"]), first_jobs);

    // ----- a 3-seed ensemble under constant latency -----
    let seeds = json!({
        "base": base,
        "design": {"type": "seeds", "count": 3, "base_seed": "7"},
    });
    let (status, _, b) = server
        .post("/api/v1/sweeps", Some(&key), &json!({"spec": seeds}))
        .await;
    assert_eq!(status, 202, "{b}");
    let seeds_id = b["sweep"]["id"].as_str().unwrap().to_string();
    assert_eq!(b["sweep"]["design"], "seeds");
    assert_eq!(b["sweep"]["label"], "3 seeds");
    assert_eq!(b["sweep"]["spec"]["design"]["base_seed"], "7");
    let seed_params: Vec<&str> = b["sweep"]["cells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["params"][0]["value"].as_str().unwrap())
        .collect();
    assert_eq!(seed_params, ["7", "8", "9"]);

    // ----- budget refusal: more configurations than the server admits, nothing stored -----
    let (_, _, before) = server.get("/api/v1/sweeps", Some(&key)).await;
    let too_many = json!({"base": base, "design": {"type": "seeds", "count": 7}});
    let (status, _, b) = server
        .post("/api/v1/sweeps", Some(&key), &json!({"spec": too_many}))
        .await;
    assert_eq!(status, 422, "{b}");
    assert_eq!(b["error"]["code"], "budget_exceeded");
    assert_eq!(b["error"]["details"]["what"], "sweep_cells");
    assert_eq!(b["error"]["details"]["limit"], 6);
    assert_eq!(b["error"]["details"]["estimate"], 7);
    // An invalid spec is refused as invalid_config.
    let bad = json!({"base": base, "design": {"type": "seeds", "seeds": ["1", "1"]}});
    let (status, _, b) = server
        .post("/api/v1/sweeps", Some(&key), &json!({"spec": bad}))
        .await;
    assert_eq!(status, 422, "{b}");
    assert_eq!(b["error"]["code"], "invalid_config");
    let (_, _, after) = server.get("/api/v1/sweeps", Some(&key)).await;
    assert_eq!(after["items"], before["items"], "refusals store nothing");

    // ----- a legacy GridSpec body is converted to a grid sweep -----
    let legacy = json!({
        "base": base,
        "x": {"type": "order_latency_ns", "values": ["1000000", "5000000"]},
        "y": {"type": "taker_fee", "values": [0.0004]}
    });
    let (status, _, b) = server
        .post("/api/v1/sweeps", Some(&key), &json!({"spec": legacy}))
        .await;
    assert_eq!(status, 202, "{b}");
    let legacy_id = b["sweep"]["id"].as_str().unwrap().to_string();
    assert_eq!(b["sweep"]["design"], "grid");
    assert_eq!(b["sweep"]["objective"]["metric"], "net_pnl");
    assert_eq!(b["sweep"]["progress"]["runs_total"], 2);

    // ----- reports -----
    let done = wait_sweep(&server, &key, &grid_id).await;
    assert_eq!(done["state"], "completed", "{done}");
    let (status, _, r) = server
        .get(&format!("/api/v1/sweeps/{grid_id}/report"), Some(&key))
        .await;
    assert_eq!(status, 200, "{r}");
    assert_wire_safe(&r, "$");
    assert!(r["ensemble"].is_null());
    let a = &r["analysis"];
    assert_eq!(a["configurations"]["tried"], 4);
    assert_eq!(a["configurations"]["completed_in_sample"], 4);
    assert_eq!(a["configurations"]["completed_out_of_sample"], 4);
    assert_eq!(a["has_out_of_sample"], true);
    let selected = a["selected"]["cell"].as_u64().unwrap();
    assert!(selected < 4);
    assert!(a["selected"]["out_of_sample"].is_number(), "{a}");
    assert!(a["selected"]["degradation"].is_number(), "{a}");
    let rc = a["rank_correlation"]["status"].as_str().unwrap();
    assert!(
        rc == "available" && a["rank_correlation"]["spearman_rho"].is_number()
            || rc == "unavailable" && a["rank_correlation"]["reason"].is_string(),
        "{a}"
    );
    assert_eq!(a["pbo"]["status"], "available", "{a}");
    let pbo = a["pbo"]["pbo"].as_f64().unwrap();
    assert!((0.0..=1.0).contains(&pbo));
    assert_eq!(a["pbo"]["combinations"], 6);
    let codes: Vec<&str> = a["caveats"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"selection_bias") && codes.contains(&"cscv_assumptions"));
    let cells = r["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 4);
    assert!(
        cells
            .iter()
            .all(|c| c["in_sample_run_id"].is_string() && c["out_of_sample_run_id"].is_string())
    );
    let md = server
        .http
        .get(server.url(&format!("/api/v1/sweeps/{grid_id}/report?format=markdown")))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert_eq!(md.status(), 200);
    assert!(
        md.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/markdown")
    );
    let text = md.text().await.unwrap();
    assert!(text.contains("PBO (CSCV, 4 blocks") && text.contains("Selected cell"));

    // The in-sample and out-of-sample runs ran on registered slice datasets that rebuild from
    // their provenance alone (a fresh registry with no files).
    let is_run = cells[0]["in_sample_run_id"].as_str().unwrap();
    let (_, _, run) = server
        .get(&format!("/api/v1/runs/{is_run}"), Some(&key))
        .await;
    let slice_id = run["dataset"]["id"]
        .as_str()
        .or_else(|| run["config"]["dataset_id"].as_str())
        .or_else(|| run["dataset_id"].as_str())
        .unwrap_or_else(|| panic!("run without dataset id: {run}"))
        .to_string();
    assert!(slice_id.starts_with("test-synth-w-"), "{slice_id}");
    let version = server.db.current_version(&slice_id).await.unwrap();
    assert_eq!(version.storage, "regenerate");
    let fresh = tempfile::tempdir().unwrap();
    let rebuilt = bb_app::registry::DatasetRegistry::new(fresh.path())
        .load(&version)
        .await
        .expect("slice rebuilds from its parent's generator");
    assert_eq!(
        rebuilt.manifest().events_sha256,
        hex::encode(&version.events_sha256)
    );

    let done = wait_sweep(&server, &key, &seeds_id).await;
    assert_eq!(done["state"], "completed", "{done}");
    let (status, _, r) = server
        .get(&format!("/api/v1/sweeps/{seeds_id}/report"), Some(&key))
        .await;
    assert_eq!(status, 200, "{r}");
    assert_wire_safe(&r, "$");
    assert!(r["analysis"].is_null());
    let e = &r["ensemble"];
    assert_eq!(e["seed_count"], 3);
    assert_eq!(e["seed_affects_result"], false);
    assert_eq!(e["collapsed"], true);
    assert!(
        e["collapse_reason"]
            .as_str()
            .unwrap()
            .contains("collapse to one result")
    );
    let seg = &e["segments"][0];
    assert_eq!(seg["segment"], "full");
    assert_eq!(seg["completed"], 3);
    assert_eq!(seg["identical"], true);
    let members = seg["members"].as_array().unwrap();
    let seeds_seen: Vec<&str> = members
        .iter()
        .map(|m| m["seed"].as_str().unwrap())
        .collect();
    assert_eq!(seeds_seen, ["7", "8", "9"]);
    let mut run_ids: Vec<&str> = members
        .iter()
        .map(|m| m["run_id"].as_str().unwrap())
        .collect();
    run_ids.dedup();
    assert_eq!(run_ids.len(), 3, "one run per seed");
    let pnl = seg["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["metric"] == "net_pnl")
        .unwrap();
    assert_eq!(pnl["n"], 3);
    assert_eq!(pnl["std_dev"], 0.0);
    assert_eq!(pnl["min"], pnl["max"]);

    let done = wait_sweep(&server, &key, &legacy_id).await;
    assert_eq!(done["state"], "completed", "{done}");
    let (status, _, r) = server
        .get(&format!("/api/v1/sweeps/{legacy_id}/report"), Some(&key))
        .await;
    assert_eq!(status, 200, "{r}");
    assert_eq!(r["analysis"]["pbo"]["status"], "unavailable");
    assert!(r["analysis"]["pbo"]["reason"].is_string());

    // A saved research report of a sweep uses the same analysis.
    let (status, _, saved) = server
        .post(
            "/api/v1/reports",
            Some(&key),
            &json!({"kind": "sweep", "subject_id": seeds_id, "title": "ensemble"}),
        )
        .await;
    assert_eq!(status, 201, "{saved}");

    server.stop().await;
    tdb.drop_db().await;
}
