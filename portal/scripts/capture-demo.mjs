// Repeatable portal capture: a screenshot of every view, plus a lightweight
// slideshow screencast cycling through them. Driven by Playwright; produces the
// assets used by the GitHub Pages showcase (docs/assets/img/).
//
//   BASE_URL=http://localhost:5318 node scripts/capture-demo.mjs
//
// Requires the portal to be running and pointed at a control plane with data
// (see the project README for seeding a demo scenario). Chromium is installed
// with `pnpm exec playwright install chromium`; on systems where that build
// doesn't match (e.g. NixOS) set CHROMIUM_PATH to a system Chrome/Chromium.
// If ImageMagick (`magick`) is on PATH the stills are stitched into an
// animated GIF.

import { chromium } from "playwright";
import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const BASE_URL = process.env.BASE_URL || "http://localhost:5318";
const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = resolve(process.env.OUT_DIR || join(HERE, "../../docs/assets/img"));
const VIEWPORT = { width: 1440, height: 900 };

mkdirSync(OUT, { recursive: true });
const log = (m) => console.log(`==> ${m}`);

// Use the system/nix Chromium when its build doesn't match Playwright's
// bundled one (e.g. on NixOS): set CHROMIUM_PATH=$(which google-chrome-stable).
// CI on a standard distro leaves it unset and uses the bundled browser.
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || undefined });
const context = await browser.newContext({ viewport: VIEWPORT, deviceScaleFactor: 2, colorScheme: "dark" });
const page = await context.newPage();

async function show(path, waitForText) {
  await page.goto(`${BASE_URL}${path}`, { waitUntil: "networkidle" });
  if (waitForText) await page.getByText(waitForText, { exact: false }).first().waitFor({ timeout: 15000 });
  await page.waitForTimeout(1200); // let the live data + transitions settle
}

async function shot(name) {
  await page.screenshot({ path: join(OUT, name) });
  log(`saved ${name}`);
}

// The slideshow plays these in order; also the set of stills the page embeds.
const SLIDES = ["events-overview.png", "event-explanation.png", "agents-fleet.png", "topology.png"];

try {
  // Events overview, then open an event to its AI explanation (prefer an
  // OOMKilled, else the first row).
  await show("/events", "Fleet overview");
  await shot("events-overview.png");

  const oomRow = page.getByText(/OOMKilled/i).first();
  const row = (await oomRow.count()) ? oomRow : page.locator("table tbody tr, [role=row]").first();
  await row.click();
  await page.getByText("EXPLANATION", { exact: false }).first().waitFor({ timeout: 15000 });
  await page.waitForTimeout(800);
  await shot("event-explanation.png");
  await page.keyboard.press("Escape").catch(() => {});

  // The remaining full-page views.
  for (const view of [
    { path: "/agents", wait: "Your fleet", name: "agents-fleet.png" },
    { path: "/topology", wait: "Topology", name: "topology.png" },
  ]) {
    await show(view.path, view.wait);
    await shot(view.name);
  }
} finally {
  await page.close();
  await context.close();
  await browser.close();
}

// Stitch the stills into a slideshow GIF (small + clean, vs a heavy screen
// recording). Skipped gracefully when ImageMagick isn't installed.
try {
  execFileSync(
    "magick",
    ["-loop", "0", "-delay", "220", ...SLIDES.map((s) => join(OUT, s)),
      "-resize", "1100x", "-layers", "Optimize", join(OUT, "ravn-portal-demo.gif")],
    { stdio: "ignore" },
  );
  log("saved ravn-portal-demo.gif");
} catch (err) {
  if (err.code === "ENOENT") {
    log("ImageMagick (magick) not on PATH — skipped GIF (screenshots still captured)");
  } else {
    log(`GIF build failed — skipped (screenshots still captured): ${err.message}`);
    process.exitCode = 1;
  }
}

log(`done — assets in ${OUT}`);
