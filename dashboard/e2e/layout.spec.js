const path = require("path");
const { test, expect } = require("@playwright/test");

const repoRoot = path.resolve(__dirname, "..", "..");
const taskDirs = [
  path.join(repoRoot, "batch", "example", "split-task1"),
  path.join(repoRoot, "batch", "example", "split-task2"),
  path.join(repoRoot, "batch", "example", "split-task3"),
  path.join(repoRoot, "batch", "example", "split-task4")
];

async function addTask(page, taskDir, expectedCount) {
  await page.getByPlaceholder("/path/to/task-dir").fill(taskDir);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page.locator(".task-card")).toHaveCount(expectedCount);
}

async function removeOneTask(page, expectedCount) {
  await page.locator(".remove-btn").first().click();
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
  test("uses 1/2/3/4 layout classes and row packing on desktop", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("#tasks")).toHaveClass(/layout-1/);

    await addTask(page, taskDirs[0], 1);
    await expect(page.locator("#tasks")).toHaveClass(/layout-1/);
    await expect.poll(() => rowCounts(page)).toEqual([1]);

    await addTask(page, taskDirs[1], 2);
    await expect(page.locator("#tasks")).toHaveClass(/layout-2/);
    await expect.poll(() => rowCounts(page)).toEqual([2]);

    await addTask(page, taskDirs[2], 3);
    await expect(page.locator("#tasks")).toHaveClass(/layout-3/);
    await expect.poll(() => rowCounts(page)).toEqual([3]);

    await addTask(page, taskDirs[3], 4);
    await expect(page.locator("#tasks")).toHaveClass(/layout-4/);
    await expect.poll(() => rowCounts(page)).toEqual([2, 2]);

    await removeOneTask(page, 3);
    await expect(page.locator("#tasks")).toHaveClass(/layout-3/);
    await expect.poll(() => rowCounts(page)).toEqual([3]);

    await removeOneTask(page, 2);
    await expect(page.locator("#tasks")).toHaveClass(/layout-2/);
    await expect.poll(() => rowCounts(page)).toEqual([2]);

    await removeOneTask(page, 1);
    await expect(page.locator("#tasks")).toHaveClass(/layout-1/);
    await expect.poll(() => rowCounts(page)).toEqual([1]);
  });

  test("keeps one-column stack on mobile viewport", async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto("/");

    await addTask(page, taskDirs[0], 1);
    await addTask(page, taskDirs[1], 2);
    await addTask(page, taskDirs[2], 3);
    await addTask(page, taskDirs[3], 4);

    await expect.poll(() => rowCounts(page)).toEqual([1, 1, 1, 1]);
  });
});
