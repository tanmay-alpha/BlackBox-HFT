// A sanitising Markdown renderer for the server's research reports (UI_DESIGN_SPEC.md §11.9).
// It builds React elements from a small CommonMark subset (headings, paragraphs, lists, tables,
// fenced code, block quotes, rules; inline code, strong, emphasis, links). Raw HTML is never
// interpreted: it is shown as text. Links are limited to http(s), relative and fragment targets
// and open with `rel="noreferrer"`.

import { createElement, Fragment } from 'react';
import type { ReactNode } from 'react';

export type Block =
  | { type: 'heading'; level: number; text: string }
  | { type: 'paragraph'; text: string }
  | { type: 'list'; ordered: boolean; items: string[] }
  | { type: 'table'; header: string[]; rows: string[][] }
  | { type: 'code'; lang: string; text: string }
  | { type: 'quote'; text: string }
  | { type: 'rule' };

const isTableRow = (l: string) => /^\s*\|.*\|\s*$/.test(l);
const isSeparator = (l: string) => /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/.test(l);
const cells = (l: string) =>
  l
    .trim()
    .replace(/^\|/, '')
    .replace(/\|$/, '')
    .split(/(?<!\\)\|/)
    .map((c) => c.trim().replace(/\\\|/g, '|'));

/** Splits Markdown into blocks. */
export function parseBlocks(md: string): Block[] {
  const lines = md.replace(/\r\n?/g, '\n').split('\n');
  const out: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (line.trim() === '') {
      i += 1;
      continue;
    }
    const fence = /^\s*(```|~~~)\s*([\w+-]*)\s*$/.exec(line);
    if (fence) {
      const body: string[] = [];
      i += 1;
      while (i < lines.length && !lines[i].trim().startsWith(fence[1])) body.push(lines[i++]);
      i += 1;
      out.push({ type: 'code', lang: fence[2], text: body.join('\n') });
      continue;
    }
    const h = /^(#{1,6})\s+(.*?)\s*#*\s*$/.exec(line);
    if (h) {
      out.push({ type: 'heading', level: h[1].length, text: h[2] });
      i += 1;
      continue;
    }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      out.push({ type: 'rule' });
      i += 1;
      continue;
    }
    if (isTableRow(line) && i + 1 < lines.length && isSeparator(lines[i + 1])) {
      const header = cells(line);
      const rows: string[][] = [];
      i += 2;
      while (i < lines.length && isTableRow(lines[i])) rows.push(cells(lines[i++]));
      out.push({ type: 'table', header, rows });
      continue;
    }
    if (/^\s*>/.test(line)) {
      const body: string[] = [];
      while (i < lines.length && /^\s*>/.test(lines[i])) body.push(lines[i++].replace(/^\s*>\s?/, ''));
      out.push({ type: 'quote', text: body.join(' ') });
      continue;
    }
    const bullet = /^\s*([-*+]|\d+[.)])\s+/;
    if (bullet.test(line)) {
      const ordered = /^\s*\d/.test(line);
      const items: string[] = [];
      while (i < lines.length && lines[i].trim() !== '') {
        if (bullet.test(lines[i])) items.push(lines[i].replace(bullet, ''));
        else if (items.length > 0) items[items.length - 1] += ` ${lines[i].trim()}`;
        i += 1;
      }
      out.push({ type: 'list', ordered, items });
      continue;
    }
    const para: string[] = [];
    while (i < lines.length && lines[i].trim() !== '' && !/^(#{1,6}\s|\s*(```|~~~)|\s*>)/.test(lines[i]) && !(isTableRow(lines[i]) && isSeparator(lines[i + 1] ?? ''))) {
      para.push(lines[i++].trim());
    }
    out.push({ type: 'paragraph', text: para.join(' ') });
  }
  return out;
}

/** Allowed link targets; anything else (javascript:, data:, //host, …) is rendered as plain text. */
export function safeHref(href: string): string | null {
  const h = href.trim();
  if (/^https?:\/\//i.test(h)) return h;
  if (h.startsWith('//') || /^[a-z][a-z0-9+.-]*:/i.test(h)) return null;
  return h;
}

const INLINE = /(`+)([\s\S]*?)\1|\*\*([^*]+)\*\*|__([^_]+)__|\*([^*\s][^*]*)\*|_([^_\s][^_]*)_|\[([^\]]+)\]\(([^)\s]+)\)/g;

/** Inline spans as React nodes; everything else is text (so `<b>` stays the literal text). */
export function renderInline(text: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  let last = 0;
  let k = 0;
  for (const m of text.matchAll(INLINE)) {
    const at = m.index ?? 0;
    if (at > last) nodes.push(text.slice(last, at));
    if (m[1]) nodes.push(createElement('code', { key: k++ }, m[2]));
    else if (m[3] ?? m[4]) nodes.push(createElement('strong', { key: k++ }, renderInline(m[3] ?? m[4])));
    else if (m[5] ?? m[6]) nodes.push(createElement('em', { key: k++ }, renderInline(m[5] ?? m[6])));
    else if (m[7]) {
      const href = safeHref(m[8]);
      nodes.push(
        href === null
          ? createElement(Fragment, { key: k++ }, m[7])
          : createElement('a', { key: k++, href, rel: 'noreferrer', target: href.startsWith('#') ? undefined : '_blank' }, renderInline(m[7])),
      );
    }
    last = at + m[0].length;
  }
  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

function renderBlock(b: Block, key: number): ReactNode {
  switch (b.type) {
    case 'heading':
      return createElement(`h${Math.min(6, b.level + 1)}`, { key }, renderInline(b.text));
    case 'paragraph':
      return createElement('p', { key }, renderInline(b.text));
    case 'list':
      return createElement(
        b.ordered ? 'ol' : 'ul',
        { key },
        b.items.map((it, i) => createElement('li', { key: i }, renderInline(it))),
      );
    case 'table':
      return createElement(
        'div',
        { key, className: 'md-table-wrap' },
        createElement(
          'table',
          { className: 'md-table' },
          createElement('thead', null, createElement('tr', null, b.header.map((c, i) => createElement('th', { key: i, scope: 'col' }, renderInline(c))))),
          createElement(
            'tbody',
            null,
            b.rows.map((r, ri) => createElement('tr', { key: ri }, r.map((c, ci) => createElement('td', { key: ci }, renderInline(c))))),
          ),
        ),
      );
    case 'code':
      return createElement('pre', { key, className: 'md-code' }, createElement('code', null, b.text));
    case 'quote':
      return createElement('blockquote', { key }, renderInline(b.text));
    case 'rule':
      return createElement('hr', { key });
  }
}

export function Markdown({ source, className }: { source: string; className?: string }) {
  return createElement('div', { className: className ? `md ${className}` : 'md' }, parseBlocks(source).map(renderBlock));
}

/** The blocks under the first heading matching `title` (until the next heading of that level or higher). */
export function sectionOf(md: string, title: RegExp): Block[] | null {
  const blocks = parseBlocks(md);
  const start = blocks.findIndex((b) => b.type === 'heading' && title.test(b.text));
  const head = blocks[start];
  if (!head || head.type !== 'heading') return null;
  const end = blocks.findIndex((b, i) => i > start && b.type === 'heading' && b.level <= head.level);
  return blocks.slice(start + 1, end < 0 ? undefined : end);
}

export function MarkdownBlocks({ blocks }: { blocks: Block[] }) {
  return createElement('div', { className: 'md' }, blocks.map(renderBlock));
}
