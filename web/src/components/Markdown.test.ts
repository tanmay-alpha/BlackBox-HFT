import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';

import { LIMITATIONS_HEADING, Markdown, parseBlocks, safeHref, sectionOf } from './Markdown';

const html = (md: string) => renderToStaticMarkup(createElement(Markdown, { source: md }));

describe('Markdown', () => {
  it('finds the final limitations section in old and new report formats', () => {
    const old = '# R\n\n## Standard limitations\n\n- old one\n';
    const now = '# R\n\n## Dataset limitations\n\n- dataset\n\n## Limitations\n\n- final\n';
    expect(sectionOf(old, LIMITATIONS_HEADING)).toEqual(parseBlocks('- old one'));
    expect(sectionOf(now, LIMITATIONS_HEADING)).toEqual(parseBlocks('- final'));
  });

  it('parses the report structure', () => {
    const blocks = parseBlocks('# Title\n\nPara one\nstill one.\n\n## Table\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```json\n{"x":1}\n```\n\n- item\n- two\n');
    expect(blocks.map((b) => b.type)).toEqual(['heading', 'paragraph', 'heading', 'table', 'code', 'list']);
  });

  it('never interprets raw HTML', () => {
    const out = html('Hello <script>alert(1)</script> <img src=x onerror=alert(1)>');
    expect(out).not.toContain('<script');
    expect(out).not.toContain('<img');
    expect(out).toContain('&lt;script&gt;');
  });

  it('only links http(s) and relative targets, with rel=noreferrer', () => {
    expect(safeHref('javascript:alert(1)')).toBeNull();
    expect(safeHref('data:text/html,x')).toBeNull();
    expect(safeHref('//evil.example')).toBeNull();
    expect(safeHref('https://example.org/a')).toBe('https://example.org/a');
    expect(safeHref('#section')).toBe('#section');
    const out = html('[ok](https://example.org) and [bad](javascript:alert(1))');
    expect(out).toContain('rel="noreferrer"');
    expect(out).not.toContain('javascript:');
  });

  it('extracts a section by heading', () => {
    const s = sectionOf('# R\n\n## Reproduce\n\nx\n\n## Standard limitations\n\n- one\n- two\n', /limitation/i);
    expect(s?.[0]).toMatchObject({ type: 'list', items: ['one', 'two'] });
  });
});
