const path = require("path");
const { test, expect } = require("@playwright/test");

const repoRoot = path.resolve(__dirname, "..", "..");
const taskDirs = [
  path.join(repoRoot, "worker", "example", "split-task1"),
  path.join(repoRoot, "worker", "example", "split-task2"),
  path.join(repoRoot, "worker", "example", "split-task3"),
  path.join(repoRoot, "worker", "example", "split-task4")
];

async function addTask(page, request, taskDir, expectedCount) {
  const addRes = await request.post("/api/tasks", {
    data: { dir: taskDir }
  });
  expect(addRes.ok()).toBeTruthy();
  await page.reload();
  await expect(page.locator(".task-card")).toHaveCount(expectedCount);
}

async function removeOneTask(page, request, expectedCount) {
  const statusRes = await request.get("/api/status");
  expect(statusRes.ok()).toBeTruthy();
  const status = await statusRes.json();
  const tasks = Array.isArray(status.tasks) ? status.tasks : [];
  expect(tasks.length).toBeGreaterThan(0);

  const removeRes = await request.delete(`/api/tasks/${tasks[0].id}`);
  expect(removeRes.ok()).toBeTruthy();
  await page.reload();
  await expect(page.locator(".task-card")).toHaveCount(expectedCount);
}

async function rowCounts(page) {
  const boxes = await page.locator(".task-card").evaluateAll((nodes) =>
    nodes.map((n) => {
      const rect = n.getBoundingClientRect();
      return { x: rect.x, y: rect.y };
    })
  );
  boxes.sort((a, b) => a.y - b.y || a.x - b.x);

  const rows = [];
  for (const box of boxes) {
    const last = rows[rows.length - 1];
    if (!last || Math.abs(last.y - box.y) > 6) {
      rows.push({ y: box.y, count: 1 });
    } else {
      last.count += 1;
    }
  }
  return rows.map((r) => r.count);
}

test.describe("task grid layout", () => {
  test.beforeEach(async ({ page, request }) => {
    await page.addInitScript(() => {
      class DisabledEventSource {
        close() {}
      }
      window.EventSource = DisabledEventSource;
    });

    const statusRes = await request.get("/api/status");
    expect(statusRes.ok()).toBeTruthy();
    const status = await statusRes.json();
    const tasks = Array.isArray(status.tasks) ? status.tasks : [];

    for (const task of tasks) {
      const removeRes = await request.delete(`/api/tasks/${task.id}`);
      expect(removeRes.ok()).toBeTruthy();
    }

    await page.goto("/");
  });

  test("uses 1/2/3/4 layout classes and row packing on desktop", async ({ page, request }) => {
    await expect(page.locator("#tasks")).toHaveClass(/layout-1/);

    await addTask(page, request, taskDirs[0], 1);
    await expect(page.locator("#tasks")).toHaveClass(/layout-1/);
    await expect.poll(() => rowCounts(page)).toEqual([1]);

    await addTask(page, request, taskDirs[1], 2);
    await expect(page.locator("#tasks")).toHaveClass(/layout-2/);
    await expect.poll(() => rowCounts(page)).toEqual([2]);

    await addTask(page, request, taskDirs[2], 3);
    await expect(page.locator("#tasks")).toHaveClass(/layout-3/);
    await expect.poll(() => rowCounts(page)).toEqual([3]);

    await addTask(page, request, taskDirs[3], 4);
    await expect(page.locator("#tasks")).toHaveClass(/layout-4/);
    await expect.poll(() => rowCounts(page)).toEqual([2, 2]);

    await removeOneTask(page, request, 3);
    await expect(page.locator("#tasks")).toHaveClass(/layout-3/);
    await expect.poll(() => rowCounts(page)).toEqual([3]);

    await removeOneTask(page, request, 2);
    await expect(page.locator("#tasks")).toHaveClass(/layout-2/);
    await expect.poll(() => rowCounts(page)).toEqual([2]);

    await removeOneTask(page, request, 1);
    await expect(page.locator("#tasks")).toHaveClass(/layout-1/);
    await expect.poll(() => rowCounts(page)).toEqual([1]);
  });

  test("keeps one-column stack on mobile viewport", async ({ page, request }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.reload();

    await addTask(page, request, taskDirs[0], 1);
    await addTask(page, request, taskDirs[1], 2);
    await addTask(page, request, taskDirs[2], 3);
    await addTask(page, request, taskDirs[3], 4);

    await expect.poll(() => rowCounts(page)).toEqual([1, 1, 1, 1]);
  });
});
