# Latency sensitivity of the demo touch maker (research note)

**Everything below is simulated on synthetic data.** Replayed prices never react to the simulated
orders (no market impact), the queue position is modeled, not observed
([QUEUE_MODELS.md](../QUEUE_MODELS.md)), and order latency is an assumption, not a calibration
([LATENCY_MODEL.md](../LATENCY_MODEL.md)). Nothing here says how a real strategy would perform.

## Question

The public demo compares two runs of one configuration that differ only in order latency
(1 ms vs 50 ms each way; runs `253bbc23b342f373` and `d6a57aed972e18d0`). This note extends that
two-point comparison into a curve over 8 latencies × 3 queue models, and repeats the full grid on
3 further synthetic datasets to show how much the numbers move.

## Setup

| Item | Value |
|---|---|
| Binary | `blackbox` 0.1.0 built from `git archive a2f754e639be6d6d5c7b33f434326b5284e15ae4` (branch `rebuild/explore` HEAD at the time), `cargo build --release --locked -p bb-app --bin blackbox`, rustc 1.99.1, target `x86_64-pc-windows-gnu`; exe sha256 `6f17577bfdeb9e552c0fccb21295c07956091341c49e90c41d7349bee70122aa` |
| Platform tag (run manifests) | `windows-x86_64-gnu+libm-e9b26b47`, Windows 10.0.26300 x64 |
| Base config | the demo run's config verbatim (`GET /api/v1/runs/253bbc23b342f373` on the local stack): `touch_maker` (order 0.01, max position 0.1, offset 0, skew 20/unit), 100 ms interval, `no_partial_fill`, constant order latency, maker 2 bp / taker 4 bp, seed 1 |
| Swept | `order_latency_ns` (entry = response) ∈ {0.5, 1, 2, 5, 10, 25, 50, 100} ms × `queue_model` ∈ {`power_prob3 n=3` (demo), `risk_adverse`, `log_prob`} = 24 cells per dataset |
| Datasets | `synthetic-demo` (generator seed 7, events sha256 `0bc26426…8f38`, identical to the hosted demo's), and `synth-s11`/`s12`/`s13` (seeds 11/12/13, sha256 `373883bf…2e87b`, `7b9edbf2…1f77`, `21b81c2d…f1ff06`); each 120 s, tick 0.1 |
| Compute | 96 runs, about 0.5 s of wall time per 24-cell grid with 6 threads (whole script ≈ 3.3 s) |

Reproducibility check: the 1 ms and 50 ms `power_prob3` cells on `synthetic-demo` get the demo's run
ids **and** result hashes (`57f055c52f8ad6b0…`, `dffbcf7dece22181…`), although the stored demo runs
were produced by a debug build with platform tag `…libm-0352e099`.

### Commands

Scratch directory `D:/bbwt-research/latency-sens/` (data dir `./data` there). The four
`specs/<dataset>.json` files are `GridSpec`s whose `base` is the demo config with `dataset_id`
replaced, `x = {"type":"order_latency_ns","values":[500000,1000000,2000000,5000000,10000000,25000000,50000000,100000000]}`
and `y = {"type":"queue_model","values":[{"type":"power_prob3","n":3.0},{"type":"risk_adverse"},{"type":"log_prob"}]}`.

```sh
BB=./target/release/blackbox.exe; DD=./data
$BB --data-dir $DD synth                                  # synthetic-demo, seed 7
$BB --data-dir $DD synth --id synth-s11 --seed 11
$BB --data-dir $DD synth --id synth-s12 --seed 12
$BB --data-dir $DD synth --id synth-s13 --seed 13
for ds in synthetic-demo synth-s11 synth-s12 synth-s13; do
  $BB --data-dir $DD grid --spec specs/$ds.json --threads 6 > out/grid-$ds.txt
done
```

Per-cell metrics were read from `data/runs/<run_id>/summary.json` (`metrics`, `manifest.result_sha256`)
and collected in `out/results.csv` (sha256 `16e80124…b00650`).

### Time windows: not supported by the CLI

`RunConfig` has no window field and no CLI command slices a dataset in time. Windowed runs exist
only in the server-side sweep registry (`SweepSpec` with `TimeWindow`, `crates/bb-core/src/sweep.rs`,
run through `bb_data::slice`), which needs the Postgres-backed API and was not used. **The robustness
section therefore uses three independent synthetic realizations (generator seeds 11, 12, 13) instead
of disjoint windows of one dataset.** That tests sensitivity to the market path drawn by the same
generator, not to regime changes within one recording.

## Definitions

From [PNL_ACCOUNTING.md](../PNL_ACCOUNTING.md#run-metrics): realized and unrealized P&L are before
fees; **gross = realized + unrealized**; **net = gross − fees** (checked: |gross − fees − net| = 0 in
every one of the 96 runs). Fill ratio = orders with a fill ÷ orders submitted. Markouts are the
quantity-weighted mean of `side × (mid(t_fill + h) − price)` in ticks; negative = adverse selection.
Capture = spread capture in ticks at the fill instant. All fills in all runs are maker fills (0 taker
fills). Money is in the synthetic instrument's quote units.

## Results on `synthetic-demo` (the demo dataset)

| Queue model | Latency (ms, each way) | Fills | Fill ratio | Fees | Realized | Unrealized | Gross (R+U) | Net | Markout 0.1 s / 1 s / 5 s / 30 s (ticks) | Capture (ticks) | Run id | result_sha256 (first 16) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---|---|
| power_prob3(n=3) | 0.5 | 1253 | 0.605 | 75.181 | -0.245 | -0.000 | -0.245 | -75.427 | -0.128 / -0.126 / -0.219 / -0.295 | -0.069 | `e5d56ecc601c5592` | `f85cbfa3cc277ca0` |
| power_prob3(n=3) | 1 | 1241 | 0.602 | 74.461 | -0.248 | -0.000 | -0.248 | -74.710 | -0.129 / -0.137 / -0.198 / -0.334 | -0.076 | `253bbc23b342f373` | `57f055c52f8ad6b0` |
| power_prob3(n=3) | 2 | 1225 | 0.598 | 73.501 | -0.278 | -0.000 | -0.278 | -73.780 | -0.116 / -0.154 / -0.252 / -0.337 | -0.076 | `8a6ac383090091c2` | `e76fa4b10a19bbdb` |
| power_prob3(n=3) | 5 | 1178 | 0.580 | 70.681 | -0.242 | -0.000 | -0.242 | -70.923 | -0.132 / -0.163 / -0.197 / -0.394 | -0.080 | `a81c8feb3a08a43a` | `add8c4ea7c0c64ab` |
| power_prob3(n=3) | 10 | 1129 | 0.566 | 67.741 | -0.239 | -0.000 | -0.239 | -67.980 | -0.134 / -0.138 / -0.146 / -0.334 | -0.074 | `59dd06d0dd65fa8e` | `99ecaea29e9c5281` |
| power_prob3(n=3) | 25 | 971 | 0.504 | 58.261 | -0.181 | -0.001 | -0.181 | -58.442 | -0.129 / -0.175 / -0.169 / -0.390 | -0.068 | `306aa7ac51d133dc` | `d3c01685371a97e6` |
| power_prob3(n=3) | 50 | 745 | 0.424 | 44.701 | -0.137 | 0.001 | -0.135 | -44.836 | -0.099 / -0.085 / -0.456 / -0.415 | -0.049 | `d6a57aed972e18d0` | `dffbcf7dece22181` |
| power_prob3(n=3) | 100 | 621 | 0.400 | 37.261 | 0.044 | -0.002 | 0.042 | -37.218 | -0.002 / 0.060 / -0.167 / -0.219 | -0.007 | `bdc868ab1269b163` | `e91fdb1de8075176` |
| risk_adverse | 0.5 | 1251 | 0.606 | 75.061 | -0.240 | -0.000 | -0.240 | -75.302 | -0.129 / -0.117 / -0.225 / -0.298 | -0.070 | `c761037fa4a71c41` | `88617c331856c87e` |
| risk_adverse | 1 | 1239 | 0.603 | 74.341 | -0.243 | -0.000 | -0.243 | -74.585 | -0.130 / -0.127 / -0.203 / -0.337 | -0.077 | `bc2d1df47e2b91fe` | `2e440549a5c78bfd` |
| risk_adverse | 2 | 1221 | 0.597 | 73.261 | -0.274 | -0.000 | -0.274 | -73.536 | -0.118 / -0.144 / -0.260 / -0.343 | -0.078 | `4ba465c99758edfa` | `69b644d1a5738460` |
| risk_adverse | 5 | 1176 | 0.580 | 70.561 | -0.243 | -0.000 | -0.243 | -70.804 | -0.136 / -0.155 / -0.214 / -0.400 | -0.083 | `42cc451bdf8e9e20` | `a1cedbcb87a54969` |
| risk_adverse | 10 | 1127 | 0.565 | 67.621 | -0.249 | -0.000 | -0.249 | -67.870 | -0.136 / -0.143 / -0.160 / -0.346 | -0.077 | `354f0446494269a9` | `405f6bd325bb89eb` |
| risk_adverse | 25 | 969 | 0.503 | 58.141 | -0.186 | -0.001 | -0.186 | -58.327 | -0.136 / -0.183 / -0.187 / -0.387 | -0.072 | `b782fca8997df40b` | `6ca457d8605be6e6` |
| risk_adverse | 50 | 743 | 0.423 | 44.581 | -0.148 | 0.001 | -0.146 | -44.727 | -0.106 / -0.093 / -0.481 / -0.435 | -0.051 | `4e71d38a93d122ff` | `603f57e6f35899f5` |
| risk_adverse | 100 | 621 | 0.400 | 37.261 | 0.044 | -0.002 | 0.042 | -37.218 | 0.002 / 0.063 / -0.168 / -0.221 | -0.010 | `517301ce27cea752` | `09dc4134cceadce8` |
| log_prob | 0.5 | 1255 | 0.605 | 75.301 | -0.249 | -0.000 | -0.249 | -75.551 | -0.131 / -0.132 / -0.214 / -0.295 | -0.067 | `2a6760ccfda8f366` | `d7cc837f271618aa` |
| log_prob | 1 | 1245 | 0.603 | 74.701 | -0.245 | -0.000 | -0.245 | -74.947 | -0.130 / -0.144 / -0.189 / -0.324 | -0.072 | `130d67fc371bd27f` | `6f16fb9009791144` |
| log_prob | 2 | 1229 | 0.598 | 73.741 | -0.279 | -0.000 | -0.279 | -74.021 | -0.117 / -0.163 / -0.245 / -0.326 | -0.072 | `008daadea43439f0` | `bdf3a171dfc8e4f3` |
| log_prob | 5 | 1182 | 0.581 | 70.921 | -0.255 | -0.000 | -0.255 | -71.176 | -0.131 / -0.181 / -0.208 / -0.396 | -0.075 | `2827b08078bd6b50` | `04cbcb24e2aa462b` |
| log_prob | 10 | 1131 | 0.567 | 67.861 | -0.240 | -0.000 | -0.240 | -68.101 | -0.133 / -0.136 / -0.137 / -0.321 | -0.072 | `97ffbfe78287c680` | `b50fec0cf7645de1` |
| log_prob | 25 | 973 | 0.504 | 58.381 | -0.193 | -0.001 | -0.194 | -58.574 | -0.128 / -0.166 / -0.168 / -0.389 | -0.066 | `d34c84ff498b1370` | `916f0f5be8a84916` |
| log_prob | 50 | 745 | 0.423 | 44.701 | -0.144 | 0.001 | -0.142 | -44.843 | -0.105 / -0.094 / -0.471 / -0.431 | -0.046 | `a84c1cea47aeed07` | `2a259478cc0ab10b` |
| log_prob | 100 | 627 | 0.404 | 37.621 | 0.077 | -0.002 | 0.075 | -37.545 | 0.018 / 0.091 / -0.090 / -0.191 | 0.002 | `cef0053e961d314c` | `c1ad2ac7dddc73b4` |

## Decomposition: the curve is a fee curve

- Every fill pays the same fee: fees ÷ fills = 0.060 in every cell (2 bp × ≈ 300 notional per fill).
  So **net ≈ −0.06 × fills + gross**.
- Gross P&L is small in every cell of every dataset: between −0.461 and +0.249, never more than
  0.69% of the fees of the same run.
- Going from 0.5 ms to 100 ms (`power_prob3`), net rises by 38.209 / 43.031 / 39.166 / 42.514
  (demo / s11 / s12 / s13). Of that, the fee reduction is 37.921 / 42.715 / 38.765 / 42.065 and the
  gross change only 0.288 / 0.316 / 0.401 / 0.449.
- Fills fall monotonically with latency in all 12 (dataset × queue model) series (fill ratio
  0.605 → 0.400 on the demo, `power_prob3`). Likely mechanism, not isolated here: a slower order
  reaches the book after some of the trades that would have filled it, and quotes are replaced
  later **[INFERENCE]** ([LATENCY_MODEL.md](../LATENCY_MODEL.md#why-entry-latency-changes-outcomes)).
- Net rises monotonically with latency in all 12 series. **This is not evidence that slower is
  better.** Net is negative in every one of the 96 runs and gross is at most +0.249, so any change
  that reduces the number of fills reduces the fee bill and with it the loss. Under a maker rebate
  (not run here) the fee term flips sign and the same fill reduction would lower net instead
  **[INFERENCE]**.
- Markouts are mostly negative (adverse selection, typically −0.1 to −0.5 ticks): 353 of the 384
  (run × horizon) values. Of the 31 positive ones, 23 are at 50–100 ms latency (e.g. demo
  `risk_adverse` 100 ms: 0.002 / 0.063 at 0.1 / 1 s, still negative at 5 s and 30 s) and the other
  8 are 30 s markouts at 5–25 ms on `synth-s13`. No consistent latency trend in markouts
  is visible over these 120 s datasets (e.g. demo `power_prob3` 5 s markout: −0.219, −0.198, −0.252,
  −0.197, −0.146, −0.169, −0.456, −0.167). The 30 s markout covers fewer fills than the others because
  fills in the last 30 s have no value.

## Where the queue model changes the ordering

Queue-model effects are small next to the latency effect: within one (dataset, latency) cell the
three models differ by at most 14 fills and 0.843 net (`synth-s11`, 1 ms: `risk_adverse` 1268 fills,
−76.515; `power_prob3` 1272, −76.762; `log_prob` 1282, −77.358), versus a 38.0–43.5 net change from
0.5 ms to 100 ms in every (dataset, queue model) series.

Ranking of the three models by net (best first; RA = `risk_adverse`, PP3 = `power_prob3 n=3`,
LP = `log_prob`; "(tie)" = two equal values):

| Dataset | 0.5 | 1 | 2 | 5 | 10 | 25 | 50 | 100 ms |
|---|---|---|---|---|---|---|---|---|
| synthetic-demo | RA>PP3>LP | RA>PP3>LP | RA>PP3>LP | RA>PP3>LP | RA>PP3>LP | RA>PP3>LP | RA>PP3>LP | PP3=RA>LP |
| synth-s11 | RA>PP3>LP | RA>PP3>LP | RA>PP3>LP | RA>PP3>LP | RA>PP3>LP | RA>PP3>LP | **LP>RA>PP3** | RA>PP3>LP |
| synth-s12 | RA>PP3>LP | RA>PP3>LP | RA>PP3>LP | RA>PP3>LP | RA>PP3=LP | RA>PP3>LP | PP3=RA>LP | RA>PP3>LP |
| synth-s13 | RA>PP3>LP | RA>PP3>LP | PP3=RA>LP | RA>PP3>LP | RA>PP3>LP | **PP3>RA>LP** | RA>PP3>LP | **LP>RA>PP3** |

- Net ranking is mostly RA > PP3 > LP, which mirrors fill counts: the most conservative queue model
  fills least, and with every fill costing 0.06 against near-zero gross, fewer fills means less loss.
  It is the same fee mechanism as the latency curve, not a sign that a model is "better".
- The strict ordering RA > PP3 > LP breaks (bold) in 3 of 32 cells, all at 25–100 ms (`s11` 50 ms,
  `s13` 25 and 100 ms), where fill counts are close and gross differences dominate; 4 more cells
  have two models at exactly equal net (demo 100 ms, `s12` 10 and 50 ms, `s13` 2 ms).
- Ranked by **gross** instead of net, there is no stable ordering at all: across the 32
  (dataset, latency) cells each of the three models comes first in some cells (full listing
  reproducible from `out/results.csv`).

## Robustness across datasets (min–max over 4 synthetic datasets)

Ranges, not confidence intervals: four realizations are too few for a distributional claim.

| Queue model | Latency (ms) | Fills min–max | Fees min–max | Gross min–max | Net min–max | Fill ratio min–max | Markout 5 s min–max (ticks) |
|---|---:|---|---|---|---|---|---|
| power_prob3(n=3) | 0.5 | 1253 – 1301 | 75.181 – 78.070 | -0.461 – -0.218 | -78.288 – -75.427 | 0.605 – 0.615 | -0.252 – -0.219 |
| power_prob3(n=3) | 1 | 1241 – 1299 | 74.461 – 77.950 | -0.452 – -0.223 | -78.172 – -74.710 | 0.602 – 0.612 | -0.272 – -0.198 |
| power_prob3(n=3) | 2 | 1225 – 1291 | 73.501 – 77.470 | -0.416 – -0.208 | -77.677 – -73.780 | 0.598 – 0.612 | -0.255 – -0.191 |
| power_prob3(n=3) | 5 | 1178 – 1244 | 70.681 – 74.649 | -0.438 – -0.194 | -74.851 – -70.923 | 0.580 – 0.593 | -0.285 – -0.197 |
| power_prob3(n=3) | 10 | 1110 – 1168 | 66.608 – 70.089 | -0.415 – -0.125 | -70.214 – -66.770 | 0.557 – 0.576 | -0.312 – -0.146 |
| power_prob3(n=3) | 25 | 938 – 974 | 56.287 – 58.447 | -0.370 – -0.025 | -58.472 – -56.478 | 0.490 – 0.508 | -0.315 – -0.164 |
| power_prob3(n=3) | 50 | 728 – 747 | 43.685 – 44.814 | -0.300 – 0.062 | -45.115 – -43.837 | 0.398 – 0.424 | -0.456 – -0.159 |
| power_prob3(n=3) | 100 | 568 – 621 | 34.076 – 37.261 | -0.145 – 0.230 | -37.218 – -34.221 | 0.376 – 0.400 | -0.167 – 0.115 |
| risk_adverse | 0.5 | 1251 – 1301 | 75.061 – 78.070 | -0.454 – -0.216 | -78.286 – -75.302 | 0.606 – 0.615 | -0.252 – -0.225 |
| risk_adverse | 1 | 1239 – 1299 | 74.341 – 77.950 | -0.445 – -0.221 | -78.170 – -74.585 | 0.603 – 0.612 | -0.265 – -0.203 |
| risk_adverse | 2 | 1221 – 1291 | 73.261 – 77.470 | -0.414 – -0.208 | -77.677 – -73.536 | 0.597 – 0.612 | -0.260 – -0.193 |
| risk_adverse | 5 | 1176 – 1244 | 70.561 – 74.649 | -0.430 – -0.191 | -74.849 – -70.804 | 0.580 – 0.593 | -0.284 – -0.214 |
| risk_adverse | 10 | 1108 – 1166 | 66.488 – 69.969 | -0.414 – -0.123 | -70.092 – -66.645 | 0.556 – 0.575 | -0.303 – -0.158 |
| risk_adverse | 25 | 936 – 974 | 56.167 – 58.447 | -0.367 – -0.027 | -58.474 – -56.374 | 0.489 – 0.506 | -0.321 – -0.165 |
| risk_adverse | 50 | 726 – 747 | 43.565 – 44.814 | -0.294 – 0.062 | -45.109 – -43.718 | 0.397 – 0.423 | -0.481 – -0.171 |
| risk_adverse | 100 | 565 – 621 | 33.896 – 37.261 | -0.171 – 0.210 | -37.218 – -34.067 | 0.372 – 0.400 | -0.168 – 0.167 |
| log_prob | 0.5 | 1255 – 1305 | 75.301 – 78.310 | -0.457 – -0.202 | -78.512 – -75.551 | 0.605 – 0.619 | -0.261 – -0.214 |
| log_prob | 1 | 1245 – 1301 | 74.701 – 78.070 | -0.448 – -0.224 | -78.294 – -74.947 | 0.603 – 0.616 | -0.275 – -0.189 |
| log_prob | 2 | 1229 – 1293 | 73.741 – 77.590 | -0.423 – -0.209 | -77.799 – -74.021 | 0.598 – 0.613 | -0.261 – -0.194 |
| log_prob | 5 | 1182 – 1250 | 70.921 – 75.009 | -0.443 – -0.201 | -75.210 – -71.176 | 0.581 – 0.596 | -0.289 – -0.208 |
| log_prob | 10 | 1110 – 1172 | 66.608 – 70.329 | -0.412 – -0.127 | -70.456 – -66.770 | 0.556 – 0.577 | -0.313 – -0.137 |
| log_prob | 25 | 940 – 976 | 56.407 – 58.567 | -0.348 – -0.026 | -58.593 – -56.601 | 0.489 – 0.511 | -0.316 – -0.154 |
| log_prob | 50 | 728 – 747 | 43.685 – 44.814 | -0.293 – 0.097 | -45.108 – -43.847 | 0.398 – 0.423 | -0.471 – -0.168 |
| log_prob | 100 | 570 – 627 | 34.196 – 37.621 | -0.138 – 0.249 | -37.545 – -34.334 | 0.377 – 0.404 | -0.093 – 0.126 |

Stable across all four datasets: fills and fees fall, net rises, monotonically with latency, for
every queue model; gross stays within ±0.5. Not stable: the sign of gross (positive only at 50–100 ms,
and only on some datasets: demo at 100 ms, `s12` at 50 and 100 ms, `s13` at 100 ms, never on `s11`),
the queue-model ranking at ≥ 25 ms, and markouts at 100 ms (5 s markout ranges from −0.167 to
+0.115 for `power_prob3`).

## Conclusions (careful)

1. On this synthetic market, with 2 bp maker fees, the demo touch maker's net P&L is almost entirely
   fees (≥ 99.3% of |net| in every run). The latency curve is a fill-count curve multiplied by a
   constant fee per fill.
2. Higher assumed latency reduces fills monotonically (all 12 series). Because each fill loses
   money after fees, net improves as latency rises. This is a property of a loss-making strategy,
   not a benefit of latency; no claim that slower is better is supported.
3. Gross P&L before fees is near zero and its sign depends on the dataset and on latency; nothing
   here establishes a gross edge at any latency.
4. Queue-model choice moves net by at most 0.843 per cell here, mostly via fill counts; it changes
   the strict net ranking of models in 3 of 32 (dataset, latency) cells, and the gross ranking has
   no stable order.
   Which model is right is unobservable from L2 data.
5. Limits of **this historical study**: synthetic generator only; no market impact; constant
   latency (the study did not exercise jitter or tails); one strategy configuration; 120 s per
   dataset; independent generator seeds instead of time windows; one fee schedule. The current
   simulator additionally supports `jitter`, `log_normal`, `heavy_tail`, separate cancel legs
   (in the latter two), and `think_time_ns`; these do not retroactively change these results.

## Appendix: run ids and result hashes

**synthetic-demo**

| Queue model | Latency (ms) | Run id | result_sha256 |
|---|---:|---|---|
| power_prob3(n=3) | 0.5 | `e5d56ecc601c5592` | `f85cbfa3cc277ca0b9bc7e1653eec5ef6d6ee70705d23adb67d0b7c525d5dc58` |
| power_prob3(n=3) | 1 | `253bbc23b342f373` | `57f055c52f8ad6b00ed2390650c608b5fbafec3fc06e88221ba11d4c03bf16db` |
| power_prob3(n=3) | 2 | `8a6ac383090091c2` | `e76fa4b10a19bbdbfb94fde48e3cfaf684adf13d3d2286eb57ed47d76879257b` |
| power_prob3(n=3) | 5 | `a81c8feb3a08a43a` | `add8c4ea7c0c64abe7b5c4801364c5b54439522e2e5bb654759c65d3a725cf15` |
| power_prob3(n=3) | 10 | `59dd06d0dd65fa8e` | `99ecaea29e9c5281aa230df342b72f85624fe623c2f64f76eaef5977e86a02bc` |
| power_prob3(n=3) | 25 | `306aa7ac51d133dc` | `d3c01685371a97e607ed600bae29289e1579a57a8d89960edc976df99276ecfb` |
| power_prob3(n=3) | 50 | `d6a57aed972e18d0` | `dffbcf7dece221819e4094ecf8a87511d63d418aeda056dba5f1983e4eca7e17` |
| power_prob3(n=3) | 100 | `bdc868ab1269b163` | `e91fdb1de8075176311e4513a6c64d02cd178e97d9bd22fa0801cee3fc9dfdb5` |
| risk_adverse | 0.5 | `c761037fa4a71c41` | `88617c331856c87e9b085aca12b4cd2713c5d434ea4e89aa53a68867fdfee1f3` |
| risk_adverse | 1 | `bc2d1df47e2b91fe` | `2e440549a5c78bfd25d4c2ed7705301a089ddff3bc13f7ac0d71b8555769e288` |
| risk_adverse | 2 | `4ba465c99758edfa` | `69b644d1a57384603f78b15dfa84a9c19c4fc6f4d2d50fdb34960c102f4a2f5c` |
| risk_adverse | 5 | `42cc451bdf8e9e20` | `a1cedbcb87a54969cf0a64ee4180ca2ea4bd37b1082645ec5322b6bdd2227635` |
| risk_adverse | 10 | `354f0446494269a9` | `405f6bd325bb89eb8bb2831d6d3e6cd7d1e65f44132b1cbcda189c3e7724d0d3` |
| risk_adverse | 25 | `b782fca8997df40b` | `6ca457d8605be6e6322a7197360541c5c6ba8eea6d03acb2dca10274f11e7cd0` |
| risk_adverse | 50 | `4e71d38a93d122ff` | `603f57e6f35899f5fe5f45d05ece7cb16c35fa5e30630b8d9dc60b41b64c6eb6` |
| risk_adverse | 100 | `517301ce27cea752` | `09dc4134cceadce8a2c252b0b22348a8356568180bd77ec1562862cf3cd7348e` |
| log_prob | 0.5 | `2a6760ccfda8f366` | `d7cc837f271618aaba07b6dd34fd77f349931c9860bb854515ff9de536ffa1c0` |
| log_prob | 1 | `130d67fc371bd27f` | `6f16fb900979114406ba4ccca37b8378610c104d7077cf96a0917d8d33be00fc` |
| log_prob | 2 | `008daadea43439f0` | `bdf3a171dfc8e4f30264ff9e030d46b5ecca51a98a415818d9277b03e3a70303` |
| log_prob | 5 | `2827b08078bd6b50` | `04cbcb24e2aa462b6cb4eb212a1392b418809aa02f7d165e798ffa886c13036e` |
| log_prob | 10 | `97ffbfe78287c680` | `b50fec0cf7645de1db564b77eb660dc586b281f88bc738651a2f100e951c0363` |
| log_prob | 25 | `d34c84ff498b1370` | `916f0f5be8a8491645459548d24cbc9f61b1c8c4da9c633d319d857be98ec3b5` |
| log_prob | 50 | `a84c1cea47aeed07` | `2a259478cc0ab10bf6b7bba2d16fa457097ed08a81352b76641b14370dae5150` |
| log_prob | 100 | `cef0053e961d314c` | `c1ad2ac7dddc73b4444cdc43da8502a7b93e24ee8aa459c0a545783ea4a345f1` |

**synth-s11**

| Queue model | Latency (ms) | Run id | result_sha256 |
|---|---:|---|---|
| power_prob3(n=3) | 0.5 | `e424fa74942f8fbf` | `bb9733646331c8a9d5b4ef35ad55c2c2176777eb6b60bd9ae2e77937f392525d` |
| power_prob3(n=3) | 1 | `98741e920904c8a2` | `f30ffda5d0bbea7519e41a816121515cf1ec909e4e92710062c14f00f1bb4d91` |
| power_prob3(n=3) | 2 | `f1a216963043334a` | `02174ae6c53d88e28f03215fcbfdac46a3edc423cc20797d778dc97561a42581` |
| power_prob3(n=3) | 5 | `1e1935d1d3f4c6e1` | `ab40493a5857e2bcbf49e0674c1101dc88f85879fc43b166695df51aa1dd9f94` |
| power_prob3(n=3) | 10 | `f4a1e6d65349671e` | `608335f9d93d298ae1efbfba7a973ef1db9ea1534aaaf10e81b1029c226bc106` |
| power_prob3(n=3) | 25 | `87204845f1ffa2fb` | `af43f4c87656682c9b52de800661d35da43a2f5b927d9499fc31a95ec8158d8e` |
| power_prob3(n=3) | 50 | `3d1f5c826229439c` | `e31705b24502ec62c11aa07e06976ee4d3c886b60d173e68cc0e93f274314d18` |
| power_prob3(n=3) | 100 | `6a8121413faae2a4` | `9d93c6cb5eab2be1d70bb97b0090232f751585516644fb9849ecc5e5df2c00f9` |
| risk_adverse | 0.5 | `591bf693a96218e8` | `b91232ef650d7a6fd37c84e366a75f914b48c705d974a357a22de6107ad2e84a` |
| risk_adverse | 1 | `1a84dc5ffa5c745e` | `63e9899b977bf8ab11465bce58eefdb5375791bba8b5cfe2abf815a20413d51d` |
| risk_adverse | 2 | `30f5e3708ebdb84c` | `81725a3cbffe4cec4f882ab75181c6a99087ed91eee5207c8428fe302396a59c` |
| risk_adverse | 5 | `5d14988d2e0b5d69` | `4f3535e09b3971ea20ec3afa74fa1196042f0031d6ca46e7090a9fd34bb69b6e` |
| risk_adverse | 10 | `aadd58aeced560f3` | `c3be87e1a2e52780308fda4520f5dcae14a6dc40fbc6a99daa98f6e4f71e5880` |
| risk_adverse | 25 | `1183fe4b0d71244c` | `5405c21fced41b78a841011e0f952be5aa19a2c5f16520b969faa7f6a7122771` |
| risk_adverse | 50 | `77e593f973b91e6b` | `093c73a3cd4442349fdb8bd018842761338ebe16ba16eddd8e0dfc4fd17f579b` |
| risk_adverse | 100 | `1686331823b7b6d0` | `5df2abaf17634f276dc9bc60fb7c9eb4d61bde79a89fb4638836695ecd49cfdb` |
| log_prob | 0.5 | `73a0d669a07288d4` | `8280fcf8ed52261c3a6f4dd88018b97aaf34066221c245ac12a33701ab24cef9` |
| log_prob | 1 | `679d0a073c6cff88` | `2f7f9ba5a21b21def9adae61a083688ec8ade1bcfed2bb0d8f0277d9953ac8a9` |
| log_prob | 2 | `555fbb281c4a8f28` | `efc7c824bfbda96d13b2abee58996525b4f1810678979949cd89fb323906954e` |
| log_prob | 5 | `3e9ef52ff076697f` | `4452dfa037d0ed2f7ecffd5a69e5bedd6a96ca87433d027bfb1ace76d5a750e2` |
| log_prob | 10 | `42e72fa7f279dcb4` | `4383c47b27f4c35576ec11e5d09578fcc1bf3688f02bbbdf872438f17c9977df` |
| log_prob | 25 | `0e7fc8f2cb3f51e8` | `260fb202e513026c2797431cc0c08de30a04ea264f8f6337b3de58af8ccf2ac0` |
| log_prob | 50 | `61aac9cef4397e97` | `f918783ad9f64b8d52938b4eb33398354106ace04ff6e0d798fe5e3b8d559c67` |
| log_prob | 100 | `fae3e4ca987b2e6a` | `e1463772a70fce086ec5edc16c401a79b0ee22c515efa3cf93d949dad839409f` |

**synth-s12**

| Queue model | Latency (ms) | Run id | result_sha256 |
|---|---:|---|---|
| power_prob3(n=3) | 0.5 | `12b351ff1210bfa6` | `44786adf50cb92cad765eb530995deb89a1228b8fd4d4274f350877d818608d1` |
| power_prob3(n=3) | 1 | `a089960690a58ce5` | `9952e3cad3ca013d86df6361088b315450b3f3ee28181c5031f9a3101d80e8f2` |
| power_prob3(n=3) | 2 | `5d93daac034cccae` | `78856863c7ffef5d9e046a5843e026184054e6d2d60a1f68e09de451bead10ad` |
| power_prob3(n=3) | 5 | `cd421fa936ca001e` | `15b102ed1d0e812d88f1bb31c9144a78fd1dada93dd5f18410cf0c1ffd108aa4` |
| power_prob3(n=3) | 10 | `eeb7366b2efd149c` | `1bde0f9915d68ef299e8e6eb1991e9c1e315f3a1eea4a260b023e7666d7c935f` |
| power_prob3(n=3) | 25 | `5dd180788f63fdcf` | `9fd98a2447becebe8ce3eb0cfb81d81aff69920ebe07500c05bbf02c6ebe29c4` |
| power_prob3(n=3) | 50 | `bb3a4ea8c5876e9a` | `9e2c99497fca2538e21892323b66eff481393f58a872cce1d6f5cfd37d329155` |
| power_prob3(n=3) | 100 | `67c0d666602f7e01` | `f83b1131a08a7c9a8b2c1d980a21db37e5b48f5b608a9c43af2a8118af67d3c3` |
| risk_adverse | 0.5 | `f65ed22228dbe8ac` | `47d3d5b53ae0a5232934a9d52ed41c953b481c7235903fc41e50eac5ecfe15b8` |
| risk_adverse | 1 | `12c5a0eba5bbd0bc` | `932ff7a15e080395d945f3a593dca4d25a68aa165a99a931e2bb8d113fde6d8f` |
| risk_adverse | 2 | `01ff2cf4ced87125` | `ed4251e2d90b032b831666cd313da89340c1078e67bd2b921b0cfdf3a82d4f0e` |
| risk_adverse | 5 | `2d55dec5acd9bd8d` | `85c56fd8a9c69cb2953ca289d663b81cd2933ddb49083fd27d399fbf410751b0` |
| risk_adverse | 10 | `df8acb9aceb3ad0d` | `b0a9da0c035c6dbbfb61c223ff176fd6c6699f92fb9f6f45841c1a5301872b0e` |
| risk_adverse | 25 | `65b2d7bf08615d6f` | `61abe23c7518c44ead049b210c8a08566d8ea0508821ccd5238d26fe973b6e73` |
| risk_adverse | 50 | `bd2892010047a8bb` | `6c2a872ef3ceb5edb7b222bf307edf5add717e0d3e0eec821dc1fc30571b662a` |
| risk_adverse | 100 | `57a439a5b5ad2303` | `c1d525618983fc41f6232d90c4fe7bd318bfe41c12096be1c3719a22bc297e12` |
| log_prob | 0.5 | `912f394115c5e633` | `ae7a269fb1da0f587a6593aa5d2247c519c8c1264fba0e94bce40d51e233239b` |
| log_prob | 1 | `4e91e1c0b2bd89a1` | `e456cc3fb2524cd51fe3ccbe90fd604ca4c9e6023924b81386783de04160f965` |
| log_prob | 2 | `f10d97cb60f7ba34` | `443c0c564d529c1319615b64f0cc42c822f07dcbccf3c8ac74132def3f6a3238` |
| log_prob | 5 | `257f3c033187adcf` | `c40704c5c81ec3893a5566576371eec6a2b99779bd23279a631f328309d0ff41` |
| log_prob | 10 | `f7890d2f8a305ec4` | `38c14f4ce045f773f1fef2796d4cab5fff7bbacf7bebe52e240c1687435d7830` |
| log_prob | 25 | `56b31fa33e0b4531` | `19fae19323354896d0c6c29d081574e50b99da28dfbe16c08b0927c77419fa2c` |
| log_prob | 50 | `6612f5911c612156` | `a3c170ff89cab55e512b06d4b5e407d60c97895b4c76d3d7adcc813cfcc85773` |
| log_prob | 100 | `ab7b49d6b029729c` | `32926d95a7fc4474acaa016d43f859c9140252f9fdc71a87e5b92e23acc600e8` |

**synth-s13**

| Queue model | Latency (ms) | Run id | result_sha256 |
|---|---:|---|---|
| power_prob3(n=3) | 0.5 | `c4560749a72fdc07` | `2d494cb29a8830305264f128eee2b39fa5724dcac680412eef10b704c2a9bcc5` |
| power_prob3(n=3) | 1 | `e4d6f4db4f882e49` | `6c020eea6b705666673e120ff11b0ded31317aa17688892ba1f382159b6f8bad` |
| power_prob3(n=3) | 2 | `498775c4567308cc` | `de081cf92e210baee763827aa0979041a76e88e0d065efd574a8d67184effad6` |
| power_prob3(n=3) | 5 | `125153c87e5dcca4` | `ca505a0ad9c6d24114dbf32ef7bdd3889b63211d1602c90123e16deccd0faac4` |
| power_prob3(n=3) | 10 | `4c9808f38f8fb300` | `4106bcfbac10b086fde57ad2525e2f7dc6b3494147f9b0135aa221503e1e5bde` |
| power_prob3(n=3) | 25 | `dd1f073505fdcc28` | `793608705a7288f4cc5ca9bd6b45865a58ad4b2e244a254bfab1c14c66559620` |
| power_prob3(n=3) | 50 | `c93e371f86aed96e` | `76a4895bfb0bd6635fed5b5da32283140bbaee0a3a62b85d1f73863ac686f41d` |
| power_prob3(n=3) | 100 | `2c6fa06136ee7db0` | `9371f3c0bfe786fa62443ee918eb66969d9c20f6bc9df48ee5ec69cdd2c05fdc` |
| risk_adverse | 0.5 | `bb053c6b3ed06f2e` | `c71d52704b8c6e4f433e723e8f10c2ea171d4c0b135c86723aa28e2c2b775763` |
| risk_adverse | 1 | `24d60e0158ecdae1` | `081c3b86deb71bc5e22e628cfc1153eb1ecc51fdff5fffa49779c6a74d9c27be` |
| risk_adverse | 2 | `a1ac346cf4910f35` | `d3967c2c9a95914f38afb2c540af2b1eb9b83b68846090488dc9f1fb86ae6bb2` |
| risk_adverse | 5 | `401f7970485455f7` | `6148dafe7c0fd42efac61bc05f267026926cd83ca8bb28ccc9ceaac67587d353` |
| risk_adverse | 10 | `dc0f77a196ab35c6` | `d328ab9dca787a948dad7969c19d62f611f25c4283364930c163c7b97b08b6d9` |
| risk_adverse | 25 | `71f579e91625b9a2` | `8f574c10607ad902847eb1f3cdf9dbb1443e37d8e221616a5e41c8ac7792dc78` |
| risk_adverse | 50 | `6989c2e2f7f92c47` | `88bee6f1213d7243bceff2b19795a28d89b92a9c4a97d2584321900b32787083` |
| risk_adverse | 100 | `8ee5a3417521898c` | `cdf24cf793dbe539ab78d58fb36ffe68226787cc8fbd7aa91c4e0b1c33950dfe` |
| log_prob | 0.5 | `a527b88b938c6509` | `0b8870bae7539033686b0964729176860015791813c9f204f141b15d7d1a9b9b` |
| log_prob | 1 | `5fb2388ac4a0cc2b` | `bda1e81169a0a3683922197e4d619f545ae9114e09e2de3890b79be0125ef981` |
| log_prob | 2 | `027b3d952b776022` | `bbeb559a10c42fbb28413d0f9493183d658f7f16ab8b4593d8e9df92c5106a7d` |
| log_prob | 5 | `be9883aba58c8d0b` | `aed108d759eb350ffa5374a512f287a2fd04569c199a3d53be1933e93029e1d8` |
| log_prob | 10 | `485d74f68a8b39ba` | `c04a753804f3db779ee6cd0ccfed247eab31d60061327a8b7e6cdf841c6e4d7d` |
| log_prob | 25 | `f0087577e632b623` | `9d6531cf2c39ad95af222e564ba4598a39c7c760fe9db93b484d0a6724b3d604` |
| log_prob | 50 | `eef94e23311ca142` | `211fa0d7dce3b3bee0bcf835051c8045d9793a6ba039a6d9a03ca1024613339a` |
| log_prob | 100 | `e546be87b0572c4b` | `62c8df198eea0c64765aa67c7f2b66846c861522f45ecd1ee56882e6905f426c` |
