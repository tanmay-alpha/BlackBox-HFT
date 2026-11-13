import type { FullConfig } from '@playwright/test';
import { rmSync } from 'node:fs';

export default function teardown(config: FullConfig) {
  rmSync(config.metadata.dataDir as string, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
}
