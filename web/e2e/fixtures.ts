import { test as base, expect } from '@playwright/test';

// An automatic fixture covers every page, including popups, throughout each test.
export const test = base.extend<{ browserHealth: void }>({
  browserHealth: [async ({ context }, use) => {
    const errors: string[] = [];
    context.on('page', (page) => {
      page.on('console', (message) => {
        if (message.type() === 'error') errors.push(`console: ${message.text()}`);
      });
      page.on('pageerror', (error) => errors.push(`page: ${error.message}`));
    });
    context.on('requestfailed', (request) => {
      errors.push(`${request.method()} ${request.url()}: ${request.failure()?.errorText}`);
    });
    context.on('response', (response) => {
      if (response.status() >= 400) errors.push(`${response.status()} ${response.url()}`);
    });
    await use();
    for (const page of context.pages()) {
      if (!page.isClosed()) await page.waitForLoadState('networkidle');
    }
    expect(errors, 'No console errors, uncaught exceptions, HTTP errors, or failed browser requests').toEqual([]);
  }, { auto: true }],
});

export { expect };
