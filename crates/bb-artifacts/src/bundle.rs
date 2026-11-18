use std::{
    borrow::Borrow,
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    ops::Range,
    path::Path,
};

use bb_core::{
    Clock,
    flight::FlightEvent,
    result::{
        EngineStateCheck, EquitySample, FillRecord, Metrics, OrderRecord, RunManifest, RunResult,
    },
};
use flate2::{Compression, GzBuilder, bufread::GzDecoder};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::{ArtifactError, ArtifactStore, integrity, io_error};

pub const FORMAT_VERSION: u32 = 1;
pub const MAX_PAGE_ROWS: usize = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Orders,
    Fills,
    Equity,
    Tape,
}
impl Section {
    pub const ALL: [Self; 4] = [Self::Orders, Self::Fills, Self::Equity, Self::Tape];
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleHeader {
    pub format: String,
    pub format_version: u32,
    pub fingerprint: String,
    pub platform: String,
    pub sections: [Section; 4],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberIndex {
    pub offset: u64,
    pub len: u64,
    pub sha256: String,
}
impl MemberIndex {
    pub fn range(&self) -> Result<Range<u64>, ArtifactError> {
        Ok(self.offset
            ..self
                .offset
                .checked_add(self.len)
                .ok_or_else(|| integrity("index", "offset overflow"))?)
    }
}

/// Timestamp maps are keyed by clock (`local` or `exchange`), never mixed across domains.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageIndex {
    pub section: Section,
    pub first_row: u64,
    pub rows: u64,
    pub offset: u64,
    pub len: u64,
    pub sha256: String,
    pub ts_min: BTreeMap<String, i64>,
    pub ts_max: BTreeMap<String, i64>,
}
impl PageIndex {
    pub fn member(&self) -> MemberIndex {
        MemberIndex {
            offset: self.offset,
            len: self.len,
            sha256: self.sha256.clone(),
        }
    }
    pub fn range(&self) -> Result<Range<u64>, ArtifactError> {
        Ok(self.offset
            ..self
                .offset
                .checked_add(self.len)
                .ok_or_else(|| integrity("index", "offset overflow"))?)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleCounts {
    pub ticks: u64,
    pub orders: u64,
    pub fills: u64,
    pub equity: u64,
    pub tape: u64,
}
impl BundleCounts {
    pub fn rows(&self, section: Section) -> u64 {
        match section {
            Section::Orders => self.orders,
            Section::Fills => self.fills,
            Section::Equity => self.equity,
            Section::Tape => self.tape,
        }
    }
    fn set(&mut self, section: Section, count: u64) {
        match section {
            Section::Orders => self.orders = count,
            Section::Fills => self.fills = count,
            Section::Equity => self.equity = count,
            Section::Tape => self.tape = count,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleTrailer {
    pub manifest: RunManifest,
    pub metrics: Metrics,
    pub engine_check: EngineStateCheck,
    pub counts: BundleCounts,
    pub result_sha256: String,
    pub summary_sha256: String,
    #[serde(with = "index_encoding")]
    pub page_index: Vec<PageIndex>,
    /// Research analytics of the run (bb-analytics report JSON), authenticated by the trailer
    /// member digest in [`BundleTrust`]. Absent in bundles written without analytics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analytics: Option<serde_json::Value>,
}

/// The verified [`BundleTrailer`] without its analytics report: what page reads and whole-run
/// reads need. The report is still authenticated (the member digest covers the whole trailer)
/// and checked to be well-formed JSON, but it is skipped instead of being built as a
/// `serde_json::Value`, which is many times the size of its text and the largest part of the
/// trailer (`docs/rebuild/HOSTED_MEMORY.md`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleTrailerCore {
    pub manifest: RunManifest,
    pub metrics: Metrics,
    pub engine_check: EngineStateCheck,
    pub counts: BundleCounts,
    pub result_sha256: String,
    pub summary_sha256: String,
    #[serde(with = "index_encoding")]
    pub page_index: Vec<PageIndex>,
    #[serde(default, rename = "analytics")]
    _analytics: Option<serde::de::IgnoredAny>,
}

/// Immutable database trust root. Keep the complete value, including manifest and member hashes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleTrust {
    pub format_version: u32,
    pub fingerprint: String,
    pub platform: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub uncompressed_bytes: u64,
    pub manifest_sha256: String,
    pub result_sha256: String,
    pub summary_sha256: String,
    pub counts: BundleCounts,
    pub header: MemberIndex,
    pub trailer: MemberIndex,
    #[serde(with = "index_encoding")]
    pub page_index: Vec<PageIndex>,
    pub page_index_sha256: String,
}

#[derive(Clone, Copy, Debug)]
pub struct BundleLimits {
    pub max_output_bytes: u64,
    pub max_page_bytes: usize,
    pub max_metadata_bytes: usize,
    pub max_pages: usize,
}
impl Default for BundleLimits {
    fn default() -> Self {
        Self {
            max_output_bytes: 1 << 30,
            max_page_bytes: 4 << 20,
            max_metadata_bytes: 32 << 20,
            max_pages: 100_000,
        }
    }
}
impl BundleLimits {
    fn validate(self) -> Result<(), ArtifactError> {
        if self.max_output_bytes == 0
            || self.max_page_bytes == 0
            || self.max_metadata_bytes == 0
            || self.max_pages == 0
        {
            return Err(ArtifactError::InvalidInput(
                "bundle limits must be positive".into(),
            ));
        }
        Ok(())
    }
}

pub struct VerifiedBundle {
    file: NamedTempFile,
    pub trust: BundleTrust,
}
impl VerifiedBundle {
    pub fn path(&self) -> &Path {
        self.file.path()
    }
    pub fn into_temp_file(self) -> NamedTempFile {
        self.file
    }
}

/// An analytics report as the trailer stores it: the compact serialization of the report as a
/// `serde_json::Value` (object keys sorted), exactly the text [`BundleTrailer::analytics`] reads
/// back. Built once, so a bundle write holds the report as text instead of a `Value` tree
/// (several times larger than its text) and never copies it.
pub struct TrailerAnalytics(Box<serde_json::value::RawValue>);
impl TrailerAnalytics {
    pub fn from_json(json: &str) -> Result<Self, ArtifactError> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        Ok(Self(serde_json::value::to_raw_value(&value)?))
    }
}

/// [`BundleTrailer`] as written: the same fields in the same order, borrowed, with the analytics
/// report as its stored text.
#[derive(Serialize)]
struct TrailerOut<'a> {
    manifest: &'a RunManifest,
    metrics: &'a Metrics,
    engine_check: &'a EngineStateCheck,
    counts: &'a BundleCounts,
    result_sha256: &'a str,
    summary_sha256: &'a str,
    page_index: IndexEncoding<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    analytics: Option<&'a serde_json::value::RawValue>,
}

pub struct BundleWriter {
    limits: BundleLimits,
    analytics: Option<TrailerAnalytics>,
}
impl BundleWriter {
    pub fn new(limits: BundleLimits) -> Result<Self, ArtifactError> {
        limits.validate()?;
        Ok(Self {
            limits,
            analytics: None,
        })
    }

    /// Embeds an analytics report in the trailer of every bundle this writer produces.
    pub fn with_analytics(mut self, analytics: TrailerAnalytics) -> Self {
        self.analytics = Some(analytics);
        self
    }

    pub fn write_run(
        &self,
        temp_dir: &Path,
        run: &RunResult,
    ) -> Result<VerifiedBundle, ArtifactError> {
        self.write(
            temp_dir,
            &run.manifest,
            &run.metrics,
            &run.engine_check,
            run.ticks,
            run.orders.iter(),
            run.fills.iter(),
            run.equity.iter(),
            run.tape.iter(),
        )
    }

    /// Iterators are consumed once, without collecting sections. Items may be owned or borrowed.
    /// Disk rereads establish the canonical science hash in tape/fills/equity order.
    #[allow(clippy::too_many_arguments)]
    pub fn write<O, F, E, T>(
        &self,
        temp_dir: &Path,
        manifest: &RunManifest,
        metrics: &Metrics,
        engine_check: &EngineStateCheck,
        ticks: u64,
        orders: O,
        fills: F,
        equity: E,
        tape: T,
    ) -> Result<VerifiedBundle, ArtifactError>
    where
        O: IntoIterator,
        O::Item: Borrow<OrderRecord>,
        F: IntoIterator,
        F::Item: Borrow<FillRecord>,
        E: IntoIterator,
        E::Item: Borrow<EquitySample>,
        T: IntoIterator,
        T::Item: Borrow<FlightEvent>,
    {
        let mut file = NamedTempFile::new_in(temp_dir)?;
        let header_value = BundleHeader {
            format: "blackbox-bundle".into(),
            format_version: FORMAT_VERSION,
            fingerprint: manifest.config_sha256.clone(),
            platform: manifest.platform.clone(),
            sections: Section::ALL,
        };
        let mut uncompressed_bytes = 0;
        let header = write_json_member(
            file.as_file_mut(),
            &header_value,
            self.limits,
            &mut uncompressed_bytes,
        )?;
        let mut pages = Vec::new();
        let mut counts = BundleCounts {
            ticks,
            ..BundleCounts::default()
        };
        counts.orders = write_section::<OrderRecord, _>(
            file.as_file_mut(),
            orders,
            &mut pages,
            self.limits,
            &mut uncompressed_bytes,
        )?;
        counts.fills = write_section::<FillRecord, _>(
            file.as_file_mut(),
            fills,
            &mut pages,
            self.limits,
            &mut uncompressed_bytes,
        )?;
        counts.equity = write_section::<EquitySample, _>(
            file.as_file_mut(),
            equity,
            &mut pages,
            self.limits,
            &mut uncompressed_bytes,
        )?;
        counts.tape = write_section::<FlightEvent, _>(
            file.as_file_mut(),
            tape,
            &mut pages,
            self.limits,
            &mut uncompressed_bytes,
        )?;
        let result_sha256 = science_hash(file.as_file_mut(), &pages, self.limits)?;
        if result_sha256 != manifest.result_sha256 {
            return Err(integrity(
                "result",
                "manifest differs from canonical tape/fills/equity",
            ));
        }
        let summary_sha256 = summary_sha256(metrics, engine_check, &counts)?;
        let trailer_value = TrailerOut {
            manifest,
            metrics,
            engine_check,
            counts: &counts,
            result_sha256: &result_sha256,
            summary_sha256: &summary_sha256,
            page_index: IndexEncoding { pages: &pages },
            analytics: self.analytics.as_ref().map(|a| &*a.0),
        };
        // science_hash seeks through the file; append, never overwrite a page.
        file.as_file_mut().seek(SeekFrom::End(0))?;
        let trailer = write_json_member(
            file.as_file_mut(),
            &trailer_value,
            self.limits,
            &mut uncompressed_bytes,
        )?;
        file.as_file_mut().sync_all()?;
        let size_bytes = file.as_file().metadata()?.len();
        let sha256 = hash_file(file.as_file_mut())?;
        let trust = BundleTrust {
            format_version: FORMAT_VERSION,
            fingerprint: manifest.config_sha256.clone(),
            platform: manifest.platform.clone(),
            sha256,
            size_bytes,
            uncompressed_bytes,
            manifest_sha256: json_hash(manifest)?,
            result_sha256,
            summary_sha256,
            counts,
            header,
            trailer,
            page_index_sha256: page_index_sha256(&pages)?,
            page_index: pages,
        };
        BundleReader::new(&trust, self.limits)?.verify_file(file.path())?;
        Ok(VerifiedBundle { file, trust })
    }
}

#[derive(Serialize)]
struct SummaryHash<'a> {
    metrics: &'a Metrics,
    engine_check: &'a EngineStateCheck,
    counts: &'a BundleCounts,
}
pub fn summary_sha256(
    metrics: &Metrics,
    engine_check: &EngineStateCheck,
    counts: &BundleCounts,
) -> Result<String, ArtifactError> {
    json_hash(&SummaryHash {
        metrics,
        engine_check,
        counts,
    })
}
/// Canonical database `page_index` object: `{"pages":[...]}` (not a bare JSON array).
pub fn page_index_json(pages: &[PageIndex]) -> Result<serde_json::Value, ArtifactError> {
    Ok(serde_json::to_value(IndexEncoding { pages })?)
}
pub fn page_index_sha256(pages: &[PageIndex]) -> Result<String, ArtifactError> {
    json_hash(&IndexEncoding { pages })
}
#[derive(Serialize)]
struct IndexEncoding<'a> {
    pages: &'a [PageIndex],
}
mod index_encoding {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        pages: &[PageIndex],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        IndexEncoding { pages }.serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<PageIndex>, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Index {
            pages: Vec<PageIndex>,
        }
        Ok(Index::deserialize(deserializer)?.pages)
    }
}
fn json_hash<T: Serialize + ?Sized>(value: &T) -> Result<String, ArtifactError> {
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, value)?;
    Ok(hex::encode(writer.0.finalize()))
}
struct HashWriter(Sha256);
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn hash_file(file: &mut File) -> Result<String, ArtifactError> {
    file.seek(SeekFrom::Start(0))?;
    Ok(bb_core::hash::sha256_reader(file)?)
}

// Serialization cannot grow a single malicious row beyond the configured page budget.
struct CappedVec {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for CappedVec {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("serialization byte budget exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encoded<T: Serialize + ?Sized>(value: &T, limit: usize) -> Result<Vec<u8>, ArtifactError> {
    let mut out = CappedVec {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut out, value).map_err(|e| {
        if e.is_io() {
            ArtifactError::LimitExceeded("serialized row/metadata")
        } else {
            e.into()
        }
    })?;
    if out.bytes.len() == limit {
        return Err(ArtifactError::LimitExceeded("serialized row/metadata"));
    }
    out.bytes.push(b'\n');
    Ok(out.bytes)
}
fn write_json_member<T: Serialize>(
    file: &mut File,
    value: &T,
    limits: BundleLimits,
    total: &mut u64,
) -> Result<MemberIndex, ArtifactError> {
    let bytes = encoded(value, limits.max_metadata_bytes)?;
    write_member(file, &bytes, limits, total)
}
fn write_member(
    file: &mut File,
    bytes: &[u8],
    limits: BundleLimits,
    total: &mut u64,
) -> Result<MemberIndex, ArtifactError> {
    let offset = file.stream_position()?;
    let mut gzip = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::default());
    gzip.write_all(bytes)?;
    let compressed = gzip.finish()?;
    let len = compressed.len() as u64;
    if offset
        .checked_add(len)
        .is_none_or(|n| n > limits.max_output_bytes)
    {
        return Err(ArtifactError::LimitExceeded("compressed bundle"));
    }
    file.write_all(&compressed)?;
    *total = total
        .checked_add(bytes.len() as u64)
        .ok_or(ArtifactError::LimitExceeded("uncompressed bundle"))?;
    Ok(MemberIndex {
        offset,
        len,
        sha256: bb_core::hash::sha256_hex(&compressed),
    })
}

trait Row: Serialize + DeserializeOwned {
    const SECTION: Section;
    fn timestamp(&self) -> (&'static str, i64);
}
impl Row for OrderRecord {
    const SECTION: Section = Section::Orders;
    fn timestamp(&self) -> (&'static str, i64) {
        ("local", self.decision_ts)
    }
}
impl Row for FillRecord {
    const SECTION: Section = Section::Fills;
    fn timestamp(&self) -> (&'static str, i64) {
        ("exchange", self.ts)
    }
}
impl Row for EquitySample {
    const SECTION: Section = Section::Equity;
    fn timestamp(&self) -> (&'static str, i64) {
        ("local", self.ts)
    }
}
impl Row for FlightEvent {
    const SECTION: Section = Section::Tape;
    fn timestamp(&self) -> (&'static str, i64) {
        (
            match self.clock {
                Clock::Local => "local",
                Clock::Exchange => "exchange",
            },
            self.ts,
        )
    }
}
fn timestamp_add(
    min: &mut BTreeMap<String, i64>,
    max: &mut BTreeMap<String, i64>,
    (clock, ts): (&str, i64),
) {
    min.entry(clock.into())
        .and_modify(|n| *n = (*n).min(ts))
        .or_insert(ts);
    max.entry(clock.into())
        .and_modify(|n| *n = (*n).max(ts))
        .or_insert(ts);
}
fn write_section<R: Row, I: IntoIterator>(
    file: &mut File,
    rows: I,
    pages: &mut Vec<PageIndex>,
    limits: BundleLimits,
    total: &mut u64,
) -> Result<u64, ArtifactError>
where
    I::Item: Borrow<R>,
{
    let mut bytes = Vec::new();
    let (mut first, mut count) = (0, 0);
    let (mut min, mut max) = (BTreeMap::new(), BTreeMap::new());
    for row in rows {
        let row: &R = row.borrow();
        let line = encoded(row, limits.max_page_bytes)?;
        if count > 0
            && (count == MAX_PAGE_ROWS as u64 || line.len() > limits.max_page_bytes - bytes.len())
        {
            flush_page(
                file,
                R::SECTION,
                first,
                count,
                &bytes,
                &mut min,
                &mut max,
                pages,
                limits,
                total,
            )?;
            first += count;
            count = 0;
            bytes.clear();
        }
        timestamp_add(&mut min, &mut max, row.timestamp());
        bytes.extend_from_slice(&line);
        count += 1;
    }
    if count > 0 {
        flush_page(
            file,
            R::SECTION,
            first,
            count,
            &bytes,
            &mut min,
            &mut max,
            pages,
            limits,
            total,
        )?;
    }
    Ok(first + count)
}
#[allow(clippy::too_many_arguments)]
fn flush_page(
    file: &mut File,
    section: Section,
    first_row: u64,
    rows: u64,
    bytes: &[u8],
    min: &mut BTreeMap<String, i64>,
    max: &mut BTreeMap<String, i64>,
    pages: &mut Vec<PageIndex>,
    limits: BundleLimits,
    total: &mut u64,
) -> Result<(), ArtifactError> {
    if pages.len() == limits.max_pages {
        return Err(ArtifactError::LimitExceeded("page index"));
    }
    let member = write_member(file, bytes, limits, total)?;
    pages.push(PageIndex {
        section,
        first_row,
        rows,
        offset: member.offset,
        len: member.len,
        sha256: member.sha256,
        ts_min: std::mem::take(min),
        ts_max: std::mem::take(max),
    });
    Ok(())
}

/// Reader borrows a database-sourced trust root; it never promotes untrusted metadata to trust.
pub struct BundleReader<'a> {
    trust: &'a BundleTrust,
    limits: BundleLimits,
}
impl<'a> BundleReader<'a> {
    pub fn new(trust: &'a BundleTrust, limits: BundleLimits) -> Result<Self, ArtifactError> {
        limits.validate()?;
        let reader = Self { trust, limits };
        reader.validate_index()?;
        Ok(reader)
    }
    fn validate_index(&self) -> Result<(), ArtifactError> {
        let t = self.trust;
        if t.format_version != FORMAT_VERSION
            || t.size_bytes > self.limits.max_output_bytes
            || t.page_index.len() > self.limits.max_pages
        {
            return Err(integrity("index", "unsupported version or budget"));
        }
        for hash in [
            &t.sha256,
            &t.manifest_sha256,
            &t.result_sha256,
            &t.summary_sha256,
            &t.page_index_sha256,
            &t.fingerprint,
        ] {
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(integrity("index", "invalid sha256"));
            }
        }
        if page_index_sha256(&t.page_index)? != t.page_index_sha256 {
            return Err(integrity("index", "page index hash mismatch"));
        }
        if t.header.offset != 0 || t.header.len == 0 {
            return Err(integrity("index", "invalid header range"));
        }
        let mut offset = t.header.range()?.end;
        let mut counts = BundleCounts {
            ticks: t.counts.ticks,
            ..BundleCounts::default()
        };
        let mut last = Section::Orders;
        for p in &t.page_index {
            if p.section < last
                || p.offset != offset
                || p.len == 0
                || p.rows == 0
                || p.rows > MAX_PAGE_ROWS as u64
                || p.first_row != counts.rows(p.section)
            {
                return Err(integrity(
                    "index",
                    "noncontiguous pages or invalid row count",
                ));
            }
            offset = p.range()?.end;
            counts.set(
                p.section,
                p.first_row
                    .checked_add(p.rows)
                    .ok_or_else(|| integrity("index", "row overflow"))?,
            );
            last = p.section;
        }
        if counts != t.counts
            || t.trailer.offset != offset
            || t.trailer.len == 0
            || t.trailer.range()?.end != t.size_bytes
        {
            return Err(integrity("index", "counts or bundle extent mismatch"));
        }
        Ok(())
    }
    /// The verified trailer, without its analytics report, from the raw header and trailer
    /// members (e.g. two ranged reads of the stored bundle), checked against the trust root; no
    /// page is read.
    pub fn check_metadata(
        &self,
        header: &[u8],
        trailer: &[u8],
    ) -> Result<BundleTrailerCore, ArtifactError> {
        Ok(self.checked_metadata(header, trailer)?.0)
    }
    /// Checks the header and trailer members against the trust root. Returns the trailer
    /// without its analytics report, and the decoded trailer row for a caller that needs the
    /// complete [`BundleTrailer`].
    fn checked_metadata(
        &self,
        header: &[u8],
        trailer: &[u8],
    ) -> Result<(BundleTrailerCore, Vec<u8>), ArtifactError> {
        self.validate_index()?;
        let h: BundleHeader =
            decode_json(header, &self.trust.header, self.limits.max_metadata_bytes)?;
        let row = decode_member(trailer, &self.trust.trailer, self.limits.max_metadata_bytes)?;
        let t: BundleTrailerCore = parse_row(&row)?;
        if h.format != "blackbox-bundle"
            || h.format_version != FORMAT_VERSION
            || h.fingerprint != self.trust.fingerprint
            || h.platform != self.trust.platform
            || h.sections != Section::ALL
        {
            return Err(integrity("header", "identity or format mismatch"));
        }
        if json_hash(&t.manifest)? != self.trust.manifest_sha256
            || t.manifest.config_sha256 != h.fingerprint
            || t.manifest.platform != h.platform
            || t.manifest.result_sha256 != self.trust.result_sha256
            || t.result_sha256 != self.trust.result_sha256
            || t.summary_sha256 != self.trust.summary_sha256
            || summary_sha256(&t.metrics, &t.engine_check, &t.counts)? != self.trust.summary_sha256
            || t.counts != self.trust.counts
            || t.page_index != self.trust.page_index
        {
            return Err(integrity("trailer", "manifest, summary or index mismatch"));
        }
        Ok((t, row))
    }
    fn metadata_file(
        &self,
        file: &mut File,
    ) -> Result<(BundleTrailerCore, Vec<u8>), ArtifactError> {
        if file.metadata()?.len() != self.trust.size_bytes {
            return Err(integrity("bundle", "size mismatch"));
        }
        let header = read_member(file, &self.trust.header, self.limits.max_metadata_bytes)?;
        let trailer = read_member(file, &self.trust.trailer, self.limits.max_metadata_bytes)?;
        self.checked_metadata(&header, &trailer)
    }
    pub fn verify_file(&self, path: &Path) -> Result<BundleTrailerCore, ArtifactError> {
        let mut file = File::open(path).map_err(io_error)?;
        let (trailer, _) = self.metadata_file(&mut file)?;
        if hash_file(&mut file)? != self.trust.sha256 {
            return Err(integrity("bundle", "sha256 mismatch"));
        }
        let mut total = decode_member(
            &read_member(
                &mut file,
                &self.trust.header,
                self.limits.max_metadata_bytes,
            )?,
            &self.trust.header,
            self.limits.max_metadata_bytes,
        )?
        .len() as u64;
        total += decode_member(
            &read_member(
                &mut file,
                &self.trust.trailer,
                self.limits.max_metadata_bytes,
            )?,
            &self.trust.trailer,
            self.limits.max_metadata_bytes,
        )?
        .len() as u64;
        for page in &self.trust.page_index {
            let raw = read_member(&mut file, page, self.limits.max_page_bytes)?;
            let bytes = decode_member(&raw, page, self.limits.max_page_bytes)?;
            validate_page(&bytes, page)?;
            total = total
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| integrity("bundle", "byte count overflow"))?;
        }
        if total != self.trust.uncompressed_bytes {
            return Err(integrity("bundle", "uncompressed byte count mismatch"));
        }
        if science_hash(&mut file, &self.trust.page_index, self.limits)? != self.trust.result_sha256
        {
            return Err(integrity("result", "canonical hash mismatch"));
        }
        Ok(trailer)
    }
    pub fn trailer(&self, path: &Path) -> Result<BundleTrailer, ArtifactError> {
        parse_row(
            &self
                .metadata_file(&mut File::open(path).map_err(io_error)?)?
                .1,
        )
    }
    /// [`Self::trailer`] without building the analytics report.
    pub fn trailer_core(&self, path: &Path) -> Result<BundleTrailerCore, ArtifactError> {
        Ok(self
            .metadata_file(&mut File::open(path).map_err(io_error)?)?
            .0)
    }
    /// The authenticated analytics report as stored JSON text, without building its values.
    /// Older bundles without a report return `None`. Metadata and the complete compressed
    /// trailer digest are checked before any report text is returned.
    pub fn trailer_analytics_json(&self, path: &Path) -> Result<Option<String>, ArtifactError> {
        #[derive(Deserialize)]
        struct Report<'a> {
            #[serde(borrow)]
            analytics: Option<&'a serde_json::value::RawValue>,
        }

        let (_, row) = self.metadata_file(&mut File::open(path).map_err(io_error)?)?;
        let report: Report<'_> =
            serde_json::from_slice(&row).map_err(|e| integrity("metadata", e.to_string()))?;
        Ok(report.analytics.map(|raw| raw.get().to_owned()))
    }
    /// Returns verified uncompressed NDJSON for one index entry (never the whole section).
    pub fn page(&self, path: &Path, page_no: usize) -> Result<Vec<u8>, ArtifactError> {
        let mut file = File::open(path).map_err(io_error)?;
        self.metadata_file(&mut file)?;
        let page = self
            .trust
            .page_index
            .get(page_no)
            .ok_or_else(|| ArtifactError::InvalidInput("page index out of range".into()))?;
        let raw = read_member(&mut file, page, self.limits.max_page_bytes)?;
        self.decode_page(&raw, page)
    }
    /// The verified trailer (without its analytics report), then the verified uncompressed
    /// NDJSON of each of `page_nos` in turn, with one metadata check for all of them (the
    /// trailer can be large: checking it per page dominates reading a whole run).
    pub fn trailer_and_pages(
        &self,
        path: &Path,
        page_nos: &[usize],
        mut each: impl FnMut(usize, Vec<u8>) -> Result<(), ArtifactError>,
    ) -> Result<BundleTrailerCore, ArtifactError> {
        let mut file = File::open(path).map_err(io_error)?;
        let (trailer, _) = self.metadata_file(&mut file)?;
        for &page_no in page_nos {
            let page = self
                .trust
                .page_index
                .get(page_no)
                .ok_or_else(|| ArtifactError::InvalidInput("page index out of range".into()))?;
            let raw = read_member(&mut file, page, self.limits.max_page_bytes)?;
            each(page_no, self.decode_page(&raw, page)?)?;
        }
        Ok(trailer)
    }
    fn decode_page(&self, raw: &[u8], page: &PageIndex) -> Result<Vec<u8>, ArtifactError> {
        let bytes = decode_member(raw, page, self.limits.max_page_bytes)?;
        validate_page(&bytes, page)?;
        Ok(bytes)
    }
    pub async fn page_from_store(
        &self,
        store: &dyn ArtifactStore,
        key: &str,
        page_no: usize,
    ) -> Result<Vec<u8>, ArtifactError> {
        self.validate_index()?;
        let page = self
            .trust
            .page_index
            .get(page_no)
            .ok_or_else(|| ArtifactError::InvalidInput("page index out of range".into()))?;
        let header = store.get_range(key, self.trust.header.range()?).await?;
        let trailer = store.get_range(key, self.trust.trailer.range()?).await?;
        self.checked_metadata(&header, &trailer)?;
        let raw = store.get_range(key, page.range()?).await?;
        self.decode_page(&raw, page)
    }
    /// Reads a row range, verifying metadata and every touched page before returning any rows.
    /// Output is bounded by the page byte budget; use multiple calls for larger ranges.
    pub fn rows(
        &self,
        path: &Path,
        section: Section,
        range: Range<u64>,
    ) -> Result<Vec<u8>, ArtifactError> {
        if range.start > range.end || range.end > self.trust.counts.rows(section) {
            return Err(ArtifactError::InvalidInput(
                "row range out of bounds".into(),
            ));
        }
        let mut file = File::open(path).map_err(io_error)?;
        self.metadata_file(&mut file)?;
        let mut output = Vec::new();
        for page in self.trust.page_index.iter().filter(|p| {
            p.section == section && p.first_row < range.end && p.first_row + p.rows > range.start
        }) {
            let raw = read_member(&mut file, page, self.limits.max_page_bytes)?;
            let bytes = self.decode_page(&raw, page)?;
            for (i, line) in lines(&bytes)?.enumerate() {
                let row = page.first_row + i as u64;
                if range.contains(&row) {
                    if output.len() + line.len() + 1 > self.limits.max_page_bytes {
                        return Err(ArtifactError::LimitExceeded("row range response"));
                    }
                    output.extend_from_slice(line);
                    output.push(b'\n');
                }
            }
        }
        Ok(output)
    }
}

trait CompressedMember {
    fn descriptor(&self) -> (u64, u64, &str);
}
impl CompressedMember for MemberIndex {
    fn descriptor(&self) -> (u64, u64, &str) {
        (self.offset, self.len, &self.sha256)
    }
}
impl CompressedMember for PageIndex {
    fn descriptor(&self) -> (u64, u64, &str) {
        (self.offset, self.len, &self.sha256)
    }
}
fn read_member(
    file: &mut File,
    index: &impl CompressedMember,
    max: usize,
) -> Result<Vec<u8>, ArtifactError> {
    let (offset, len, _) = index.descriptor();
    // DEFLATE overhead can make incompressible members slightly larger than the input cap.
    let cap = max.saturating_add(max / 100).saturating_add(1024);
    if len > cap as u64 {
        return Err(integrity("member", "compressed member exceeds budget"));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut raw = vec![0; len as usize];
    file.read_exact(&mut raw).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            integrity("member", "truncated member")
        } else {
            e.into()
        }
    })?;
    Ok(raw)
}
fn decode_member(
    raw: &[u8],
    index: &impl CompressedMember,
    max: usize,
) -> Result<Vec<u8>, ArtifactError> {
    let (_, len, sha256) = index.descriptor();
    if raw.len() as u64 != len || bb_core::hash::sha256_hex(raw) != sha256 {
        return Err(integrity("member", "compressed sha256 mismatch"));
    }
    let mut gzip = GzDecoder::new(raw);
    let mut bytes = Vec::new();
    (&mut gzip)
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| integrity("member", format!("gzip: {e}")))?;
    if bytes.len() > max || !gzip.get_ref().is_empty() {
        return Err(integrity("member", "oversized or trailing member bytes"));
    }
    Ok(bytes)
}
fn decode_json<T: DeserializeOwned>(
    raw: &[u8],
    index: &MemberIndex,
    max: usize,
) -> Result<T, ArtifactError> {
    parse_row(&decode_member(raw, index, max)?)
}
/// The single NDJSON row of a decoded metadata member.
fn parse_row<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ArtifactError> {
    let mut rows = lines(bytes)?;
    let line = rows
        .next()
        .ok_or_else(|| integrity("metadata", "empty member"))?;
    if rows.next().is_some() {
        return Err(integrity("metadata", "multiple metadata rows"));
    }
    serde_json::from_slice(line).map_err(|e| integrity("metadata", e.to_string()))
}
fn lines(bytes: &[u8]) -> Result<impl Iterator<Item = &[u8]>, ArtifactError> {
    if bytes.last() != Some(&b'\n') {
        return Err(integrity("page", "missing NDJSON terminator"));
    }
    Ok(bytes[..bytes.len() - 1].split(|b| *b == b'\n'))
}
fn validate_page(bytes: &[u8], page: &PageIndex) -> Result<(), ArtifactError> {
    match page.section {
        Section::Orders => validate_rows::<OrderRecord>(bytes, page),
        Section::Fills => validate_rows::<FillRecord>(bytes, page),
        Section::Equity => validate_rows::<EquitySample>(bytes, page),
        Section::Tape => validate_rows::<FlightEvent>(bytes, page),
    }
}
fn validate_rows<R: Row>(bytes: &[u8], page: &PageIndex) -> Result<(), ArtifactError> {
    let (mut min, mut max) = (BTreeMap::new(), BTreeMap::new());
    let mut count = 0;
    for line in lines(bytes)? {
        let row: R = serde_json::from_slice(line).map_err(|e| integrity("page", e.to_string()))?;
        timestamp_add(&mut min, &mut max, row.timestamp());
        count += 1;
    }
    if count != page.rows || min != page.ts_min || max != page.ts_max {
        return Err(integrity("page", "row count or clock ranges mismatch"));
    }
    Ok(())
}
fn science_hash(
    file: &mut File,
    pages: &[PageIndex],
    limits: BundleLimits,
) -> Result<String, ArtifactError> {
    let mut hash = HashWriter(Sha256::new());
    hash.write_all(b"[")?;
    for (i, section) in [Section::Tape, Section::Fills, Section::Equity]
        .into_iter()
        .enumerate()
    {
        if i > 0 {
            hash.write_all(b",")?;
        }
        hash.write_all(b"[")?;
        let mut first = true;
        for page in pages.iter().filter(|p| p.section == section) {
            let raw = read_member(file, page, limits.max_page_bytes)?;
            let bytes = decode_member(&raw, page, limits.max_page_bytes)?;
            for line in lines(&bytes)? {
                if !first {
                    hash.write_all(b",")?;
                }
                first = false;
                // Pages contain the writer's exact canonical bb-core JSON. Joining those bytes
                // preserves original f64 encodings without a parse/round-trip changing bits.
                hash.write_all(line)?;
            }
        }
        hash.write_all(b"]")?;
    }
    hash.write_all(b"]")?;
    Ok(hex::encode(hash.0.finalize()))
}
