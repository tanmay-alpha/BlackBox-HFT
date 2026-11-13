//! Assembles a pinned-engine backtest from a [`RunConfig`], with every model wrapped in its
//! recording tap.
//!
//! The engine's `L2AssetBuilder` constructs the local and exchange processors internally and
//! offers no way to observe them. Everything it uses is public, though, so this module performs
//! the same assembly by hand (same constructors, same order) and inserts the wrappers. No engine
//! code is copied or modified.

use bb_core::config::{ExchangeModel, OrderLatencyConfig, QueueModelConfig, RunConfig};
use hftbacktest::{
    backtest::{
        Asset, Backtest,
        assettype::LinearAsset,
        data::{Data, DataSource, FeedLatencyAdjustment, Reader},
        models::{
            CommonFees, ConstantLatency, LogProbQueueFunc, LogProbQueueFunc2, PowerProbQueueFunc,
            PowerProbQueueFunc2, PowerProbQueueFunc3, ProbQueueModel, QueueModel,
            RiskAdverseQueueModel, TradingValueFeeModel,
        },
        order::order_bus,
        proc::{Local, LocalProcessor, NoPartialFillExchange, PartialFillExchange, Processor},
        state::State,
    },
    depth::HashMapMarketDepth,
    types::Event,
};

use crate::{
    SimError, SimInput,
    procs::{RecExch, RecLocal},
    tape::Tape,
    taps::{F64Probe, JitterLatency, LatencyKind, QueuePosProbe, TapFee, TapLatency, TapQueue},
};

pub(crate) type Depth = HashMapMarketDepth;

/// Initial capacity of the local last-trades buffer. Must be non-zero for the engine to keep
/// trades at all; the buffer grows as needed and is drained every strategy tick.
const LAST_TRADES_CAPACITY: usize = 256;

pub(crate) fn build(input: &SimInput<'_>, tape: &Tape) -> Result<Backtest<Depth>, SimError> {
    let t = || tape.clone();
    let p = input.config.execution.exchange_model == ExchangeModel::PartialFill;
    match input.config.execution.queue_model {
        QueueModelConfig::RiskAdverse => assemble(
            input,
            tape,
            TapQueue::<_, F64Probe>::new(RiskAdverseQueueModel::<Depth>::new(), t(), p),
        ),
        QueueModelConfig::LogProb => assemble(
            input,
            tape,
            TapQueue::<_, QueuePosProbe>::new(
                ProbQueueModel::<_, Depth>::new(LogProbQueueFunc::new()),
                t(),
                p,
            ),
        ),
        QueueModelConfig::LogProb2 => assemble(
            input,
            tape,
            TapQueue::<_, QueuePosProbe>::new(
                ProbQueueModel::<_, Depth>::new(LogProbQueueFunc2::new()),
                t(),
                p,
            ),
        ),
        QueueModelConfig::PowerProb { n } => assemble(
            input,
            tape,
            TapQueue::<_, QueuePosProbe>::new(
                ProbQueueModel::<_, Depth>::new(PowerProbQueueFunc::new(n)),
                t(),
                p,
            ),
        ),
        QueueModelConfig::PowerProb2 { n } => assemble(
            input,
            tape,
            TapQueue::<_, QueuePosProbe>::new(
                ProbQueueModel::<_, Depth>::new(PowerProbQueueFunc2::new(n)),
                t(),
                p,
            ),
        ),
        QueueModelConfig::PowerProb3 { n } => assemble(
            input,
            tape,
            TapQueue::<_, QueuePosProbe>::new(
                ProbQueueModel::<_, Depth>::new(PowerProbQueueFunc3::new(n)),
                t(),
                p,
            ),
        ),
    }
}

fn assemble<QM>(
    input: &SimInput<'_>,
    tape: &Tape,
    queue_model: QM,
) -> Result<Backtest<Depth>, SimError>
where
    QM: QueueModel<Depth> + 'static,
{
    let cfg: &RunConfig = input.config;
    let inst = input.instrument;
    let exec = &cfg.execution;

    // The engine copies the rows into its own aligned buffer; the caller's slice is untouched,
    // so the feed-latency adjustment below never leaks into another run.
    let data = Data::<Event>::from_data(input.events);
    let mut reader = Reader::<Event>::builder()
        .parallel_load(false)
        .data(vec![DataSource::Data(data)]);
    if exec.feed_latency_offset_ns != 0 {
        reader = reader.preprocessor(FeedLatencyAdjustment::new(exec.feed_latency_offset_ns));
    }
    let reader = reader
        .build()
        .map_err(|e| SimError::Engine(format!("building the data reader: {e}")))?;

    let latency = match exec.order_latency {
        OrderLatencyConfig::Constant {
            entry_ns,
            response_ns,
        } => LatencyKind::Constant(ConstantLatency::new(entry_ns, response_ns)),
        OrderLatencyConfig::Jitter {
            entry_ns,
            response_ns,
            jitter_ns,
        } => LatencyKind::Jitter(JitterLatency::new(
            entry_ns,
            response_ns,
            jitter_ns,
            cfg.seed,
        )),
    };
    // `order_bus` clones the model: the local end calls `entry`, the exchange end `response`.
    let (order_e2l, order_l2e) = order_bus(TapLatency::new(latency, tape.clone()));

    let fees = || TradingValueFeeModel::new(CommonFees::new(exec.fees.maker, exec.fees.taker));
    let asset_type = LinearAsset::new(inst.contract_size);
    let depth = || Depth::new(inst.tick_size, inst.lot_size);

    // Local side: the engine's own account with an untapped fee model.
    let local = RecLocal::new(
        Local::new(
            depth(),
            State::new(asset_type.clone(), fees()),
            LAST_TRADES_CAPACITY,
            order_l2e,
        ),
        tape.clone(),
    );

    // Exchange side: its fee model is tapped, which is where every execution is observed.
    let exch_state = State::new(asset_type, TapFee::new(fees(), tape.clone()));
    let exch: Box<dyn Processor> = match exec.exchange_model {
        ExchangeModel::NoPartialFill => Box::new(RecExch::new(
            NoPartialFillExchange::new(depth(), exch_state, queue_model, order_e2l),
            tape.clone(),
        )),
        ExchangeModel::PartialFill => Box::new(RecExch::new(
            PartialFillExchange::new(depth(), exch_state, queue_model, order_e2l),
            tape.clone(),
        )),
    };

    let local: Box<dyn LocalProcessor<Depth>> = Box::new(local);
    Backtest::builder()
        .add_asset(Asset {
            local,
            exch,
            reader,
        })
        .build()
        .map_err(|e| SimError::Engine(format!("building the backtest: {e}")))
}
