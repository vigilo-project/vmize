const path = require("path");
const { defineConfig, devices } = require("@playwright/test");

const repoRoot = path.resolve(__dirname, "..");
const port = process.env.DASHBOARD_E2E_PORT || "18080";

module.exports = defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  retries: process.env.CI ? 2 : 0,
  reporter: [["list"]],
  timeout: 30_000,
  expect: {
    timeout: 5_000
  },
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    trace: "on-first-retry"
  },
  webServer: {
    command: `cargo run -p vmize -- dashboard --port ${port}`,
    cwd: repoRoot,
    port: Number(port),
    reuseExistingServer: !process.env.CI,
    timeout: 120_000
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        viewport: { width: 1440, height: 900 }
      }
    }
  ]
});
