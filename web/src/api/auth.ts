// The research API key (UI_DESIGN_SPEC.md §10.2, API_CONTRACT.md §3). It lives in
// `sessionStorage['bb.apiKey']` of this tab only: never `localStorage`, the URL, logs, error
// reports or exported files. It is cleared on sign-out and on any 401.

const STORAGE_KEY = 'bb.apiKey';

/** `bbk_` + 43 base64url characters (32 random bytes). */
export const API_KEY_PATTERN = /^bbk_[A-Za-z0-9_-]{43}$/;

type Listener = (key: string | null) => void;
const listeners = new Set<Listener>();

function storage(): Storage | null {
  try {
    return typeof sessionStorage === 'undefined' ? null : sessionStorage;
  } catch {
    return null;
  }
}

export function getApiKey(): string | null {
  return storage()?.getItem(STORAGE_KEY) ?? null;
}

export function setApiKey(key: string): void {
  if (!API_KEY_PATTERN.test(key)) throw new Error('An API key is bbk_ followed by 43 characters (A-Z, a-z, 0-9, _ or -).');
  storage()?.setItem(STORAGE_KEY, key);
  for (const l of listeners) l(key);
}

export function clearApiKey(): void {
  storage()?.removeItem(STORAGE_KEY);
  for (const l of listeners) l(null);
}

export function subscribeApiKey(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
