import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

// Self-hosted fonts (CSP font-src 'self'): Inter 400/500/600 and JetBrains Mono 400/600, latin.
import '@fontsource/inter/latin-400.css';
import '@fontsource/inter/latin-500.css';
import '@fontsource/inter/latin-600.css';
import '@fontsource/jetbrains-mono/latin-400.css';
import '@fontsource/jetbrains-mono/latin-600.css';

import './styles/tokens.css';
import './styles/base.css';
import './styles/shell.css';

import App from './App';

// A face is fetched on its first use, so text set in a weight that appears only after data arrives
// (e.g. Compare's Inter 500 legend) swapped fonts late and re-wrapped (layout shift 0.27,
// PHASE32_PERFORMANCE). Start every face now, in parallel with the API requests.
for (const face of ['400 1em Inter', '500 1em Inter', '600 1em Inter', '400 1em "JetBrains Mono"', '600 1em "JetBrains Mono"']) {
  void document.fonts?.load(face);
}

const container = document.getElementById('root');
if (!container) {
  throw new Error('index.html is missing the #root element');
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
