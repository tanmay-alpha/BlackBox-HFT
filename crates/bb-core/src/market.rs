//! Market replay views derived from a dataset. All timestamps are on the exchange clock.

use serde::{Deserialize, Serialize};

use crate::{RelNs, Side};

/// Top of book at every change, columnar. `bid_tick[i]`/`ask_tick[i]` hold from `ts[i]` until
/// `ts[i + 1]`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BboSeries {
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<String>))]
    pub ts: Vec<RelNs>,
    /// `None` while that side of the reconstructed book is empty.
    pub bid_tick: Vec<Option<i64>>,
    pub ask_tick: Vec<Option<i64>>,
    pub bid_qty: Vec<f64>,
    pub ask_qty: Vec<f64>,
}

impl BboSeries {
    pub fn len(&self) -> usize {
        self.ts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ts.is_empty()
    }

    /// Index of the entry in force at `t` (the last entry with `ts <= t`).
    pub fn index_at(&self, t: RelNs) -> Option<usize> {
        let n = self.ts.partition_point(|x| *x <= t);
        n.checked_sub(1)
    }

    /// Mid price in fractional ticks at `t`, when both sides existed and the book was not crossed.
    pub fn mid_tick_at(&self, t: RelNs) -> Option<f64> {
        let i = self.index_at(t)?;
        match (self.bid_tick[i], self.ask_tick[i]) {
            (Some(b), Some(a)) if a > b => Some((a as f64 + b as f64) * 0.5),
            _ => None,
        }
    }
}

/// Market trades, columnar.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TradeSeries {
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<String>))]
    pub ts: Vec<RelNs>,
    pub price_tick: Vec<i64>,
    pub qty: Vec<f64>,
    /// Aggressor side when the data states it.
    pub side: Vec<Option<Side>>,
}

/// The reconstructed book at one instant: up to `levels` price levels per side, best first, as
/// `(price_tick, qty)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DepthFrame {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub ts: RelNs,
    pub bids: Vec<(i64, f64)>,
    pub asks: Vec<(i64, f64)>,
}

/// Everything the replay view needs for a time window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReplayWindow {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub t0: RelNs,
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub t1: RelNs,
    /// Spacing of `depth` frames.
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub step_ns: i64,
    pub levels: u32,
    /// Top of book, downsampled to at most one entry per `step_ns` when the window is long. When
    /// downsampled, each entry is the state at the end of its step (no averaging, no invention).
    pub bbo: BboSeries,
    pub bbo_downsampled: bool,
    pub trades: TradeSeries,
    /// `true` when the window held more trades than the requested cap and only the largest by
    /// quantity were kept.
    pub trades_truncated: bool,
    pub depth: Vec<DepthFrame>,
}
