import { defineConfig, devices } from '@playwright/test'

export default defineConfig({
  testDir: './e2e',
  // Multi-step form-filling specs run six scripted model turns with full-page
  // DOM indexing between them; that measured ~25s on a fast runner
  // (2026-09-15) and GitHub runners drift slower over time (the same suite
  // went 1.1m → 2.9m between 2026-09-15 and 2026-10-01, blowing the old 45s
  // budget mid-loop). 150s keeps the budget honest against runner perf drift
  // without masking a real hang — a genuine hang still fails the run.
  timeout: 150_000,
  expect: { timeout: 10_000 },
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [['line'], ['html', { open: 'never', outputFolder: 'playwright-report' }]] : 'list',
  use: {
    ...devices['Pixel 7'],
    browserName: 'chromium',
    headless: true,
    trace: 'retain-on-failure'
  }
})
