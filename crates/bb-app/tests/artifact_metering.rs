//! SECURITY_REVIEW §6.3 / SEC-11 against a real Postgres (`BLACKBOX_TEST_DATABASE_URL`, else
//! skip), with a counting fake object store: a metered store reserves its budget before every
//! GET, HEAD and PUT attempt (retries included), a spent budget sends nothing, reads are
//! classified by caller (anonymous requests get at most half of the daily share, the worker's
//! read-back is server-side), and bundle integrity verification is unchanged.

mod common;

use std::{
    ops::Range,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use bb_app::artifacts::{ReadBudget, Storage, StorageError};
use bb_artifacts::{ArtifactError, ArtifactHead, ArtifactStore, FsArtifactStore};
use bb_db::{Database, Scope};
use common::*;

macro_rules! db_or_skip {
    () => {
        match TestDb::create().await {
            Some(db) => db,
            None => return,
        }
    };
}

/// A filesystem store counting every request it receives; the next `fail_gets` / `fail_puts`
/// requests answer 503 (a transient Blob failure that the wrapper retries).
struct CountingStore {
    inner: FsArtifactStore,
    gets: AtomicUsize,
    heads: AtomicUsize,
    puts: AtomicUsize,
    fail_gets: AtomicUsize,
    fail_puts: AtomicUsize,
}

impl CountingStore {
    fn new(root: &Path) -> Arc<Self> {
        Arc::new(Self {
            inner: FsArtifactStore::new(root).unwrap(),
            gets: AtomicUsize::new(0),
            heads: AtomicUsize::new(0),
            puts: AtomicUsize::new(0),
            fail_gets: AtomicUsize::new(0),
            fail_puts: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> (usize, usize, usize) {
        (
            self.gets.load(Ordering::SeqCst),
            self.heads.load(Ordering::SeqCst),
            self.puts.load(Ordering::SeqCst),
        )
    }
}

/// Decrements `counter` if positive; true when this request should fail.
fn take(counter: &AtomicUsize) -> bool {
    let mut n = counter.load(Ordering::SeqCst);
    while n > 0 {
        match counter.compare_exchange(n, n - 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(current) => n = current,
        }
    }
    false
}

#[async_trait::async_trait]
impl ArtifactStore for CountingStore {
    async fn put_bundle_from_file(
        &self,
        key: &str,
        path: &Path,
    ) -> Result<ArtifactHead, ArtifactError> {
        self.puts.fetch_add(1, Ordering::SeqCst);
        if take(&self.fail_puts) {
            return Err(ArtifactError::Http(503));
        }
        self.inner.put_bundle_from_file(key, path).await
    }
    async fn get_range(&self, key: &str, range: Range<u64>) -> Result<bytes::Bytes, ArtifactError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        if take(&self.fail_gets) {
            return Err(ArtifactError::Http(503));
        }
        self.inner.get_range(key, range).await
    }
    async fn head(&self, key: &str) -> Result<ArtifactHead, ArtifactError> {
        self.heads.fetch_add(1, Ordering::SeqCst);
        if take(&self.fail_gets) {
            return Err(ArtifactError::Http(503));
        }
        self.inner.head(key).await
    }
    async fn delete(&self, key: &str) -> Result<(), ArtifactError> {
        self.inner.delete(key).await
    }
}

const KEY: &str = "runs/metering/object.bbz";

/// Monthly reads charged so far (all backends of the fake are `vercel_blob`).
async fn month_reads(db: &Database) -> i32 {
    sqlx::query_scalar("SELECT coalesce(sum(reads),0)::int FROM bb.storage_usage_monthly WHERE storage_backend='vercel_blob'")
        .fetch_one(db.pool())
        .await
        .unwrap()
}

/// Today's (UTC) reads and anonymous reads.
async fn today_reads(db: &Database) -> (i32, i32) {
    sqlx::query_as("SELECT coalesce(max(reads),0),coalesce(max(anonymous_reads),0) FROM bb.storage_reads_daily WHERE storage_backend='vercel_blob' AND day=(now() AT TIME ZONE 'UTC')::date")
        .fetch_one(db.pool())
        .await
        .unwrap()
}

fn budget_refused<T: std::fmt::Debug>(result: Result<T, StorageError>) {
    assert!(
        matches!(result, Err(StorageError::Budget)),
        "expected a budget refusal, got {result:?}"
    );
}

/// A spent read budget sends no GET or HEAD; a spent upload allowance sends no PUT; a retry
/// after a transient failure reserves again and stops at a spent budget; the filesystem store
/// stays unmetered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spent_budgets_send_no_store_request_for_get_head_put_or_retry() {
    let tdb = db_or_skip!();
    let db = tdb.pool().await;
    let dir = tempfile::tempdir().unwrap();
    let store = CountingStore::new(&dir.path().join("objects"));
    let file = dir.path().join("object.bin");
    std::fs::write(&file, vec![7u8; 4096]).unwrap();
    store.inner.put_bundle_from_file(KEY, &file).await.unwrap();

    // Caps are multiples of the days left this month, so the daily pacing share is exact.
    let d = days_left(&db).await;
    let metered = |reads: i32, bytes: i64| {
        Storage::metered(
            store.clone(),
            "vercel_blob",
            ReadBudget::new(db.clone(), reads, bytes),
        )
    };

    // Zero monthly reads: nothing reaches the store, nothing is charged.
    let spent = metered(0, i64::MAX);
    budget_refused(spent.get_range(KEY, 0..16).await);
    budget_refused(spent.head(KEY).await);
    assert_eq!(store.calls(), (0, 0, 0));
    assert_eq!(month_reads(&db).await, 0);

    // A byte cap below the range: refused before the GET; HEAD (0 bytes) still fits.
    let bytes = metered(1_000 * d, 8);
    budget_refused(bytes.get_range(KEY, 0..16).await);
    assert_eq!(store.calls(), (0, 0, 0));
    assert_eq!(bytes.head(KEY).await.unwrap().size_bytes, 4096);
    assert_eq!(store.calls(), (0, 1, 0));
    assert_eq!(month_reads(&db).await, 1, "the HEAD is a charged read op");

    // Retry: a daily share of 2 leaves one read op; the first GET fails transiently, its retry
    // is refused before a second request is sent (fail closed, not unmetered).
    let one_left = metered(2 * d, i64::MAX);
    store.fail_gets.store(1, Ordering::SeqCst);
    budget_refused(one_left.get_range(KEY, 0..16).await);
    assert_eq!(store.calls(), (1, 1, 0));
    assert_eq!(month_reads(&db).await, 2);
    // HEAD retries are reserved the same way.
    let three = metered(3 * d, i64::MAX);
    store.fail_gets.store(1, Ordering::SeqCst);
    budget_refused(three.head(KEY).await);
    assert_eq!(store.calls(), (1, 2, 0));
    assert_eq!(month_reads(&db).await, 3);
    // With budget left, a transient failure is retried and every attempt is charged.
    let roomy = metered(100 * d, i64::MAX);
    store.fail_gets.store(1, Ordering::SeqCst);
    assert_eq!(roomy.get_range(KEY, 0..16).await.unwrap().len(), 16);
    assert_eq!(store.calls(), (3, 2, 0));
    assert_eq!(month_reads(&db).await, 5);

    // Uploads: a workspace share of two attempts; both fail transiently, the third attempt is
    // refused before any PUT.
    let ws = db.create_workspace("metering", "metering").await.unwrap();
    sqlx::query("UPDATE bb.workspaces SET quota_uploads_per_month=2 WHERE id=$1")
        .bind(ws.id)
        .execute(db.pool())
        .await
        .unwrap();
    store.fail_puts.store(5, Ordering::SeqCst);
    budget_refused(
        roomy
            .put("runs/metering/upload.bbz", &file, Some(ws.id))
            .await,
    );
    assert_eq!(store.calls(), (3, 2, 2));
    // The share is spent: no PUT at all.
    store.fail_puts.store(0, Ordering::SeqCst);
    budget_refused(
        roomy
            .put("runs/metering/upload.bbz", &file, Some(ws.id))
            .await,
    );
    assert_eq!(store.calls(), (3, 2, 2));
    // The global monthly allowance spent: no PUT for a job without a workspace either.
    sqlx::query(
        "UPDATE bb.storage_usage_monthly SET uploads=$1 WHERE storage_backend='vercel_blob'",
    )
    .bind(bb_db::BLOB_UPLOADS_PER_MONTH)
    .execute(db.pool())
    .await
    .unwrap();
    budget_refused(roomy.put("runs/metering/global.bbz", &file, None).await);
    assert_eq!(store.calls(), (3, 2, 2));
    assert_eq!(month_reads(&db).await, 5, "uploads never charge reads");

    // The filesystem store is unmetered: it reads and writes without any budget row.
    let fs = Storage::filesystem(store.clone());
    assert_eq!(fs.backend, "fs");
    assert_eq!(fs.get_range(KEY, 0..4).await.unwrap().len(), 4);
    assert_eq!(fs.head(KEY).await.unwrap().size_bytes, 4096);
    fs.put("runs/metering/fs.bbz", &file, Some(ws.id))
        .await
        .unwrap();
    assert_eq!(store.calls(), (4, 3, 3));
    let fs_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM bb.storage_reads_daily WHERE storage_backend='fs'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(fs_rows, 0);
    db.pool().close().await;
    tdb.drop_db().await;
}

/// The daily pacing share and its anonymous half, on fixed days: reads before today shrink the
/// share, anonymous reads stop at half of it, keyed and server-side reads use the rest, and a
/// new day restores both.
#[tokio::test]
async fn anonymous_reads_reserve_at_most_half_of_the_daily_share() {
    use bb_db::ReadClass::{Anonymous, Keyed, System};
    let tdb = db_or_skip!();
    let db = tdb.pool().await;
    let nov1 = chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    // 300 a month over 30 days: 10 a day, 5 of them anonymous.
    let charge = |class, day| {
        let db = db.clone();
        async move {
            db.charge_storage_read_on("vercel_blob", 0, 300, i64::MAX, day, class)
                .await
                .unwrap()
        }
    };
    for _ in 0..5 {
        assert!(charge(Anonymous, nov1).await);
    }
    assert!(!charge(Anonymous, nov1).await, "anonymous half share spent");
    for _ in 0..5 {
        assert!(charge(Keyed, nov1).await);
    }
    assert!(!charge(Keyed, nov1).await, "daily share spent");
    assert!(!charge(System, nov1).await, "daily share spent");
    let (reads, anonymous): (i32, i32) = sqlx::query_as(
        "SELECT reads,anonymous_reads FROM bb.storage_reads_daily WHERE day=$1 AND storage_backend='vercel_blob'",
    )
    .bind(nov1)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!((reads, anonymous), (10, 5));

    // November 2: (300 - 10) / 29 = 10 again; anonymous may not take more than 5 of them even
    // when keyed traffic is idle, and server-side reads are not limited by the anonymous half.
    let nov2 = chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
    for _ in 0..5 {
        assert!(charge(Anonymous, nov2).await);
    }
    assert!(!charge(Anonymous, nov2).await);
    for _ in 0..5 {
        assert!(charge(System, nov2).await);
    }
    assert!(!charge(System, nov2).await);
    // On the last day the share is what the month has left: with 20 reads spent and a cap of 21
    // it is one read, which leaves anonymous callers nothing (half of 1 rounds down).
    let nov30 = chrono::NaiveDate::from_ymd_opt(2026, 11, 30).unwrap();
    let small = |class| {
        let db = db.clone();
        async move {
            db.charge_storage_read_on("vercel_blob", 0, 21, i64::MAX, nov30, class)
                .await
                .unwrap()
        }
    };
    assert!(!small(Anonymous).await);
    assert!(small(Keyed).await);
    db.pool().close().await;
    tdb.drop_db().await;
}

/// The day's remaining days in UTC, as the budget computes them.
async fn days_left(db: &Database) -> i32 {
    sqlx::query_scalar("SELECT ((date_trunc('month', now() AT TIME ZONE 'UTC') + interval '1 month')::date - (now() AT TIME ZONE 'UTC')::date)::int")
        .fetch_one(db.pool())
        .await
        .unwrap()
}

/// End to end over HTTP with a metered counting store: the worker's read-back is a server-side
/// read, an anonymous cold run read is an anonymous read, a keyed one is keyed; once the
/// anonymous half share is spent an anonymous miss is refused (503 `storage`) without any store
/// request while a keyed read still succeeds; a tampered stored bundle still fails integrity
/// verification through the metered store.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn caller_classes_are_metered_end_to_end_and_integrity_is_unchanged() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let db = tdb.pool().await;
    // A daily share of 20 reads (10 anonymous) for the rest of this month.
    let reads_cap = 20 * days_left(&db).await;
    let objects = dir.path().join("objects");
    let store = CountingStore::new(&objects);
    let storage = Storage::metered(
        store.clone(),
        "vercel_blob",
        ReadBudget::new(db.clone(), reads_cap, i64::MAX),
    );
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            storage: Some(storage),
            ..Options::default()
        },
    )
    .await;
    let manifest = register_dataset(&server.db, dir.path(), 20).await;
    let (_, admin) = workspace_key(&server.db, "demo", &[Scope::Admin]).await;
    let (a, _) = run(&server, &admin, config_json(&manifest, 100, 1), "a").await;
    let (b, _) = run(&server, &admin, config_json(&manifest, 200, 1), "b").await;
    let (c, _) = run(&server, &admin, config_json(&manifest, 300, 1), "c").await;
    // Three uploads, each read back once by the worker: server-side reads, none anonymous.
    let (gets, _, puts) = store.calls();
    assert_eq!(puts, 3);
    assert_eq!(today_reads(&db).await, (gets as i32, 0));
    server.stop().await;

    // A fresh instance (empty bundle cache): every run read below is a store miss.
    let cold = tempfile::tempdir().unwrap();
    let storage = Storage::metered(
        store.clone(),
        "vercel_blob",
        ReadBudget::new(db.clone(), reads_cap, i64::MAX),
    );
    let server = Server::start(
        &tdb,
        cold.path(),
        Options {
            storage: Some(storage),
            ..Options::default()
        },
    )
    .await;

    // Anonymous cold read of run a: every GET it causes is an anonymous read.
    let before = (store.calls().0, today_reads(&db).await);
    let (status, _, body) = server.get(&format!("/api/v1/runs/{a}/equity"), None).await;
    assert_eq!(status, 200, "{body}");
    let gets = store.calls().0 - before.0;
    assert!(gets >= 1);
    let (reads, anonymous) = today_reads(&db).await;
    assert_eq!(reads - before.1.0, gets as i32);
    assert_eq!(anonymous - before.1.1, gets as i32);

    // Keyed cold read of run b: reads, no anonymous reads.
    let before = (store.calls().0, today_reads(&db).await);
    let (status, _, body) = server
        .get(&format!("/api/v1/runs/{b}/equity"), Some(&admin))
        .await;
    assert_eq!(status, 200, "{body}");
    let gets = store.calls().0 - before.0;
    assert!(gets >= 1);
    let (reads, anonymous) = today_reads(&db).await;
    assert_eq!(reads - before.1.0, gets as i32);
    assert_eq!(anonymous, before.1.1);

    // Spend the anonymous half share (10) as if earlier anonymous reads had taken it.
    let (reads, anonymous) = today_reads(&db).await;
    let fill = 10 - anonymous;
    assert!(fill >= 0 && reads + fill < 20, "{reads} {anonymous}");
    sqlx::query("UPDATE bb.storage_reads_daily SET reads=reads+$1,anonymous_reads=anonymous_reads+$1 WHERE storage_backend='vercel_blob' AND day=(now() AT TIME ZONE 'UTC')::date")
        .bind(fill)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE bb.storage_usage_monthly SET reads=reads+$1 WHERE storage_backend='vercel_blob' AND month=date_trunc('month',now() AT TIME ZONE 'UTC')::date")
        .bind(fill)
        .execute(db.pool())
        .await
        .unwrap();
    let before = store.calls();
    let (status, headers, body) = server.get(&format!("/api/v1/runs/{c}/equity"), None).await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["error"]["details"]["reason"], "storage", "{body}");
    assert!(headers.get("retry-after").is_some());
    assert_eq!(
        store.calls(),
        before,
        "a refused anonymous miss sends nothing"
    );
    // A keyed caller still has the other half.
    let (status, _, body) = server
        .get(&format!("/api/v1/runs/{c}/equity"), Some(&admin))
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(store.calls().0 > before.0);
    server.stop().await;

    // Integrity on read is unchanged: flip one byte of run a's stored object and read it cold
    // (fresh data directory, empty bundle cache) through the metered store.
    let storage_key: String = sqlx::query_scalar("SELECT a.storage_key FROM bb.run_artifacts a JOIN bb.runs r ON r.id=a.run_id JOIN bb.experiment_configs c ON c.id=r.experiment_config_id WHERE c.short_id=$1 AND a.kind='bundle'")
        .bind(&a)
        .fetch_one(db.pool())
        .await
        .unwrap();
    let mut tampered = false;
    for entry in walk(&objects) {
        if entry
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with(&storage_key)
        {
            let mut bytes = std::fs::read(&entry).unwrap();
            let middle = bytes.len() / 2;
            bytes[middle] ^= 0x01;
            std::fs::write(&entry, bytes).unwrap();
            tampered = true;
        }
    }
    assert!(tampered, "stored object not found");
    let fresh = tempfile::tempdir().unwrap();
    let storage = Storage::metered(
        store.clone(),
        "vercel_blob",
        ReadBudget::new(db.clone(), reads_cap * 2, i64::MAX),
    );
    let server = Server::start(
        &tdb,
        fresh.path(),
        Options {
            storage: Some(storage),
            ..Options::default()
        },
    )
    .await;
    let (status, _, body) = server
        .get(&format!("/api/v1/runs/{a}/equity"), Some(&admin))
        .await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(body["error"]["code"], "integrity_failed", "{body}");
    server.stop().await;
    db.pool().close().await;
    tdb.drop_db().await;
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}
