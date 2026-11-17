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

// A face is fetched on its first use. Inter 500 is used only by text that appears after data
// arrives (e.g. Compare's evidence legend), so its late swap re-wrapped that text (layout shift 0.27,
// PHASE32_PERFORMANCE). Start that one face now; the faces first paint uses load as usual (fetching
// all five up front delayed mobile LCP in the A/B run).
void document.fonts?.load('500 1em Inter');

const container = document.getElementById('root');
if (!container) {
  throw new Error('index.html is missing the #root element');
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
