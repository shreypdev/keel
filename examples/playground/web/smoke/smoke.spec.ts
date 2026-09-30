import { type Page, expect, test } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";

// The playground's web app, production build, real Chromium: one pass through the four views.
// Screenshots go to examples/playground/.proof/web-<view>.png; what the page logged goes to the
// test output (`npm run smoke | tee ../.proof/web-smoke.log`).

const PROOF = fileURLToPath(new URL("../../.proof/", import.meta.url));

/** The median of a list of numbers. */
const median = (values: number[]): number => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)] ?? Number.NaN;

/** The milliseconds in a readout such as `update: 0.42 ms`. */
async function readoutMs(page: Page): Promise<number> {
  const text = (await page.getByTestId("biglist-timing").textContent()) ?? "";
  const match = /([\d.]+) ms/.exec(text);
  if (match === null) throw new Error(`no timing in "${text}"`);
  return Number(match[1]);
}

test("the playground runs: todos, counter, 10k list, remote", async ({ page }) => {
  mkdirSync(PROOF, { recursive: true });
  const logged: string[] = [];
  const problems: string[] = [];
  page.on("console", (message) => {
    logged.push(`[console.${message.type()}] ${message.text()}`);
    if (message.type() === "error") problems.push(message.text());
  });
  page.on("pageerror", (error) => {
    logged.push(`[pageerror] ${error.message}`);
    problems.push(error.message);
  });
  const shot = async (view: string): Promise<void> => {
    await page.screenshot({ path: `${PROOF}web-${view}.png`, fullPage: true });
  };

  await page.goto("/");
  await expect(page.getByTestId("todo-input")).toBeVisible();
  const startedAt = await page.evaluate(() => performance.now());
  console.log(`startup: the first view was interactive ${startedAt.toFixed(0)} ms after navigation started (fetch, compile and instantiate the 538 KB core, create four stores, render)`);
  await expect(page.getByRole("heading", { name: "Keel playground" })).toBeVisible();

  // ---- Todos -------------------------------------------------------------------------------
  await test.step("todos: a refused add shows the typed error", async () => {
    await page.getByTestId("todo-add").click();
    await expect(page.getByTestId("todo-error")).toHaveText("the title cannot be empty");
    await expect(page.getByTestId("todo-item")).toHaveCount(0);
  });

  await test.step("todos: add two, toggle one, remaining follows", async () => {
    await page.getByTestId("todo-input").fill("Read the spec");
    await page.getByTestId("todo-add").click();
    await page.getByTestId("todo-input").fill("Write the smoke test");
    await page.getByTestId("todo-add").click();
    await expect(page.getByTestId("todo-item")).toHaveCount(2);
    await expect(page.getByTestId("todo-error")).toHaveCount(0);
    await expect(page.getByTestId("remaining")).toHaveText("2 left");
    await page.getByTestId("todo-toggle").first().check();
    await expect(page.getByTestId("remaining")).toHaveText("1 left");
    await page.getByTestId("filter-active").click();
    await expect(page.getByTestId("todo-item")).toHaveCount(1);
    await page.getByTestId("filter-all").click();
    await expect(page.getByTestId("todo-item")).toHaveCount(2);
    await shot("todos");
  });

  // ---- Counter -----------------------------------------------------------------------------
  await test.step("counter: increment and decrement, parity from the core", async () => {
    await page.getByTestId("tab-counter").click();
    await expect(page.getByTestId("counter-value")).toHaveText("0");
    await expect(page.getByTestId("counter-parity")).toHaveText("even");
    await page.getByTestId("counter-inc").click();
    await expect(page.getByTestId("counter-value")).toHaveText("1");
    await expect(page.getByTestId("counter-parity")).toHaveText("odd");
    await page.getByTestId("counter-inc").click();
    await expect(page.getByTestId("counter-value")).toHaveText("2");
    await expect(page.getByTestId("counter-parity")).toHaveText("even");
    await page.getByTestId("counter-dec").click();
    await expect(page.getByTestId("counter-value")).toHaveText("1");
    await expect(page.getByTestId("counter-parity")).toHaveText("odd");
    await expect(page.getByTestId("counter-changes")).toHaveText("3 changes");
    await shot("counter");
  });

  // ---- 10k list ----------------------------------------------------------------------------
  await test.step("10k list: windowed, one-row patches, a stream of updates", async () => {
    await page.getByTestId("tab-biglist").click();
    await expect(page.getByTestId("biglist-count")).toHaveText("10000");
    const drawn = await page.getByTestId("biglist-row").count();
    expect(drawn, "only the visible rows are in the DOM").toBeLessThan(40);
    expect(drawn).toBeGreaterThan(8);
    await expect(page.getByTestId("biglist-row").first()).toContainText("Item 1");

    await page.getByTestId("biglist-insert-top").click();
    await expect(page.getByTestId("biglist-count")).toHaveText("10001");
    await expect(page.getByTestId("biglist-row").first()).toContainText("Inserted at the top");

    // How long one operation takes, end to end (call, core, change-set, applied to the list): the median of 25.
    const samples: number[] = [];
    for (let i = 0; i < 25; i++) {
      await page.getByTestId("biglist-update").click();
      await expect(page.getByTestId("biglist-timing")).toContainText("update:");
      samples.push(await readoutMs(page));
    }
    console.log(`10k list: update round trip, median of 25 clicks: ${median(samples).toFixed(2)} ms (samples: ${samples.join(", ")})`);

    const versionsBefore = await page.locator(".row-version.flash").count();
    // While the stream runs, watch the frames: a second of requestAnimationFrame timestamps.
    const frames = page.evaluate(
      () =>
        new Promise<number[]>((resolve) => {
          const stamps: number[] = [];
          const tick = (now: number): void => {
            stamps.push(now);
            if (now - (stamps[0] ?? now) < 1_000) requestAnimationFrame(tick);
            else resolve(stamps);
          };
          requestAnimationFrame(tick);
        }),
    );
    await page.getByTestId("biglist-stream").check();
    const stamps = await frames;
    await page.getByTestId("biglist-stream").uncheck();
    const gaps = stamps.slice(1).map((stamp, i) => stamp - (stamps[i] as number));
    console.log(`10k list: streaming ten updates a second: ${stamps.length} frames in one second, longest gap between frames ${Math.max(...gaps).toFixed(1)} ms, median ${median(gaps).toFixed(1)} ms`);
    const streamed = await page.locator(".row-version.flash").count();
    console.log(`10k list: rows with a version above 0 after the update clicks: ${versionsBefore}; after a second of streaming: ${streamed}`);
    expect(streamed, "rows changed while streaming").toBeGreaterThan(versionsBefore);
    await expect(page.getByTestId("biglist-count")).toHaveText("10001");

    await page.getByTestId("biglist-remove").click();
    await expect(page.getByTestId("biglist-count")).toHaveText("10000");
    await shot("biglist");
  });

  // ---- Remote ------------------------------------------------------------------------------
  await test.step("remote: the seeded items, an optimistic add, then the server's", async () => {
    await page.getByTestId("tab-remote").click();
    await expect(page.getByTestId("remote-item")).toHaveCount(3);
    await expect(page.getByTestId("remote-status")).toHaveText("success");
    await expect(page.getByTestId("remote-fetching")).toHaveCount(0);
    await expect(page.getByTestId("remote-item").first()).toContainText("Buy milk");

    await page.getByTestId("remote-input").fill("Ship the playground");
    await page.getByTestId("remote-add").click();
    // The server answers after 300 ms; the placeholder is there before it does.
    await expect(page.getByTestId("remote-item")).toHaveCount(4, { timeout: 250 });
    await expect(page.getByTestId("remote-pending")).toHaveCount(1, { timeout: 250 });
    await expect(page.getByTestId("remote-pending")).toHaveCount(0);
    await expect(page.getByTestId("remote-item")).toHaveCount(4);
    await expect(page.getByTestId("remote-item").last()).toContainText("Ship the playground");

    await page.getByTestId("remote-toggle").first().check();
    await expect(page.getByTestId("remote-toggle").first()).toBeChecked();
    await expect(page.getByTestId("remote-saving")).toHaveCount(0); // the PATCH was answered

    await page.getByTestId("remote-refresh").click();
    await expect(page.getByTestId("remote-fetching")).toBeVisible();
    await expect(page.getByTestId("remote-fetching")).toHaveCount(0);
    await expect(page.getByTestId("remote-status")).toHaveText("success");
    await expect(page.getByTestId("remote-toggle").first()).toBeChecked();
    await expect(page.getByTestId("remote-item")).toHaveCount(4);
  });

  await test.step("remote: offline, the add waits; online, it is replayed", async () => {
    await page.getByTestId("remote-offline").check();
    await page.getByTestId("remote-input").fill("Written offline");
    await page.getByTestId("remote-add").click();
    await expect(page.getByTestId("remote-item")).toHaveCount(5);
    await expect(page.getByTestId("remote-pending")).toHaveText("waiting for the network");
    // It stays queued: give the server (which is down) time to be asked again, and see that the item is still only a placeholder.
    await page.waitForTimeout(1_000);
    await expect(page.getByTestId("remote-item")).toHaveCount(5);
    await expect(page.getByTestId("remote-pending")).toHaveCount(1);
    await shot("remote-offline");

    await page.getByTestId("remote-offline").uncheck();
    await expect(page.getByTestId("remote-pending")).toHaveCount(0);
    await expect(page.getByTestId("remote-item")).toHaveCount(5);
    await expect(page.getByTestId("remote-item").last()).toContainText("Written offline");
    await expect(page.getByTestId("remote-status")).toHaveText("success");
    await shot("remote");
  });

  // A dark-mode look at the same page, for the record.
  await page.emulateMedia({ colorScheme: "dark" });
  await page.getByTestId("tab-todos").click();
  await expect(page.getByTestId("todo-item")).toHaveCount(2);
  await shot("todos-dark");

  for (const line of logged) console.log(line);
  expect(problems, "the page logged no errors").toEqual([]);
});

test("a core that cannot be loaded is reported on the page", async ({ page }) => {
  // The page asks for the wasm core; make that fail, as a missing file or a schema mismatch would.
  await page.route("**/*.wasm", (route) => route.abort());
  await page.goto("/");
  const alert = page.getByRole("alert");
  await expect(alert).toContainText("Keel did not start");
  await expect(alert).toContainText("wasm core");
  await expect(page.getByTestId("tab-todos")).toHaveCount(0);
});
