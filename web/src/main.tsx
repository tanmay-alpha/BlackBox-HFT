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

const container = document.getElementById('root');
if (!container) {
  throw new Error('index.html is missing the #root element');
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
