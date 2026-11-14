//! Curated, ready-to-run configurations.
//!
//! Presets are sized relative to the dataset's instrument (quantities in lots, price distances in
//! ticks), so the same catalogue is meaningful on any dataset. Four of them share one strategy
//! and differ in exactly one execution assumption each, which is what makes them comparable.

use bb_core::{
    OrdType, Side, Tif,
    config::{
        AvellanedaStoikovParams, CubicLeg, ExchangeModel, ExecutionConfig, FeeConfig, GlftParams,
        ImbalanceTakerParams, LogNormalLeg, OrderLatencyConfig, QueueModelConfig, RecordConfig,
        RiskConfig, RunConfig, ScriptedOrder, ScriptedParams, ScriptedPrice, SigmaSource,
        StrategyConfig, StrategyKind, TouchMakerParams,
    },
    dataset::{DatasetManifest, InstrumentMeta, Quantiles},
    time::NS_PER_MS,
};

use crate::api::Preset;

/// Strategy evaluation interval of every preset: 100 ms on the local clock.
const INTERVAL_NS: i64 = 100 * NS_PER_MS;

const ORDER_LOTS: f64 = 10.0;
const MAX_POSITION_LOTS: f64 = 100.0;
const MAX_ORDER_LOTS: f64 = 50.0;

/// Baseline order latency, each way.
const BASE_LATENCY_NS: i64 = NS_PER_MS;
/// Order latency of the "slower line" scenario, each way.
const SLOW_LATENCY_NS: i64 = 50 * NS_PER_MS;

/// Touch-maker inventory skew, in ticks, reached at the position limit.
const SKEW_TICKS_AT_MAX_POSITION: f64 = 2.0;

// Avellaneda-Stoikov parameters, expressed in units of 1/tick so the quotes are
// instrument-relative. The strategy's half spread in price units is
//
//   delta = 0.5 * gamma * sigma^2 * horizon + (1 / gamma) * ln(1 + gamma / kappa)
//
// with gamma = AS_GAMMA_PER_TICK / tick_size, kappa = AS_KAPPA_PER_TICK / tick_size and
// horizon = AS_HORIZON_S:
//
//   liquidity term  = (tick / 0.1) * ln(1 + 0.1 / 0.5) = 10 * ln(1.2) ticks  = 1.82 ticks
//   volatility term = 0.5 * (0.1 / tick) * sigma^2 * 1 s = 0.05 * (sigma / tick)^2 ticks
//                     (0.05 ticks at 1 tick per sqrt(second), 1.25 ticks at 5)
//
// so the half spread is about 2 ticks in a quiet market and widens as measured volatility
// rises. The reservation price moves by position * gamma * sigma^2 * horizon
// = 0.1 * position * (sigma / tick)^2 ticks, with the position in base units.
const AS_GAMMA_PER_TICK: f64 = 0.1;
const AS_KAPPA_PER_TICK: f64 = 0.5;
const AS_HORIZON_S: f64 = 1.0;
/// 50 ticks of 100 ms: volatility is estimated over roughly the last 5 seconds.
const AS_VOL_HALFLIFE_TICKS: u32 = 50;

/// Log-normal latency preset: sigma of ln(latency) on every leg, around the baseline's 1 ms
/// median. Deciles: median * e^(-/+ 0.5 * 1.2816) = 0.527 ms and 1.898 ms.
const LOG_NORMAL_SIGMA: f64 = 0.5;

/// Heavy-tailed latency preset: ABIDES' default cubic shape (`jitter` 0.5, `clip` 0.1, `unit`
/// 10) on the baseline's 1 ms as the minimum. Each message takes 1 ms + 0.05 ms / x^3 with x
/// uniform on (0.1, 1]: between 1.05 ms and 51 ms, median 1.30 ms, mean 3.75 ms, one in ten
/// above 8.29 ms.
const CUBIC_LEG: CubicLeg = CubicLeg {
    min_ns: BASE_LATENCY_NS,
    jitter: 0.5,
    clip: 0.1,
    unit: 10.0,
};

// GLFT parameters, instrument-relative: gamma is per (tick x order size), kappa per tick, so
// with gamma = GLFT_GAMMA / (tick * Delta) and kappa = GLFT_KAPPA_PER_TICK / tick:
//
//   c1    = ln(1 + 0.1 / 0.5) / 0.1 ticks = 1.82 ticks (the Avellaneda-Stoikov preset's
//           liquidity term)
//   omega = (tick / Delta) * (sigma / tick) * sqrt(0.1 * 1.2^6 / A) = 0.546 (sigma / tick)
//           ticks per order size of inventory at A = 1 per second
//
// so the flat half spread is 1.82 + 0.27 (sigma / tick) ticks and each order size of inventory
// shifts both quotes by 0.55 (sigma / tick) ticks.
const GLFT_GAMMA: f64 = 0.1;
const GLFT_KAPPA_PER_TICK: f64 = 0.5;
/// Fills per second of a quote at the mid: an assumption, not calibrated to the data.
const GLFT_INTENSITY_A: f64 = 1.0;

/// The preset catalogue for the dataset described by `manifest`.
pub fn presets(manifest: &DatasetManifest) -> Vec<Preset> {
    build(
        &manifest.id,
        &manifest.instrument,
        manifest.quality.spread_ticks.as_ref(),
    )
}

/// The preset with the given id, if the catalogue has one.
pub fn find(manifest: &DatasetManifest, id: &str) -> Option<Preset> {
    presets(manifest).into_iter().find(|p| p.id == id)
}

/// Quantities shared by every preset, in base units.
#[derive(Copy, Clone)]
struct Sizing {
    order_qty: f64,
    max_position: f64,
    max_order_qty: f64,
}

fn base_execution() -> ExecutionConfig {
    ExecutionConfig {
        exchange_model: ExchangeModel::NoPartialFill,
        queue_model: QueueModelConfig::PowerProb3 { n: 3.0 },
        order_latency: OrderLatencyConfig::Constant {
            entry_ns: BASE_LATENCY_NS,
            response_ns: BASE_LATENCY_NS,
        },
        think_time_ns: 0,
        feed_latency_offset_ns: 0,
        fees: FeeConfig {
            maker: 0.0002,
            taker: 0.0004,
        },
    }
}

fn base_config(dataset_id: &str, sizing: Sizing, kind: StrategyKind) -> RunConfig {
    RunConfig {
        dataset_id: dataset_id.to_string(),
        strategy: StrategyConfig {
            interval_ns: INTERVAL_NS,
            kind,
        },
        execution: base_execution(),
        risk: RiskConfig {
            max_position: sizing.max_position,
            max_order_qty: sizing.max_order_qty,
            max_open_orders: 8,
            max_drawdown: None,
        },
        record: RecordConfig::default(),
        seed: 1,
    }
}

fn touch_maker(sizing: Sizing) -> StrategyKind {
    StrategyKind::TouchMaker(TouchMakerParams {
        order_qty: sizing.order_qty,
        max_position: sizing.max_position,
        offset_ticks: 0,
        // The skew is per unit of position, so it is scaled by the position limit: quotes shift
        // by one tick from a quarter of the limit and by two ticks at the limit.
        skew_ticks_per_unit: SKEW_TICKS_AT_MAX_POSITION / sizing.max_position,
    })
}

/// How many ticks above the best bid a buy must be priced to reach the ask. The 99th percentile
/// of the dataset's observed spread is used when the quality report has it; otherwise one tick,
/// the narrowest spread a valid book can have.
fn crossing_offset(spread_ticks: Option<&Quantiles>) -> i64 {
    spread_ticks
        .map(|q| q.p99.ceil())
        .filter(|ticks| ticks.is_finite() && *ticks >= 1.0)
        .map_or(1, |ticks| ticks as i64)
}

fn probe_orders(order_qty: f64, cross: i64) -> Vec<ScriptedOrder> {
    let order = |at_tick: u64,
                 side: Side,
                 offset: i64,
                 tif: Tif,
                 cancel_after_ticks: Option<u64>,
                 label: String| ScriptedOrder {
        at_tick,
        slot: 0,
        side,
        price: ScriptedPrice::FromTouch { offset },
        qty: order_qty,
        tif,
        ord_type: OrdType::Limit,
        cancel_after_ticks,
        label,
    };
    vec![
        order(
            20,
            Side::Buy,
            -1,
            Tif::Gtx,
            Some(50),
            "probe 1: post-only buy 1 tick below the best bid, cancel requested 50 ticks later"
                .to_string(),
        ),
        order(
            25,
            Side::Buy,
            0,
            Tif::Gtc,
            None,
            "probe 2: buy joining the best bid, behind the quantity already displayed there"
                .to_string(),
        ),
        order(
            30,
            Side::Sell,
            0,
            Tif::Gtc,
            None,
            "probe 3: sell joining the best ask, behind the quantity already displayed there"
                .to_string(),
        ),
        order(
            35,
            Side::Buy,
            cross,
            Tif::Ioc,
            None,
            format!(
                "probe 4: immediate-or-cancel buy at best bid + {cross} ticks, which reaches the \
                 ask when the spread is at most {cross} ticks"
            ),
        ),
        order(
            40,
            Side::Buy,
            cross,
            Tif::Gtx,
            None,
            format!(
                "probe 5: post-only buy at best bid + {cross} ticks, which would cross the ask \
                 when the spread is at most {cross} ticks"
            ),
        ),
    ]
}

fn build(
    dataset_id: &str,
    instrument: &InstrumentMeta,
    spread_ticks: Option<&Quantiles>,
) -> Vec<Preset> {
    let lot = instrument.lot_size;
    let tick = instrument.tick_size;
    let sizing = Sizing {
        order_qty: ORDER_LOTS * lot,
        max_position: MAX_POSITION_LOTS * lot,
        max_order_qty: MAX_ORDER_LOTS * lot,
    };
    let maker = || base_config(dataset_id, sizing, touch_maker(sizing));

    let maker_baseline = maker();

    let mut maker_slow = maker();
    maker_slow.execution.order_latency = OrderLatencyConfig::Constant {
        entry_ns: SLOW_LATENCY_NS,
        response_ns: SLOW_LATENCY_NS,
    };

    let mut maker_conservative_queue = maker();
    maker_conservative_queue.execution.queue_model = QueueModelConfig::RiskAdverse;

    let mut maker_rebate = maker();
    maker_rebate.execution.fees.maker = -0.0001;

    let avellaneda_stoikov = base_config(
        dataset_id,
        sizing,
        StrategyKind::AvellanedaStoikov(AvellanedaStoikovParams {
            order_qty: sizing.order_qty,
            max_position: sizing.max_position,
            gamma: AS_GAMMA_PER_TICK / tick,
            kappa: AS_KAPPA_PER_TICK / tick,
            horizon_s: AS_HORIZON_S,
            vol_halflife_ticks: AS_VOL_HALFLIFE_TICKS,
        }),
    );

    let imbalance_taker = base_config(
        dataset_id,
        sizing,
        StrategyKind::ImbalanceTaker(ImbalanceTakerParams {
            order_qty: sizing.order_qty,
            max_position: sizing.max_position,
            threshold: 0.6,
            hold_ticks: 20,
            cooldown_ticks: 10,
        }),
    );

    let cross = crossing_offset(spread_ticks);
    let forensics_probe = base_config(
        dataset_id,
        sizing,
        StrategyKind::Scripted(ScriptedParams {
            orders: probe_orders(sizing.order_qty, cross),
        }),
    );

    let mut latency_lognormal = maker();
    let leg = LogNormalLeg {
        median_ns: BASE_LATENCY_NS,
        sigma: LOG_NORMAL_SIGMA,
    };
    latency_lognormal.execution.order_latency = OrderLatencyConfig::LogNormal {
        entry: leg,
        response: leg,
        cancel: None,
    };

    let mut latency_heavy_tail = maker();
    latency_heavy_tail.execution.order_latency = OrderLatencyConfig::HeavyTail {
        entry: CUBIC_LEG,
        response: CUBIC_LEG,
        cancel: None,
    };

    let glft_baseline = base_config(
        dataset_id,
        sizing,
        StrategyKind::Glft(GlftParams {
            order_qty: sizing.order_qty,
            max_position: sizing.max_position,
            gamma: GLFT_GAMMA / (tick * sizing.order_qty),
            kappa: GLFT_KAPPA_PER_TICK / tick,
            intensity_a: GLFT_INTENSITY_A,
            sigma_source: SigmaSource::WeightedMid,
            vol_halflife_ticks: AS_VOL_HALFLIFE_TICKS,
        }),
    );

    let preset =
        |id: &str, title: &str, description: &str, demonstrates: &str, config: RunConfig| Preset {
            id: id.to_string(),
            title: title.to_string(),
            description: description.to_string(),
            demonstrates: demonstrates.to_string(),
            config,
        };

    vec![
        preset(
            "maker_baseline",
            "Touch maker: baseline",
            "Joins the best bid and best ask every 100 ms with post-only orders of 10 lots, \
             skewing quotes away from inventory (position limit 100 lots). Execution \
             assumptions: 1 ms order latency each way, the PowerProb3 (n = 3) queue model, \
             whole-order fills, 2 bp maker and 4 bp taker fees.",
            "The reference scenario: how often a passive quote at the touch is modeled to fill, \
             and what those fills earn after adverse selection and fees.",
            maker_baseline,
        ),
        preset(
            "maker_slow",
            "Touch maker: slower line",
            "The baseline strategy with 50 ms order latency each way instead of 1 ms. Nothing \
             else differs, so comparing it with the baseline isolates the latency assumption.",
            "Same strategy, slower line: every quote and every cancel reaches the simulated \
             exchange 50 ms after the decision, by which time the market may have moved.",
            maker_slow,
        ),
        preset(
            "maker_conservative_queue",
            "Touch maker: conservative queue model",
            "The baseline strategy under the RiskAdverse queue model, which advances an order \
             only when trades print at its price and never credits cancellations ahead of it.",
            "How much of the baseline's fill rate rests on the assumption that part of every \
             depth decrease happened ahead of the order.",
            maker_conservative_queue,
        ),
        preset(
            "maker_rebate",
            "Touch maker: maker rebate",
            "The baseline strategy with a maker rebate of 1 bp instead of a 2 bp maker fee. \
             Fees do not influence this strategy's decisions, so orders and fills are the same \
             as the baseline and only the accounting changes.",
            "The fee schedule's share of a passive strategy's simulated result.",
            maker_rebate,
        ),
        preset(
            "avellaneda_stoikov",
            "Avellaneda-Stoikov market maker",
            "Quotes around an inventory-adjusted reservation price with the Avellaneda-Stoikov \
             (2008) spread. Parameters are scaled to the tick size so the half spread is about \
             2 ticks in a quiet market and widens with measured volatility; it stays silent \
             until its volatility estimate has warmed up. Caveats: the remaining horizon T - t \
             is held at 1 s, which makes the paper's finite-horizon formulas a near-terminal \
             approximation rather than a stationary policy (Guéant, Lehalle and \
             Fernandez-Tapia 2013); the paper's fills are one unit each, and the formulas are \
             applied to 10-lot fills without the trade-size scaling of Guéant (2017); volatility \
             is estimated from the local mid, which also jumps whenever one side of a wide book \
             moves. The glft_baseline preset uses stationary formulas, an explicit trade size \
             and a size-weighted volatility input instead.",
            "A model-driven maker that quotes behind the touch and trades fill frequency for \
             wider captured spread.",
            avellaneda_stoikov,
        ),
        preset(
            "imbalance_taker",
            "Imbalance taker",
            "Sends an immediate-or-cancel order of 10 lots at the opposite touch when \
             top-of-book imbalance exceeds 0.6 in absolute value, closes the position 20 ticks \
             (2 s) later and waits at least 10 ticks between entries.",
            "A liquidity-taking strategy, where feed and order latency decide whether the \
             quoted price is still there on arrival and every fill pays the taker fee.",
            imbalance_taker,
        ),
        preset(
            "forensics_probe",
            "Forensics probe: five scripted orders",
            "Places five labelled orders a few ticks apart: a post-only buy one tick below the \
             bid that is cancelled 50 ticks later, a buy joining the bid, a sell joining the \
             ask, an immediate-or-cancel buy priced to reach the ask, and a post-only buy \
             priced to cross the ask.",
            "One order per typical outcome (cancelled, resting in the modeled queue, taker \
             fill on arrival, post-only expiry), so each explanation rule can be read on a \
             small tape.",
            forensics_probe,
        ),
        preset(
            "latency_lognormal",
            "Touch maker: log-normal latency",
            "The baseline strategy with seeded log-normal order latency instead of a constant \
             1 ms: entry, cancel and response latencies are each drawn independently with a \
             median of 1 ms and sigma 0.5 on the log scale, so one message in ten takes under \
             0.53 ms and one in ten over 1.90 ms (mean 1.13 ms). The draws come from the run \
             seed; messages never overtake one another. Nothing else differs.",
            "What dispersion alone does, at the same median latency as the baseline: quotes \
             and cancels that arrive late some of the time.",
            latency_lognormal,
        ),
        preset(
            "latency_heavy_tail",
            "Touch maker: heavy-tailed latency",
            "The baseline strategy with ABIDES' cubic latency model on every message: 1 ms plus \
             0.05 ms / x^3 with x uniform on (0.1, 1], so latencies range from 1.05 ms to 51 ms \
             with a median of 1.30 ms, a mean of 3.75 ms and one message in ten above 8.3 ms. \
             Seeded from the run seed; messages never overtake one another, so one slow \
             message also holds back those sent after it. Nothing else differs.",
            "Tail latency: rare slow quotes and cancels, which uniform jitter cannot produce, \
             and what they cost a passive strategy.",
            latency_heavy_tail,
        ),
        preset(
            "glft_baseline",
            "GLFT market maker",
            "Quotes with the Guéant-Lehalle-Fernandez-Tapia (2013) asymptotic formulas in the \
             trade-size form of Guéant (2017), with the 10-lot quote size as the trade size. \
             Parameters are scaled to the tick size: the flat half spread is 1.82 ticks plus a \
             volatility term, and each 10 lots of inventory skew both quotes. Volatility is \
             estimated from the size-weighted mid; the fill intensity A = 1 per second is an \
             assumption, not calibrated to the data. Execution as the baseline.",
            "A model whose quotes stay valid far from any terminal time, with an explicit trade \
             size and inventory bound, against the Avellaneda-Stoikov preset.",
            glft_baseline,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instrument(tick_size: f64, lot_size: f64) -> InstrumentMeta {
        InstrumentMeta {
            tick_size,
            lot_size,
            contract_size: 1.0,
        }
    }

    fn by_id<'a>(presets: &'a [Preset], id: &str) -> &'a Preset {
        presets
            .iter()
            .find(|p| p.id == id)
            .unwrap_or_else(|| panic!("preset {id} missing"))
    }

    #[test]
    fn catalogue_is_complete_valid_and_unique() {
        let presets = build("ds", &instrument(0.1, 0.001), None);
        let ids: Vec<&str> = presets.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "maker_baseline",
                "maker_slow",
                "maker_conservative_queue",
                "maker_rebate",
                "avellaneda_stoikov",
                "imbalance_taker",
                "forensics_probe",
                "latency_lognormal",
                "latency_heavy_tail",
                "glft_baseline",
            ]
        );
        for p in &presets {
            assert_eq!(p.config.validate(), Vec::<String>::new(), "preset {}", p.id);
            assert_eq!(p.config.dataset_id, "ds");
            assert!(!p.title.is_empty() && !p.description.is_empty());
            assert!(!p.demonstrates.is_empty());
        }
    }

    #[test]
    fn maker_variants_differ_from_the_baseline_in_one_assumption_each() {
        let presets = build("ds", &instrument(0.5, 0.01), None);
        let base = &by_id(&presets, "maker_baseline").config;

        let slow = &by_id(&presets, "maker_slow").config;
        assert_eq!(slow.strategy, base.strategy);
        assert_eq!(
            slow.execution.order_latency,
            OrderLatencyConfig::Constant {
                entry_ns: 50_000_000,
                response_ns: 50_000_000
            }
        );
        let mut slow_reset = slow.clone();
        slow_reset.execution.order_latency = base.execution.order_latency;
        assert_eq!(&slow_reset, base);

        let queue = &by_id(&presets, "maker_conservative_queue").config;
        assert_eq!(queue.execution.queue_model, QueueModelConfig::RiskAdverse);
        let mut queue_reset = queue.clone();
        queue_reset.execution.queue_model = base.execution.queue_model;
        assert_eq!(&queue_reset, base);

        let rebate = &by_id(&presets, "maker_rebate").config;
        assert_eq!(rebate.execution.fees.maker, -0.0001);
        let mut rebate_reset = rebate.clone();
        rebate_reset.execution.fees = base.execution.fees;
        assert_eq!(&rebate_reset, base);

        for id in ["latency_lognormal", "latency_heavy_tail"] {
            let variant = &by_id(&presets, id).config;
            assert_ne!(
                variant.execution.order_latency,
                base.execution.order_latency
            );
            // Same location as the baseline's constant 1 ms.
            assert_eq!(variant.execution.order_latency.base_entry_ns(), 1_000_000);
            let mut reset = variant.clone();
            reset.execution.order_latency = base.execution.order_latency;
            assert_eq!(&reset, base, "{id}");
        }
    }

    #[test]
    fn quantities_are_whole_lots_of_the_instrument() {
        let inst = instrument(0.01, 0.25);
        let presets = build("ds", &inst, None);
        let base = &by_id(&presets, "maker_baseline").config;
        assert_eq!(inst.lots(base.risk.max_position), 100);
        assert_eq!(inst.lots(base.risk.max_order_qty), 50);
        match &base.strategy.kind {
            StrategyKind::TouchMaker(p) => {
                assert_eq!(inst.lots(p.order_qty), 10);
                assert_eq!(inst.lots(p.max_position), 100);
                // Two ticks of skew at the position limit.
                assert!((p.skew_ticks_per_unit * p.max_position - 2.0).abs() < 1e-9);
            }
            other => panic!("baseline is not a touch maker: {other:?}"),
        }
    }

    #[test]
    fn avellaneda_stoikov_half_spread_is_about_two_ticks_for_any_tick_size() {
        for tick in [0.01, 0.1, 0.5, 1.0, 25.0] {
            let presets = build("ds", &instrument(tick, 0.001), None);
            let StrategyKind::AvellanedaStoikov(p) =
                &by_id(&presets, "avellaneda_stoikov").config.strategy.kind
            else {
                panic!("wrong strategy kind");
            };
            // Zero-volatility half spread, the liquidity term of the formula, in ticks.
            let half_spread_ticks = (1.0 / p.gamma) * (1.0 + p.gamma / p.kappa).ln() / tick;
            assert!(
                (half_spread_ticks - 1.823).abs() < 0.01,
                "tick {tick}: {half_spread_ticks}"
            );
        }
    }

    #[test]
    fn glft_liquidity_term_is_about_two_ticks_for_any_instrument() {
        for (tick, lot) in [(0.01, 0.001), (0.1, 0.001), (0.5, 0.01), (25.0, 1.0)] {
            let presets = build("ds", &instrument(tick, lot), None);
            let StrategyKind::Glft(p) = &by_id(&presets, "glft_baseline").config.strategy.kind
            else {
                panic!("wrong strategy kind");
            };
            assert_eq!(p.order_qty, 10.0 * lot);
            // c1 = ln(1 + gamma Delta / k) / (gamma Delta) = 10 ln(1.2) ticks.
            let x = p.gamma * p.order_qty / p.kappa;
            let c1_ticks = x.ln_1p() / (p.gamma * p.order_qty) / tick;
            assert!((c1_ticks - 1.823).abs() < 0.01, "tick {tick}: {c1_ticks}");
        }
    }

    #[test]
    fn probe_script_has_five_distinct_orders_and_uses_the_observed_spread() {
        let spread = Quantiles {
            n: 100,
            min: 1.0,
            p50: 1.0,
            p90: 2.0,
            p99: 2.5,
            max: 9.0,
            mean: 1.2,
        };
        let presets = build("ds", &instrument(0.1, 0.001), Some(&spread));
        let StrategyKind::Scripted(script) =
            &by_id(&presets, "forensics_probe").config.strategy.kind
        else {
            panic!("wrong strategy kind");
        };
        assert_eq!(script.orders.len(), 5);
        let ticks: Vec<u64> = script.orders.iter().map(|o| o.at_tick).collect();
        assert_eq!(ticks, [20, 25, 30, 35, 40]);
        assert_eq!(script.orders[0].cancel_after_ticks, Some(50));
        assert_eq!(
            script.orders[0].price,
            ScriptedPrice::FromTouch { offset: -1 }
        );
        assert_eq!(script.orders[0].tif, Tif::Gtx);
        assert_eq!(script.orders[2].side, Side::Sell);
        // p99 spread of 2.5 ticks rounds up to a 3-tick crossing offset.
        assert_eq!(
            script.orders[3].price,
            ScriptedPrice::FromTouch { offset: 3 }
        );
        assert_eq!(script.orders[3].tif, Tif::Ioc);
        assert_eq!(
            script.orders[4].price,
            ScriptedPrice::FromTouch { offset: 3 }
        );
        assert_eq!(script.orders[4].tif, Tif::Gtx);
        assert!(script.orders.iter().all(|o| !o.label.is_empty()));

        assert_eq!(crossing_offset(None), 1);
        let degenerate = Quantiles {
            p99: f64::NAN,
            ..spread
        };
        assert_eq!(crossing_offset(Some(&degenerate)), 1);
    }
}
