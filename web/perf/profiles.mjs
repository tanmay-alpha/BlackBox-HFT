// Measurement profiles of the browser performance harness (`perf/run.mjs`). Every constant here
// is copied into the output JSON and the Markdown summary, so a result names its own conditions.

/** Chrome on Android user agent: the mobile profile is Chromium with phone emulation, not Safari. */
const MOBILE_UA =
  'Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/CHROME_VERSION Mobile Safari/537.36';

const mbit = (n) => Math.round((n * 1_000_000) / 8);
const kbit = (n) => Math.round((n * 1_000) / 8);

export const PROFILES = {
  desktop: {
    name: 'desktop',
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 1,
    isMobile: false,
    hasTouch: false,
    userAgent: null,
    cpuThrottle: 1,
    network: {
      name: 'Lighthouse desktop preset (10 Mbit/s down, 10 Mbit/s up, 40 ms RTT)',
      downloadBytesPerSec: mbit(10),
      uploadBytesPerSec: mbit(10),
      latencyMs: 40,
    },
  },
  mobile: {
    name: 'mobile',
    viewport: { width: 390, height: 844 },
    deviceScaleFactor: 3,
    isMobile: true,
    hasTouch: true,
    userAgent: MOBILE_UA,
    cpuThrottle: 4,
    network: {
      name: 'Slow 4G (1.6 Mbit/s down, 750 kbit/s up, 150 ms RTT)',
      downloadBytesPerSec: mbit(1.6),
      uploadBytesPerSec: kbit(750),
      latencyMs: 150,
    },
  },
};

/** Timeouts and windows shared by all profiles. */
export const LIMITS = {
  /** Longest wait for a scenario's ready marker after navigation start. */
  readyTimeoutMs: 120_000,
  /** Required span without a long task and without an in-flight request after the ready marker. */
  quietMs: 2_000,
  /** The quiet wait gives up after this long past the ready marker (recorded as `quiet_reached: false`). */
  quietCapMs: 20_000,
  /** Longest wait for an interaction's visible result. */
  resultTimeoutMs: 60_000,
  /** Wait after an interaction's result so Event Timing and long-task entries are delivered. */
  settleMs: 700,
  /** A long task is a main-thread task over this many ms (Long Tasks API); TBT counts the excess. */
  longTaskMs: 50,
};
