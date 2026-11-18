//! The versioned demo snapshot (FINAL_AUDIT §4 item 6, SECURITY_REVIEW SEC-10): every row the
//! anonymous list and catalogue routes read (`/runs`, `/experiments`, `/comparisons`, `/reports`,
//! `/sweeps`, `/datasets`, `/presets`, `/demos`) and the demo comparisons `GET /comparisons/{id}`
//! answers anonymously, loaded from Postgres once and answered from memory for any query string.
//! Filters, sorts, cursors and limits are evaluated here with the semantics of the keyed path's
//! SQL, so varied anonymous queries cannot reach the database.
//!
//! Freshness (docs/rebuild/PHASE6_RELIABILITY.md §A3):
//!
//! * Changes made by this process ([`AppState::data_generation`]: any state change or quarantine
//!   here) are never served stale: [`maintain`] rebuilds about a second after a change, and an
//!   anonymous read that finds the snapshot behind rebuilds it first (single flight).
//! * Rows written by another process (`blackbox demo seed`, a separate worker, an operator's SQL)
//!   are found by [`bb_db::Database::demo_data_version`], one cheap statement over the demo
//!   workspace's rows, read before every build. An anonymous read at least [`RECHECK`] after the
//!   snapshot was built or last checked reads it again (one reader at a time; the others keep
//!   the current snapshot meanwhile). A changed version counts as a change this process made: it
//!   bumps the read version, so the snapshot and cached anonymous responses are rebuilt before
//!   anything else is served. Worst-case staleness for an external write
//!   is therefore [`RECHECK`] plus one build, and reads served from the snapshot query Postgres
//!   at most once per [`RECHECK`] at any request rate, so they leave Neon (5 minutes to scale to
//!   zero) asleep at least two thirds of the time.
//! * A failed check keeps the snapshot (retried after [`RECHECK`]); a snapshot that no build or
//!   check confirmed for [`MAX_AGE`] is rebuilt. A failed build never leaves a partial snapshot:
//!   anonymous reads use the database path for [`RETRY_AFTER`], then one reader retries.
//!
//! Rows are checked against their stored digests while building (FINAL_AUDIT item 5); a changed
//! row is left out (a run is quarantined), and since any row change also changes the data
//! version, a row changed after the build is dropped at the next check. Stored comparisons are
//! kept without their per-order pairs (only `/comparisons/{id}/pairs` serves those, from the
//! database), which keeps the snapshot to kilobytes per comparison. A demo workspace with more
//! than [`MAX_ROWS`] rows in a list is not snapshotted; anonymous reads then use the database path.
//! The database fingerprint includes quality checks independently of dataset versions, joined
//! workspace/key/job/config rows, and public experiment visibility used by the demo catalogue.
//! Presets and instruments are rendered from version manifests, not mutable instrument rows;
//! the audited dependency list is in docs/rebuild/PHASE6_RELIABILITY.md.
//!
//! [`AppState::data_generation`]: super::AppState::data_generation

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, PoisonError, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use bb_db::{
    Comparison, Cursor, ExperimentQuery, ExperimentView, JobView, ResearchReport, RunKey, RunSort,
    RunView,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{
    Shared,
    auth::Caller,
    comparisons, datasets,
    error::ApiError,
    model::{Dataset, Demo, Preset},
    reports,
    runs::listed_summary,
};

/// Largest number of rows of one list kept in memory.
pub const MAX_ROWS: usize = 1000;
/// Minimum time between two reads of the database's demo data version (rows written by another
/// process): the worst-case staleness of such a write, and the most often warm anonymous traffic
/// can make the server query Postgres.
pub const RECHECK: Duration = Duration::from_secs(15 * 60);
/// Longest a snapshot is served without a build or a check confirming it (checks failing).
pub const MAX_AGE: Duration = Duration::from_secs(3600);
/// After a failed build, anonymous reads use the database path this long before a retry.
pub const RETRY_AFTER: Duration = Duration::from_secs(60);
const PAGE: i64 = 100;

/// One immutable build (plus when it was last confirmed).
pub struct Snapshot {
    /// `AppState::data_generation` when the build started.
    generation: u64,
    /// The demo workspace and its `demo_data_version` read before any row.
    workspace: Uuid,
    version: String,
    /// When a build or a check last found this snapshot current, and when a check last ran.
    confirmed: Mutex<Instant>,
    checked: Mutex<Instant>,
    /// Live, digest-checked runs listed in the demo workspace (every platform), newest first.
    runs: Vec<RunView>,
    pub runs_by_id: HashMap<Uuid, RunView>,
    /// Demo experiments, newest first.
    experiments: Vec<ExperimentView>,
    pub jobs: HashMap<Uuid, JobView>,
    /// Intact comparisons and saved reports, newest first.
    comparisons: Vec<Comparison>,
    reports: Vec<ResearchReport>,
    sweeps: Vec<bb_db::Sweep>,
    sweep_cells: HashMap<Uuid, Vec<bb_db::SweepCellView>>,
    /// Rendered datasets in the database's id order.
    datasets: Vec<Dataset>,
    /// `GET /presets` and `GET /demos` by `dataset_id` (`None` = the default dataset).
    presets: HashMap<Option<String>, Vec<Preset>>,
    demos: HashMap<Option<String>, Vec<Demo>>,
}

#[derive(Default)]
pub struct DemoSnapshot {
    current: RwLock<Option<Arc<Snapshot>>>,
    /// Generation and time of the last failed build.
    failed: Mutex<Option<(u64, Instant)>>,
    build: tokio::sync::Mutex<()>,
    /// Single flight of data version checks.
    check: tokio::sync::Mutex<()>,
    /// [`RECHECK`] override in milliseconds (0 = the default), see [`DemoSnapshot::set_recheck`].
    recheck_ms: AtomicU64,
}

fn now(lock: &Mutex<Instant>) -> Instant {
    *lock.lock().unwrap_or_else(PoisonError::into_inner)
}

fn touch(lock: &Mutex<Instant>) {
    *lock.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
}

impl Snapshot {
    fn usable(&self, generation: u64) -> bool {
        self.generation == generation && now(&self.confirmed).elapsed() < MAX_AGE
    }
}

impl DemoSnapshot {
    fn fresh(&self, generation: u64) -> Option<Arc<Snapshot>> {
        self.current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .filter(|s| s.usable(generation))
            .cloned()
    }

    fn recently_failed(&self, generation: u64) -> bool {
        self.failed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some_and(|(g, at)| g == generation && at.elapsed() < RETRY_AFTER)
    }

    /// The interval between data version checks ([`RECHECK`] unless overridden).
    pub fn recheck(&self) -> Duration {
        match self.recheck_ms.load(Ordering::Relaxed) {
            0 => RECHECK,
            ms => Duration::from_millis(ms),
        }
    }

    /// Overrides [`RECHECK`] (tests and drills; at least 1 ms).
    pub fn set_recheck(&self, interval: Duration) {
        let ms = u64::try_from(interval.as_millis())
            .unwrap_or(u64::MAX)
            .max(1);
        self.recheck_ms.store(ms, Ordering::Relaxed);
    }

    /// The snapshot for an anonymous caller (keyed callers always read the database).
    pub async fn for_read(&self, state: &Shared, caller: &Caller) -> Option<Arc<Snapshot>> {
        if caller.is_anonymous() {
            self.get(state).await
        } else {
            None
        }
    }

    /// The current snapshot, checked against the database's data version when due and rebuilt
    /// first when stale; `None` when it cannot be built (callers use the database path).
    pub async fn get(&self, state: &Shared) -> Option<Arc<Snapshot>> {
        let generation = state.data_generation();
        if let Some(s) = self.fresh(generation)
            && self.unchanged(state, &s).await
        {
            return Some(s);
        }
        if self.recently_failed(generation) {
            return None;
        }
        let _single_flight = self.build.lock().await;
        let generation = state.data_generation();
        if let Some(s) = self.fresh(generation) {
            return Some(s);
        }
        match build(state).await {
            Ok(s) => {
                let s = Arc::new(s);
                *self.current.write().unwrap_or_else(PoisonError::into_inner) = Some(s.clone());
                *self.failed.lock().unwrap_or_else(PoisonError::into_inner) = None;
                Some(s)
            }
            Err(e) => {
                tracing::warn!(error = %e.message, "demo snapshot not built; anonymous reads use the database");
                *self.current.write().unwrap_or_else(PoisonError::into_inner) = None;
                *self.failed.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some((generation, Instant::now()));
                None
            }
        }
    }

    /// Whether `s` still matches the database: `true` until a check is due, while another
    /// reader checks, and after a failed check (retried after the interval); one version read
    /// otherwise. A changed version is a state change this process can now observe: it bumps the
    /// read version ([`AppState::data_generation`], which also keys the anonymous response
    /// cache), so `s` is not served again and the caller rebuilds (`false`).
    ///
    /// [`AppState::data_generation`]: super::AppState::data_generation
    async fn unchanged(&self, state: &Shared, s: &Snapshot) -> bool {
        let interval = self.recheck();
        if now(&s.checked).elapsed() < interval {
            return true;
        }
        let Ok(_single_flight) = self.check.try_lock() else {
            return true;
        };
        if now(&s.checked).elapsed() < interval {
            return s.generation == state.data_generation();
        }
        let version = state.db.demo_data_version(s.workspace).await;
        touch(&s.checked);
        match version {
            Ok(v) if v == s.version => {
                touch(&s.confirmed);
                true
            }
            Ok(_) => {
                tracing::info!("demo data changed in another process; rebuilding the snapshot");
                state.hub.bump();
                false
            }
            Err(e) => {
                tracing::warn!(error = %e, "demo data version check failed; keeping the snapshot");
                true
            }
        }
    }

    fn generation(&self) -> Option<u64> {
        self.current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|s| s.generation)
    }
}

/// Rebuilds the snapshot about a second after data changed in this process (once the generation
/// has been stable for a tick), so anonymous reads rarely wait for a build. Memory-only while
/// nothing changes.
pub async fn maintain(state: Shared) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut seen = state.data_generation();
    loop {
        tick.tick().await;
        let now = state.data_generation();
        let stable = now == seen;
        seen = now;
        if stable && state.snapshot.generation().is_some_and(|g| g != now) {
            state.snapshot.get(&state).await;
        }
    }
}

fn too_many(what: &str) -> ApiError {
    ApiError::internal(format!(
        "demo {what} exceed the snapshot size ({MAX_ROWS} rows)"
    ))
}

/// Every page of a newest-first keyset list.
async fn all_pages<T, F, Fut>(
    what: &str,
    key: impl Fn(&T) -> Cursor,
    page: F,
) -> Result<Vec<T>, ApiError>
where
    F: Fn(Option<Cursor>) -> Fut,
    Fut: Future<Output = bb_db::Result<Vec<T>>>,
{
    let mut rows: Vec<T> = Vec::new();
    loop {
        let next = page(rows.last().map(&key)).await?;
        let done = next.len() < PAGE as usize;
        rows.extend(next);
        if rows.len() > MAX_ROWS {
            return Err(too_many(what));
        }
        if done {
            return Ok(rows);
        }
    }
}

async fn build(state: &Shared) -> Result<Snapshot, ApiError> {
    let generation = state.data_generation();
    let demo = state.workspaces.demo(state).await?;
    let caller = Caller {
        key: None,
        workspace: demo.clone(),
        demo: demo.clone(),
    };
    let db = &state.db;
    // Read before any row: a write that lands during the build changes the version again, so
    // the next check rebuilds.
    let version = db.demo_data_version(demo.id).await?;

    let rows = all_pages(
        "runs",
        |v: &RunView| Cursor {
            created_at: v.run.created_at,
            id: v.run.id,
        },
        |c| async move { db.workspace_run_views(demo.id, c.as_ref(), PAGE).await },
    )
    .await?;
    let mut runs = Vec::with_capacity(rows.len());
    for view in rows {
        // A changed row is quarantined here and left out, as on the database path.
        if listed_summary(state, &view).await?.is_some() {
            runs.push(view);
        }
    }
    let runs_by_id = runs.iter().map(|v| (v.run.id, v.clone())).collect();

    let query = ExperimentQuery::default();
    let experiments = all_pages(
        "experiments",
        |v: &ExperimentView| Cursor {
            created_at: v.experiment.created_at,
            id: v.experiment.id,
        },
        |c| {
            let query = &query;
            async move { db.experiment_views(demo.id, query, c.as_ref(), PAGE).await }
        },
    )
    .await?;
    let job_ids: Vec<Uuid> = experiments
        .iter()
        .filter_map(|v| v.experiment.job_id)
        .collect();
    let mut jobs = HashMap::new();
    for ids in job_ids.chunks(PAGE as usize) {
        for j in db.job_views_by_ids(ids).await? {
            jobs.insert(j.job.id, j);
        }
    }

    let by_row = |created_at: DateTime<Utc>, id: Uuid| Cursor { created_at, id };
    let mut comparisons = all_pages(
        "comparisons",
        |r: &Comparison| by_row(r.created_at, r.id),
        |c| async move {
            let rows = db.comparisons(demo.id, c.as_ref(), PAGE).await?;
            Ok::<_, bb_db::Error>(rows.into_iter().map(without_pairs).collect())
        },
    )
    .await?;
    let mut saved = all_pages(
        "reports",
        |r: &ResearchReport| by_row(r.created_at, r.id),
        |c| async move { db.reports(demo.id, c.as_ref(), PAGE).await },
    )
    .await?;
    // Changed rows are left out here as on the database path.
    comparisons.retain(comparisons::intact);
    saved.retain(reports::intact);
    let sweeps = all_pages(
        "sweeps",
        |r: &bb_db::Sweep| by_row(r.created_at, r.id),
        |c| async move { db.sweeps(demo.id, c.as_ref(), PAGE).await },
    )
    .await?;
    let mut sweep_cells = HashMap::new();
    for sweep in &sweeps {
        sweep_cells.insert(sweep.id, db.sweep_cell_views(sweep.id).await?);
    }

    let mut rows: Vec<bb_db::Dataset> = Vec::new();
    loop {
        let after = rows.last().map(|d| d.id.clone());
        let page = db.datasets(demo.id, after.as_deref(), PAGE).await?;
        let done = page.len() < PAGE as usize;
        rows.extend(page);
        if rows.len() > MAX_ROWS {
            return Err(too_many("datasets"));
        }
        if done {
            break;
        }
    }
    let mut rendered = Vec::with_capacity(rows.len());
    for d in &rows {
        rendered.push(datasets::dataset_resource(state, d, false).await?);
    }
    let mut presets = HashMap::new();
    let mut demos = HashMap::new();
    for key in std::iter::once(None).chain(rows.iter().map(|d| Some(d.id.clone()))) {
        // A catalogue that cannot be built (e.g. a dataset without a current version) is left
        // to the database path, which answers its error.
        if let Ok(p) = datasets::presets_for(state, &caller, key.as_deref()).await {
            presets.insert(key.clone(), p);
        }
        if let Ok(d) = datasets::demos_for(state, &caller, key.as_deref()).await {
            demos.insert(key, d);
        }
    }

    let at = Instant::now();
    Ok(Snapshot {
        generation,
        workspace: demo.id,
        version,
        confirmed: Mutex::new(at),
        checked: Mutex::new(at),
        runs,
        runs_by_id,
        experiments,
        jobs,
        comparisons,
        reports: saved,
        sweeps,
        sweep_cells,
        datasets: rendered,
        presets,
        demos,
    })
}

/// A stored comparison without its per-order pairs: every snapshot read renders the resource
/// without them (`/comparisons/{id}/pairs` reads the database), and they are most of its bytes.
/// `summary_intact` was computed by the database from the whole row first.
fn without_pairs(mut row: Comparison) -> Comparison {
    if let Some(pairs) = row.summary.pointer_mut("/alignment/pairs") {
        *pairs = serde_json::Value::Array(Vec::new());
    }
    row
}

fn matches_identity(short_id: &str, fingerprint: &[u8], identity: Option<&str>) -> bool {
    identity.is_none_or(|id| short_id == id || hex::encode(fingerprint) == id)
}

fn before_cursor(created_at: DateTime<Utc>, id: Uuid, c: Option<&Cursor>) -> bool {
    c.is_none_or(|c| (created_at, id) < (c.created_at, c.id))
}

impl Snapshot {
    /// `Database::run_views` of the demo workspace (same filters, order and keyset).
    pub fn run_views(
        &self,
        platform: &str,
        q: &ExperimentQuery,
        sort: RunSort,
        after: Option<&RunKey>,
        count: usize,
    ) -> Vec<RunView> {
        // `EXISTS` an experiment of the workspace for the run matching the experiment filters.
        let listed = |v: &RunView| {
            self.experiments.iter().any(|e| {
                let x = &e.experiment;
                x.experiment_config_id == v.run.experiment_config_id
                    && x.platform == v.run.platform
                    && q.sweep_id.is_none_or(|s| x.sweep_id == Some(s))
                    && q.label_prefix
                        .as_deref()
                        .is_none_or(|p| x.label.starts_with(p))
                    && q.created_by_key
                        .is_none_or(|k| x.created_by_key_id == Some(k))
            })
        };
        let value = |v: &RunView| v.key(sort).value.unwrap_or(0.0);
        let mut rows: Vec<&RunView> = self
            .runs
            .iter()
            .filter(|v| {
                v.run.platform == platform
                    && q.dataset_id.as_deref().is_none_or(|d| v.dataset_id == d)
                    && q.strategy_kind
                        .as_deref()
                        .is_none_or(|s| v.strategy_kind.as_deref() == Some(s))
                    && q.created_after.is_none_or(|t| v.run.created_at >= t)
                    && q.created_before.is_none_or(|t| v.run.created_at < t)
                    && matches_identity(&v.short_id, &v.fingerprint, q.run_identity.as_deref())
                    && listed(v)
            })
            .filter(|v| match (sort, after) {
                (_, None) => true,
                (RunSort::CreatedDesc, Some(k)) => {
                    (v.run.created_at, v.run.id) < (k.created_at, k.id)
                }
                (RunSort::NetPnlDesc, Some(k)) => k
                    .value
                    .is_none_or(|kv| value(v) < kv || (value(v) == kv && v.run.id < k.id)),
                (RunSort::NetPnlAsc | RunSort::MaxDrawdownAsc, Some(k)) => k
                    .value
                    .is_none_or(|kv| value(v) > kv || (value(v) == kv && v.run.id > k.id)),
            })
            .collect();
        let by_value = |a: &&RunView, b: &&RunView| {
            value(a).total_cmp(&value(b)).then(a.run.id.cmp(&b.run.id))
        };
        match sort {
            RunSort::CreatedDesc => {
                rows.sort_by_key(|v| std::cmp::Reverse((v.run.created_at, v.run.id)))
            }
            RunSort::NetPnlDesc => rows.sort_by(|a, b| by_value(b, a)),
            RunSort::NetPnlAsc | RunSort::MaxDrawdownAsc => rows.sort_by(by_value),
        }
        rows.into_iter().take(count).cloned().collect()
    }

    /// `Database::experiment_views` of the demo workspace.
    pub fn experiment_views(
        &self,
        q: &ExperimentQuery,
        cursor: Option<&Cursor>,
        count: usize,
    ) -> Vec<ExperimentView> {
        self.experiments
            .iter()
            .filter(|v| {
                let e = &v.experiment;
                q.dataset_id.as_deref().is_none_or(|d| e.dataset_id == d)
                    && q.strategy_kind
                        .as_deref()
                        .is_none_or(|s| e.strategy_kind == s)
                    && q.sweep_id.is_none_or(|s| e.sweep_id == Some(s))
                    && q.label_prefix
                        .as_deref()
                        .is_none_or(|p| e.label.starts_with(p))
                    && q.created_after.is_none_or(|t| e.created_at >= t)
                    && q.created_before.is_none_or(|t| e.created_at < t)
                    && q.created_by_key
                        .is_none_or(|k| e.created_by_key_id == Some(k))
                    && matches_identity(&v.short_id, &v.fingerprint, q.run_identity.as_deref())
                    && before_cursor(e.created_at, e.id, cursor)
                    && (q.states.is_empty() || q.states.contains(&v.status))
            })
            .take(count)
            .cloned()
            .collect()
    }

    /// `Database::comparisons` of the demo workspace (intact rows only).
    pub fn comparisons(&self, cursor: Option<&Cursor>, count: usize) -> Vec<Comparison> {
        self.comparisons
            .iter()
            .filter(|r| before_cursor(r.created_at, r.id, cursor))
            .take(count)
            .cloned()
            .collect()
    }

    /// One intact demo comparison (without per-order pairs).
    pub fn comparison(&self, id: Uuid) -> Option<&Comparison> {
        self.comparisons.iter().find(|r| r.id == id)
    }

    /// `Database::reports` of the demo workspace (intact rows only).
    pub fn reports(&self, cursor: Option<&Cursor>, count: usize) -> Vec<ResearchReport> {
        self.reports
            .iter()
            .filter(|r| before_cursor(r.created_at, r.id, cursor))
            .take(count)
            .cloned()
            .collect()
    }

    /// Demo sweeps in the database's newest-first keyset order.
    pub fn sweeps(&self, cursor: Option<&Cursor>, count: usize) -> Vec<bb_db::Sweep> {
        self.sweeps
            .iter()
            .filter(|r| before_cursor(r.created_at, r.id, cursor))
            .take(count)
            .cloned()
            .collect()
    }

    pub fn sweep_cells(&self, id: Uuid) -> &[bb_db::SweepCellView] {
        self.sweep_cells.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Rendered datasets after the id `after` (in the database's order), at most `count`.
    pub fn datasets(&self, after: Option<&str>, count: usize) -> Vec<&Dataset> {
        let start = match after {
            None => 0,
            Some(a) => match self.datasets.iter().position(|d| d.id == a) {
                Some(i) => i + 1,
                None => self
                    .datasets
                    .iter()
                    .take_while(|d| d.id.as_str() < a)
                    .count(),
            },
        };
        self.datasets.iter().skip(start).take(count).collect()
    }

    pub fn has_dataset(&self, id: &str) -> bool {
        self.datasets.iter().any(|d| d.id == id)
    }

    pub fn presets(&self, dataset_id: Option<&str>) -> Option<&Vec<Preset>> {
        self.presets.get(&dataset_id.map(str::to_string))
    }

    pub fn demos(&self, dataset_id: Option<&str>) -> Option<&Vec<Demo>> {
        self.demos.get(&dataset_id.map(str::to_string))
    }
}
