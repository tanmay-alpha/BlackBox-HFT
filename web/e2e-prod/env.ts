// Targets of the production suite. The defaults are the deployed product (Vercel UI, Render API);
// `PROD_WEB_URL` and `PROD_API_URL` point the same specs at another stack, e.g. a local
// `blackbox serve --web-dir web/dist` that serves both on one origin. Origins only, no path.

const PRODUCTION_WEB = 'https://blackbox-hft.vercel.app';

export const WEB_URL = (process.env.PROD_WEB_URL ?? PRODUCTION_WEB).replace(/\/+$/, '');
export const API_URL = (process.env.PROD_API_URL ?? 'https://blackbox-hft-api.onrender.com').replace(/\/+$/, '');

/** Evidence (screenshots, ids) is written to `docs/` only from runs against the deployed product. */
export const IS_PRODUCTION = WEB_URL === PRODUCTION_WEB;
