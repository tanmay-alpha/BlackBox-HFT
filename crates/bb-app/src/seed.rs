//! `blackbox demo seed`: makes the public demo workspace show real results (idempotent).
//!
//! 1. Generates the deterministic synthetic dataset and the synthetic stress datasets of
//!    `execution_validation` (`demos::stress_datasets`) and registers each as a dataset version
//!    (`storage = 'regenerate'`, events sha256 recorded, quality report stored).
//! 2. Computes and stores their market analytics.
//! 3. Submits every canonical demo experiment (`demos::demos`) and `demos::execution_validation`
//!    into the `demo` workspace through the normal admission path and job queue, pinned and
//!    labelled `<demo id>: <run label>`. A worker of the result platform computes them;
//!    re-running the command creates nothing new. With `wait`, this process computes them itself
//!    first.
//! 4. Once a demo's runs are completed: stores its curated comparisons (`demos::curated_pairs`)
//!    and one saved report (of its first comparison) in the `demo` workspace, through the same
//!    code as `POST /comparisons` and `POST /reports`. Anonymous visitors only read them; nothing
//!    about anonymous write protection changes. Missing pieces are reported as pending; run the
//!    command again after the worker finished.

use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use bb_data::synthetic::SyntheticConfig;
use uuid::Uuid;

use crate::{
    admission::{self, SubmitOptions},
    analytics,
    config::Profile,
    registry,
    runtime::Runtime,
    v1::{comparisons, reports},
    worker,
};

pub struct SeedReport {
    pub dataset_id: String,
    pub version: i32,
    pub events_sha256: String,
    /// (label, run id, state)
    pub experiments: Vec<(String, String, Option<String>)>,
    /// Curated comparisons and saved reports, created, already present or pending.
    pub curated: Vec<Curated>,
}

/// One curated demo object.
pub struct Curated {
    /// `comparison` or `report`.
    pub kind: &'static str,
    pub title: String,
    /// `created`, `existing` or `pending`.
    pub status: &'static str,
    /// The stored id, or why it is pending.
    pub detail: String,
}

/// Registers the synthetic dataset `config` and stores its market analytics (best effort).
async fn register(
    runtime: &Runtime,
    config: &SyntheticConfig,
) -> Result<(bb_db::DatasetVersion, bb_core::dataset::DatasetManifest)> {
    let db = &runtime.db;
    let hosted = runtime.cfg.profile == Profile::Hosted;
    let (version, ctx) = registry::register_synthetic(db, &runtime.registry, config, hosted)
        .await
        .map_err(|e| anyhow!("registering the synthetic dataset {}: {e}", config.id))?;
    let manifest = ctx.manifest().clone();

    let version_no = version.version_no;
    let market_ctx = ctx.clone();
    match tokio::task::spawn_blocking(move || analytics::dataset_analytics(&market_ctx, version_no))
        .await
    {
        Ok(Ok(report)) => {
            let encoded = analytics::encode(&report).map_err(|e| anyhow!("{e}"))?;
            db.put_dataset_analytics(
                version.id,
                bb_analytics::ANALYTICS_VERSION,
                &encoded.json,
                &encoded.sha256,
            )
            .await
            .map_err(|e| anyhow!("storing dataset analytics: {e}"))?;
        }
        Ok(Err(e)) => tracing::warn!(error = %e, "market analytics unavailable"),
        Err(e) => tracing::warn!(error = %e, "market analytics task failed"),
    }
    Ok((version, manifest))
}

pub async fn seed(runtime: &Runtime, platform: &str, wait: bool) -> Result<SeedReport> {
    let db = &runtime.db;
    let (version, manifest) = register(runtime, &SyntheticConfig::default()).await?;
    let mut stress = Vec::new();
    for config in crate::demos::stress_datasets() {
        stress.push(register(runtime, &config).await?.1);
    }

    let demo = db
        .workspace("demo")
        .await
        .map_err(|e| anyhow!("demo workspace: {e}"))?;
    let mut demos = crate::demos::demos(&manifest);
    demos.extend(crate::demos::execution_validation(|id| {
        std::iter::once(&manifest)
            .chain(&stress)
            .find(|m| m.id == id)
    }));
    let mut experiments = Vec::new();
    let mut open_jobs = Vec::new();
    for d in &demos {
        for run in &d.runs {
            let label = format!("{}: {}", d.id, run.label);
            let admitted = admission::admit(
                db,
                demo.id,
                runtime.cfg.profile,
                &runtime.cfg.limits,
                run.config.clone(),
            )
            .await
            .map_err(|e| anyhow!("{label}: {} ({})", e.message, e.code))?;
            let submission = admission::submit(
                db,
                &runtime.hub,
                &runtime.metrics,
                &admitted,
                SubmitOptions {
                    workspace: demo.id,
                    key: None,
                    label: &label,
                    platform,
                    priority: 10,
                    deadline_ms: runtime.cfg.deadline_ms,
                    sweep: None,
                    bypass_quotas: true,
                    storage_backend: runtime.storage.backend,
                },
            )
            .await
            .map_err(|e| anyhow!("{label}: {} ({})", e.message, e.code))?;
            if !submission.experiment.pinned {
                let _ = db
                    .pin_experiment(
                        demo.id,
                        submission.experiment.id,
                        submission.experiment.version,
                        true,
                    )
                    .await;
            }
            let state = match (&submission.run, &submission.job) {
                (Some(_), _) => "completed".to_string(),
                (None, Some(job)) => {
                    open_jobs.push(job.id);
                    job.state.clone()
                }
                (None, None) => "unknown".to_string(),
            };
            experiments.push((label, admitted.config_row.short_id.clone(), Some(state)));
        }
    }

    if wait && !open_jobs.is_empty() {
        compute(runtime, platform, &open_jobs).await?;
        for (_, _, state) in &mut experiments {
            *state = Some("completed".into());
        }
    }

    let curated = curate(runtime, demo.id, platform, &demos, &experiments).await?;
    Ok(SeedReport {
        dataset_id: manifest.id.clone(),
        version: version.version_no,
        events_sha256: manifest.events_sha256.clone(),
        experiments,
        curated,
    })
}

/// Runs this process's worker until every job in `jobs` completed.
async fn compute(runtime: &Runtime, platform: &str, jobs: &[Uuid]) -> Result<()> {
    if runtime.hub.slots == 0 {
        bail!("this runtime has no worker slot");
    }
    if platform != runtime.cfg.worker_platform {
        bail!(
            "--wait computes runs of this machine's platform ({}) only, not {platform}",
            runtime.cfg.worker_platform
        );
    }
    let worker = tokio::spawn(worker::run(runtime.worker_ctx(), true));
    runtime.hub.submitted();
    let outcome = async {
        loop {
            let mut open = 0;
            for id in jobs {
                let job = runtime
                    .db
                    .job(*id)
                    .await
                    .map_err(|e| anyhow!("job {id}: {e}"))?;
                match job.state.as_str() {
                    "completed" => {}
                    "failed" | "cancelled" | "expired" => {
                        bail!("demo job {id} ended {}", job.state)
                    }
                    _ => open += 1,
                }
            }
            if open == 0 {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    .await;
    runtime.hub.start_drain();
    let _ = worker.await;
    outcome
}

/// At most 100 characters (label/title rule, SEC-7).
fn bounded_text(text: String) -> String {
    if text.chars().count() <= 100 {
        text
    } else {
        text.chars().take(99).chain(['…']).collect()
    }
}

/// Stores the curated comparisons and saved reports of every demo whose runs are completed.
async fn curate(
    runtime: &Runtime,
    workspace: Uuid,
    platform: &str,
    demos: &[crate::api::Demo],
    experiments: &[(String, String, Option<String>)],
) -> Result<Vec<Curated>> {
    let state = runtime.app_state();
    let db = &runtime.db;
    let saved = db
        .reports(workspace, None, 100)
        .await
        .map_err(|e| anyhow!("listing demo reports: {e}"))?;
    let mut out = Vec::new();
    for d in demos {
        let mut views = Vec::new();
        for run in &d.runs {
            let label = format!("{}: {}", d.id, run.label);
            let (_, id, _) = experiments
                .iter()
                .find(|(l, _, _)| *l == label)
                .ok_or_else(|| anyhow!("{label} was not submitted"))?;
            views.push(match db.run_view(workspace, id, platform).await {
                Ok(view) => Some(view),
                Err(bb_db::Error::NotFound) => None,
                Err(e) => return Err(anyhow!("{label}: {e}")),
            });
        }
        for (k, (i, j)) in crate::demos::curated_pairs(d).into_iter().enumerate() {
            let title = bounded_text(format!(
                "{}: {} vs {}",
                d.id, d.runs[i].label, d.runs[j].label
            ));
            let (Some(a), Some(b)) = (&views[i], &views[j]) else {
                out.push(Curated {
                    kind: "comparison",
                    title,
                    status: "pending",
                    detail: "waiting for both runs to complete".into(),
                });
                if k == 0 {
                    out.push(Curated {
                        kind: "report",
                        title: bounded_text(format!("{}: curated comparison report", d.title)),
                        status: "pending",
                        detail: "waiting for the comparison".into(),
                    });
                }
                continue;
            };
            let audit = bb_db::Audit {
                workspace: Some(workspace),
                ..Default::default()
            };
            let (row, existed) =
                comparisons::compare_and_store(&state, workspace, a, b, title.clone(), &audit)
                    .await
                    .map_err(|e| anyhow!("{title}: {} ({})", e.message, e.code))?;
            out.push(Curated {
                kind: "comparison",
                title: row.label.clone(),
                status: if existed { "existing" } else { "created" },
                detail: format!("{} ({})", row.id, row.eligibility),
            });
            if k != 0 {
                continue;
            }
            let report_title = bounded_text(format!("{}: curated comparison report", d.title));
            match saved
                .iter()
                .find(|r| r.kind == "comparison" && r.comparison_id == Some(row.id))
            {
                Some(r) => out.push(Curated {
                    kind: "report",
                    title: r.title.clone(),
                    status: "existing",
                    detail: r.id.to_string(),
                }),
                None => {
                    let snapshot =
                        reports::comparison_snapshot(&state, workspace, &row, report_title.clone())
                            .await
                            .map_err(|e| anyhow!("{report_title}: {}", e.message))?;
                    let r = reports::store(&state, snapshot, &audit)
                        .await
                        .map_err(|e| anyhow!("{report_title}: {} ({})", e.message, e.code))?;
                    out.push(Curated {
                        kind: "report",
                        title: r.title,
                        status: "created",
                        detail: r.id.to_string(),
                    });
                }
            }
        }
    }
    Ok(out)
}
