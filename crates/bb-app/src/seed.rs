//! `blackbox demo seed`: makes the public demo workspace show real results (idempotent).
//!
//! 1. Generates the deterministic synthetic dataset and registers it as a dataset version
//!    (`storage = 'regenerate'`, events sha256 recorded, quality report stored).
//! 2. Computes and stores its market analytics.
//! 3. Submits every canonical demo experiment (`demos::demos`) into the `demo` workspace
//!    through the normal admission path and job queue, pinned and labelled
//!    `<demo id>: <run label>`. A worker of the result platform computes them; re-running the
//!    command creates nothing new.

use anyhow::{Result, anyhow};
use bb_data::synthetic::SyntheticConfig;

use crate::{
    admission::{self, SubmitOptions},
    analytics,
    config::Profile,
    registry,
    runtime::Runtime,
};

pub struct SeedReport {
    pub dataset_id: String,
    pub version: i32,
    pub events_sha256: String,
    pub experiments: Vec<(String, String, Option<String>)>,
}

pub async fn seed(runtime: &Runtime, platform: &str) -> Result<SeedReport> {
    let db = &runtime.db;
    let hosted = runtime.cfg.profile == Profile::Hosted;
    let (version, ctx) =
        registry::register_synthetic(db, &runtime.registry, &SyntheticConfig::default(), hosted)
            .await
            .map_err(|e| anyhow!("registering the synthetic dataset: {e}"))?;
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

    let demo = db
        .workspace("demo")
        .await
        .map_err(|e| anyhow!("demo workspace: {e}"))?;
    let mut experiments = Vec::new();
    for d in crate::demos::demos(&manifest) {
        for run in d.runs {
            let label = format!("{}: {}", d.id, run.label);
            let admitted = admission::admit(
                db,
                demo.id,
                runtime.cfg.profile,
                &runtime.cfg.limits,
                run.config,
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
                (None, Some(job)) => job.state.clone(),
                (None, None) => "unknown".to_string(),
            };
            experiments.push((label, admitted.config_row.short_id.clone(), Some(state)));
        }
    }
    Ok(SeedReport {
        dataset_id: manifest.id.clone(),
        version: version.version_no,
        events_sha256: manifest.events_sha256.clone(),
        experiments,
    })
}
