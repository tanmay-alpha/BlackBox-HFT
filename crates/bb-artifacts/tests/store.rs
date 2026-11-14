mod common;
use bb_artifacts::*;
use std::{collections::BTreeMap, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

#[tokio::test]
async fn filesystem_atomic_publication_ranges_and_traversal() {
    let directory = tempfile::tempdir().unwrap();
    let store = FsArtifactStore::new(directory.path().join("store")).unwrap();
    let source = directory.path().join("source.bbz");
    std::fs::write(&source, b"0123456789").unwrap();
    let key = "runs/abc/test/run.bbz";
    let (a, b) = tokio::join!(
        store.put_bundle_from_file(key, &source),
        store.put_bundle_from_file(key, &source)
    );
    assert!(matches!(
        (&a, &b),
        (Ok(_), Err(ArtifactError::AlreadyExists)) | (Err(ArtifactError::AlreadyExists), Ok(_))
    ));
    assert_eq!(store.head(key).await.unwrap().size_bytes, 10);
    assert_eq!(&store.get_range(key, 2..6).await.unwrap()[..], b"2345");
    assert!(matches!(
        store.get_range(key, 5..20).await,
        Err(ArtifactError::Integrity(_))
    ));
    for key in [
        "../outside",
        "/absolute",
        "a/../../outside",
        "a\\outside",
        "C:/outside",
        "a//b",
        "a/./b",
        "CON.bbz",
        "a/nul",
        "a/b.",
        "a/%2e%2e/outside",
    ] {
        assert!(
            matches!(
                store.put_bundle_from_file(key, &source).await,
                Err(ArtifactError::InvalidInput(_))
            ),
            "{key}"
        );
    }
    store.delete(key).await.unwrap();
    store.delete(key).await.unwrap();
    assert!(matches!(
        store.head(key).await,
        Err(ArtifactError::NotFound)
    ));
    assert!(matches!(
        store.get_range(key, 0..1).await,
        Err(ArtifactError::NotFound)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn filesystem_rejects_symlink_escape() {
    let directory = tempfile::tempdir().unwrap();
    let store = FsArtifactStore::new(directory.path().join("store")).unwrap();
    let outside = directory.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, directory.path().join("store/link")).unwrap();
    let source = directory.path().join("source");
    std::fs::write(&source, b"safe").unwrap();
    assert!(matches!(
        store.put_bundle_from_file("link/escaped", &source).await,
        Err(ArtifactError::InvalidInput(_))
    ));
    assert!(!outside.join("escaped").exists());
}

#[tokio::test]
async fn reader_range_store_authenticates_every_access() {
    let directory = tempfile::tempdir().unwrap();
    let bundle = BundleWriter::new(BundleLimits::default())
        .unwrap()
        .write_run(directory.path(), &common::run(8))
        .unwrap();
    let root = directory.path().join("objects");
    let store = FsArtifactStore::new(&root).unwrap();
    store
        .put_bundle_from_file("run.bbz", bundle.path())
        .await
        .unwrap();
    let reader = BundleReader::new(&bundle.trust, BundleLimits::default()).unwrap();
    let expected = reader.page(bundle.path(), 0).unwrap();
    assert_eq!(
        reader.page_from_store(&store, "run.bbz", 0).await.unwrap(),
        expected
    );
    // A previously successful page read must not grant trust to a later mutated trailer.
    let mut bytes = std::fs::read(root.join("run.bbz")).unwrap();
    bytes[bundle.trust.trailer.offset as usize + 1] ^= 1;
    std::fs::write(root.join("run.bbz"), bytes).unwrap();
    assert!(matches!(
        reader.page_from_store(&store, "run.bbz", 0).await,
        Err(ArtifactError::Integrity(_))
    ));
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
impl Reply {
    fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: vec![],
            body: body.into(),
        }
    }
    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}
struct Request {
    method: String,
    target: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}
async fn mock(build: impl FnOnce(&str) -> Vec<Reply>) -> (String, JoinHandle<Vec<Request>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let replies = build(&base);
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for reply in replies {
            let (mut connection, _) =
                tokio::time::timeout(Duration::from_secs(10), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
            let mut buffer = Vec::new();
            let header_end = loop {
                let mut chunk = [0; 4096];
                let n = connection.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                buffer.extend_from_slice(&chunk[..n]);
                if let Some(end) = buffer.windows(4).position(|b| b == b"\r\n\r\n") {
                    break end + 4;
                }
                assert!(buffer.len() < 64 << 10);
            };
            let text = std::str::from_utf8(&buffer[..header_end]).unwrap();
            let mut lines = text.split("\r\n");
            let first: Vec<_> = lines.next().unwrap().split_whitespace().collect();
            let (method, target) = (first[0].to_owned(), first[1].to_owned());
            let headers: BTreeMap<_, _> = lines
                .filter_map(|line| line.split_once(':'))
                .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
                .collect();
            let length: usize = headers
                .get("content-length")
                .map(|s| s.parse().unwrap())
                .unwrap_or(0);
            while buffer.len() < header_end + length {
                let mut chunk = [0; 8192];
                let n = connection.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                buffer.extend_from_slice(&chunk[..n]);
            }
            requests.push(Request {
                method,
                target,
                headers,
                body: buffer[header_end..header_end + length].to_vec(),
            });
            let mut response = format!("HTTP/1.1 {} Mock\r\nConnection: close\r\n", reply.status);
            if !reply
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            {
                response.push_str(&format!("Content-Length: {}\r\n", reply.body.len()));
            }
            for (name, value) in reply.headers {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("\r\n");
            connection.write_all(response.as_bytes()).await.unwrap();
            connection.write_all(&reply.body).await.unwrap();
            connection.shutdown().await.unwrap();
        }
        requests
    });
    (base, task)
}
fn mock_store(base: &str) -> VercelBlobStore {
    let mut config = BlobConfig::for_store("test").unwrap();
    config.api_url = format!("{base}/api/blob").parse().unwrap();
    config.read_base_url = format!("{base}/objects/").parse().unwrap();
    config.retry.base_delay = Duration::ZERO;
    VercelBlobStore::with_config("local-mock-secret", config).unwrap()
}

#[tokio::test]
async fn blob_private_api12_stream_upload_range_head_delete_and_retries() {
    let (base, requests) = mock(|base| vec![
        Reply::new(429, Vec::new()).header("Retry-After", "0"), Reply::new(503, Vec::new()),
        Reply::new(200, serde_json::to_vec(&serde_json::json!({"pathname":"runs/a.bbz","url":format!("{base}/objects/runs/a.bbz"),"etag":"etag"})).unwrap()),
        Reply::new(206, b"2345".to_vec()).header("Content-Range", "bytes 2-5/10"),
        Reply::new(200, Vec::new()).header("Content-Length", "10").header("ETag", "etag"), Reply::new(200, b"{}".to_vec()),
    ]).await;
    let store = mock_store(&base);
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("run.bbz");
    let payload = vec![0x5a; 128 << 10];
    std::fs::write(&source, &payload).unwrap();
    let uploaded = store
        .put_bundle_from_file("runs/a.bbz", &source)
        .await
        .unwrap();
    assert_eq!(uploaded.size_bytes, payload.len() as u64);
    assert_eq!(uploaded.etag.as_deref(), Some("etag"));
    assert_eq!(
        &store.get_range("runs/a.bbz", 2..6).await.unwrap()[..],
        b"2345"
    );
    assert_eq!(store.head("runs/a.bbz").await.unwrap().size_bytes, 10);
    store.delete("runs/a.bbz").await.unwrap();
    let requests = requests.await.unwrap();
    assert_eq!(requests.len(), 6);
    for request in &requests {
        assert_eq!(
            request.headers.get("authorization").unwrap(),
            "Bearer local-mock-secret"
        );
        assert_eq!(request.headers.get("x-api-version").unwrap(), "12");
    }
    for upload in &requests[..3] {
        assert_eq!(upload.method, "PUT");
        assert!(upload.target.starts_with("/api/blob?pathname=runs%2Fa.bbz"));
        assert_eq!(upload.body, payload);
        for (name, expected) in [
            ("x-vercel-blob-access", "private"),
            ("x-add-random-suffix", "0"),
            ("x-allow-overwrite", "0"),
            ("x-content-type", "application/gzip"),
        ] {
            assert_eq!(upload.headers.get(name).unwrap(), expected);
        }
    }
    assert_eq!(requests[3].method, "GET");
    assert_eq!(requests[3].headers.get("range").unwrap(), "bytes=2-5");
    assert_eq!(requests[4].method, "HEAD");
    assert_eq!(requests[5].method, "POST");
    assert_eq!(requests[5].target, "/api/blob/delete");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&requests[5].body).unwrap(),
        serde_json::json!({"urls":[format!("{base}/objects/runs/a.bbz")]})
    );
}

#[tokio::test]
async fn blob_404_conflict_bad_ranges_and_exhausted_retries() {
    let (base, requests) = mock(|_| {
        vec![
            Reply::new(404, Vec::new()),
            Reply::new(200, b"unranged".to_vec()),
            Reply::new(206, b"xx".to_vec()).header("Content-Range", "bytes 1-2/10"),
            Reply::new(206, b"x".to_vec()).header("Content-Range", "bytes 0-1/10"),
            Reply::new(500, Vec::new()),
            Reply::new(502, Vec::new()),
            Reply::new(503, Vec::new()),
            Reply::new(409, Vec::new()),
            Reply::new(404, Vec::new()),
        ]
    })
    .await;
    let store = mock_store(&base);
    assert!(matches!(
        store.get_range("a.bbz", 0..2).await,
        Err(ArtifactError::NotFound)
    ));
    for _ in 0..3 {
        assert!(matches!(
            store.get_range("a.bbz", 0..2).await,
            Err(ArtifactError::Integrity(_))
        ));
    }
    assert!(matches!(
        store.head("a.bbz").await,
        Err(ArtifactError::Http(503))
    ));
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("a.bbz");
    std::fs::write(&source, b"test").unwrap();
    assert!(matches!(
        store.put_bundle_from_file("a.bbz", &source).await,
        Err(ArtifactError::AlreadyExists)
    ));
    store.delete("a.bbz").await.unwrap();
    assert_eq!(requests.await.unwrap().len(), 9);
}
