// scripts/record-demo.ts
//
// Records the README demo GIF. Layout: 1440×600 canvas — one main terminal
// (Owner, 1040×600) on the left, two picture-in-picture side panes
// (Operator 400×295 top, Viewer 400×295 bottom) on the right.
// Six-beat story: establish → collaborate → handoff → viewer-denial → wrap →
// outro, target ~16–20 s total.
//
// Design rationale: /Users/liys/.claude/plans/demo-compiled-wave.md
//
// Text overlays (title chip, chat bubbles, outro wordmark) are rendered as
// transparent PNGs via Playwright instead of ffmpeg drawtext, since the
// Homebrew ffmpeg build on this machine lacks libfreetype. Scene boundaries
// are recorded as wall-clock timestamps during capture so the filter_complex
// uses real beat times, not pre-declared constants that drift if typing
// speed changes.

import { chromium, type Page, type Browser } from '@playwright/test';
import { setTimeout as sleep } from 'timers/promises';
import { readFileSync, mkdirSync, writeFileSync, existsSync, rmSync, statSync } from 'fs';
import { join } from 'path';
import { homedir } from 'os';
import { execSync } from 'child_process';
import { fileURLToPath } from 'url';
import { dirname } from 'path';

const __dirname = dirname(fileURLToPath(import.meta.url));

// ── Config ───────────────────────────────────────────────────────────────────
const BASE_URL = 'http://localhost:7700';
const TMP_DIR = join(__dirname, '../tmp');
const FRAMES_DIR = join(TMP_DIR, 'demo-frames');
const OVERLAYS_DIR = join(TMP_DIR, 'demo-overlays');
const OUTPUT_GIF = join(__dirname, '../web/public/demo.gif');

// Canvas math — each viewport is its own cell in the final GIF, no ffmpeg
// scaling. Changing these numbers requires adjusting the overlay offsets
// in the ffmpeg filter_complex below.
const MAIN_VIEWPORT = { width: 1040, height: 600 };
const SIDE_VIEWPORT = { width: 400, height: 295 };
const CANVAS = { width: 1440, height: 600 };
const SIDE_X = MAIN_VIEWPORT.width;                              // 1040
const SIDE_GAP = CANVAS.height - SIDE_VIEWPORT.height * 2;       // 10
const OP_Y = 0;
const VW_Y = SIDE_VIEWPORT.height + SIDE_GAP;                    // 305

const FPS = 10;
const FRAME_MS = Math.round(1000 / FPS);

// Font sizes for recording (larger than production so GIF text stays legible).
const MAIN_FONT_SIZE = 20;
const SIDE_FONT_SIZE = 14;

// ── Helpers ───────────────────────────────────────────────────────────────────
function getAdminToken(): string {
  const p = join(homedir(), '.telepair', 'admin_token');
  try {
    return readFileSync(p, 'utf-8').trim();
  } catch {
    throw new Error(
      `Admin token not found at ${p} — run \`./target/release/telepair\` once to generate it`,
    );
  }
}

async function apiPost<T>(path: string, body: object, token: string): Promise<T> {
  const res = await fetch(`${BASE_URL}/api${path}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`POST ${path} → ${res.status}: ${await res.text()}`);
  return res.json() as Promise<T>;
}

// Find the xterm-bearing div under `.terminal-container`. Owner/Operator
// have a single child (the Terminal component's ref div); Viewer renders a
// `.terminal-readonly-badge` div first, so `.terminal-container > div`
// matches the badge — not the xterm. The Terminal component exposes the
// xterm instance as a `__xterm` property on its ref div, so iterate and
// pick the div that actually carries it.

// Read the full xterm scrollback buffer for readiness checks.
async function readTerminal(page: Page): Promise<string> {
  return page.evaluate(() => {
    const divs = Array.from(document.querySelectorAll('.terminal-container > div')) as any[];
    const el = divs.find((d) => d.__xterm);
    const term = el?.__xterm;
    if (!term) return '';
    const buf = term.buffer.active;
    const lines: string[] = [];
    for (let i = 0; i < buf.length; i++) {
      const l = buf.getLine(i);
      if (l) lines.push(l.translateToString(true));
    }
    return lines.join('\n');
  });
}

// Apply recording CSS + xterm font-size tweaks. The side panes are tight
// (400×295) so we hide the session topbar entirely and let the terminal
// swallow the full viewport; the main pane keeps the topbar as a
// contextual "Owner — session · xxx" header. Jiggles the viewport to
// force the fit-addon to recompute rows/cols at the new font size (the
// product only refits on explicit settings changes, not on a direct
// `term.options.fontSize =` assignment).
async function applyRecordingStyles(page: Page, isSide: boolean, fontSize: number) {
  const extraCss = isSide
    ? `
      .session-topbar { display: none !important; }
      .session-body   { height: 100vh !important; }
      .banner         { display: none !important; }
    `
    : '';
  await page.addStyleTag({
    content: `
      * { cursor: none !important; }
      .sidebar-backdrop { display: none !important; }
      ${extraCss}
    `,
  });

  await page.evaluate((size) => {
    const divs = Array.from(document.querySelectorAll('.terminal-container > div')) as any[];
    const term = divs.find((d) => d.__xterm)?.__xterm;
    if (term) term.options.fontSize = size;
  }, fontSize);

  // Force a refit by temporarily resizing the viewport. ResizeObserver in
  // Terminal.tsx picks up the container change and calls fitAddon.fit()
  // after a 100 ms debounce, which recomputes rows/cols for the new font.
  const vp = page.viewportSize();
  if (vp) {
    await page.setViewportSize({ width: vp.width + 1, height: vp.height });
    await page.waitForTimeout(250);
    await page.setViewportSize(vp);
    await page.waitForTimeout(250);
  }
}

async function typeLine(page: Page, text: string, delay = 60) {
  await page.keyboard.type(text, { delay });
}

// Render a text overlay (pill / chat bubble / wordmark panel) as a
// transparent PNG by loading HTML in a hidden context and screenshotting.
async function renderOverlay(
  browser: Browser,
  opts: { width: number; height: number; html: string; outPath: string; deviceScaleFactor?: number },
): Promise<{ width: number; height: number }> {
  const dpr = opts.deviceScaleFactor ?? 1;
  const ctx = await browser.newContext({
    viewport: { width: opts.width, height: opts.height },
    deviceScaleFactor: dpr,
  });
  const p = await ctx.newPage();
  await p.setContent(
    `<!doctype html><html><head><meta charset="utf-8"><style>
       html, body { margin: 0; padding: 0; background: transparent; }
       body {
         font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif;
         -webkit-font-smoothing: antialiased;
         color: #e6edf3;
       }
       * { box-sizing: border-box; }
     </style></head><body>${opts.html}</body></html>`,
  );
  await p.evaluate(() => document.fonts?.ready);
  await p.screenshot({ path: opts.outPath, omitBackground: true });
  await ctx.close();
  return { width: opts.width * dpr, height: opts.height * dpr };
}

// Chat-bubble HTML factory. Colored dot + role label + body text. Sized to
// fit the main pane bottom strip without obscuring command output.
function bubbleHtml(role: 'operator' | 'viewer' | 'neutral', text: string): string {
  const dotColor = role === 'operator' ? '#58a6ff' : role === 'viewer' ? '#8b949e' : '#3fb950';
  return `
    <div style="
      display: inline-flex;
      align-items: center;
      gap: 10px;
      padding: 12px 18px;
      background: rgba(33, 38, 45, 0.94);
      border: 1px solid #30363d;
      border-radius: 14px;
      font-size: 18px;
      line-height: 1;
      letter-spacing: 0.1px;
      box-shadow: 0 2px 8px rgba(0, 0, 0, 0.35);
    ">
      <span style="display:inline-block;width:10px;height:10px;border-radius:50%;background:${dotColor};"></span>
      <span>${text}</span>
    </div>`;
}

// ── Main ──────────────────────────────────────────────────────────────────────
async function main() {
  const adminToken = getAdminToken();
  const healthRes = await fetch(`${BASE_URL}/api/health`);
  if (!healthRes.ok) {
    console.error('Server not reachable at', BASE_URL, '— run `cargo run` first');
    process.exit(1);
  }
  console.log('Server OK');

  if (existsSync(FRAMES_DIR)) rmSync(FRAMES_DIR, { recursive: true });
  if (existsSync(OVERLAYS_DIR)) rmSync(OVERLAYS_DIR, { recursive: true });
  mkdirSync(FRAMES_DIR, { recursive: true });
  mkdirSync(OVERLAYS_DIR, { recursive: true });

  let browser: Browser | undefined;
  let sessionId: string | undefined;
  let capturing = false;
  let capturePromise: Promise<void> | undefined;

  try {
    browser = await chromium.launch({ headless: false });

    // ── Pre-render text overlays to transparent PNGs ────────────────────────────
    const titlePath    = join(OVERLAYS_DIR, 'title.png');
    const bubbleOpPath = join(OVERLAYS_DIR, 'bubble_op.png');
    const bubbleVwPath = join(OVERLAYS_DIR, 'bubble_vw.png');
    const bubbleWrPath = join(OVERLAYS_DIR, 'bubble_wr.png');
    const outroPath    = join(OVERLAYS_DIR, 'outro.png');

    const titleSize = await renderOverlay(browser, {
      width: 520, height: 56, outPath: titlePath,
      html: `
        <div style="
          display: inline-flex; align-items: center; gap: 10px;
          padding: 12px 22px;
          background: rgba(33, 38, 45, 0.92);
          border: 1px solid #30363d;
          border-radius: 999px;
          font-size: 15px;
          letter-spacing: 0.25px;
        ">
          <span style="width:8px;height:8px;border-radius:50%;background:#3fb950;box-shadow:0 0 6px #3fb950;"></span>
          <span>telepair · a live collaborative session</span>
        </div>`,
    });

    const bubbleOpSize = await renderOverlay(browser, {
      width: 560, height: 64, outPath: bubbleOpPath,
      html: bubbleHtml('operator', 'Operator: running health check'),
    });
    const bubbleVwSize = await renderOverlay(browser, {
      width: 420, height: 64, outPath: bubbleVwPath,
      html: bubbleHtml('viewer', 'Viewer is read-only · input blocked'),
    });
    const bubbleWrSize = await renderOverlay(browser, {
      width: 420, height: 64, outPath: bubbleWrPath,
      html: bubbleHtml('neutral', 'LGTM — ready to merge'),
    });

    // Outro: the entire 1440×600 canvas, dim overlay + centered wordmark.
    // Covering the whole canvas lets us "dissolve into the logo" via a single
    // overlay with an enable-gate + a final fade=out on the full stream.
    await renderOverlay(browser, {
      width: CANVAS.width, height: CANVAS.height, outPath: outroPath,
      html: `
        <div style="
          position: fixed; inset: 0;
          background: rgba(13, 17, 23, 0.82);
          display: flex; flex-direction: column; align-items: center;
          justify-content: center; gap: 14px;
        ">
          <div style="
            font-size: 72px; font-weight: 300; letter-spacing: 2px;
            color: #e6edf3;
          ">telepair</div>
          <div style="font-size: 16px; color: #8b949e; letter-spacing: 0.4px;">
            share your terminal. keep the keys.
          </div>
        </div>`,
    });

    console.log('Overlays ready');

    // ── Browser contexts for the three participants ────────────────────────────
    const ownerCtx    = await browser.newContext({ viewport: MAIN_VIEWPORT, locale: 'en-US' });
    const operatorCtx = await browser.newContext({ viewport: SIDE_VIEWPORT, locale: 'en-US' });
    const viewerCtx   = await browser.newContext({ viewport: SIDE_VIEWPORT, locale: 'en-US' });

    // Disable xterm.js's WebGL renderer for all three contexts. The WebGL
    // path has a stuck-cell-offset bug after client-side `term.reset()`
    // (rows render ~11 rows down the viewport) that no public API clears.
    // The canvas renderer has no equivalent cache and repaints correctly.
    for (const c of [ownerCtx, operatorCtx, viewerCtx]) {
      await c.addInitScript(() => { (window as unknown as { __DISABLE_WEBGL: boolean }).__DISABLE_WEBGL = true; });
    }

    const ownerPage    = await ownerCtx.newPage();
    const operatorPage = await operatorCtx.newPage();
    const viewerPage   = await viewerCtx.newPage();

    // ── Owner: log in + create session via dashboard ────────────────────────────
    // Login page defaults to email mode; click the "Admin token" tab first.
    await ownerPage.goto(`${BASE_URL}/login`);
    await ownerPage.getByRole('tab', { name: 'Admin token' }).click();
    await ownerPage.locator('#token').fill(adminToken);
    await ownerPage.locator('button[type="submit"]').click();
    await ownerPage.waitForURL(`${BASE_URL}/`);

    await ownerPage.locator('.target-card').first().click();
    const launchDialog = ownerPage.getByRole('dialog', { name: 'Start a session' });
    await launchDialog.waitFor({ state: 'visible', timeout: 5_000 });
    await launchDialog.getByRole('button', { name: 'Launch' }).click();
    await ownerPage.waitForURL(/\/session\/.+/);

    sessionId = ownerPage.url().split('/session/')[1];
    console.log('Session:', sessionId);

    await ownerPage.locator('.xterm').waitFor({ state: 'visible', timeout: 10_000 });
    await ownerPage.locator('.status-dot[data-status="connected"]').waitFor({
      state: 'attached', timeout: 10_000,
    });

    const operatorInvite = await apiPost<{ token: string }>(
      `/sessions/${sessionId}/invites`,
      { role: 'operator', max_uses: 1 },
      adminToken,
    );
    const viewerInvite = await apiPost<{ token: string }>(
      `/sessions/${sessionId}/invites`,
      { role: 'viewer', max_uses: 1 },
      adminToken,
    );

    await operatorPage.goto(`${BASE_URL}/join/${operatorInvite.token}`);
    await operatorPage.waitForURL(/\/session\/.+/, { timeout: 10_000 });
    await operatorPage.locator('.xterm').waitFor({ state: 'visible', timeout: 10_000 });
    await operatorPage.locator('.status-dot[data-status="connected"]').waitFor({
      state: 'attached', timeout: 10_000,
    });

    await viewerPage.goto(`${BASE_URL}/join/${viewerInvite.token}`);
    await viewerPage.waitForURL(/\/session\/.+/, { timeout: 10_000 });
    await viewerPage.locator('.xterm').waitFor({ state: 'visible', timeout: 10_000 });
    await viewerPage.locator('.status-dot[data-status="connected"]').waitFor({
      state: 'attached', timeout: 10_000,
    });
    console.log('All three participants connected');

    // ── Close sidebars (idempotent — may already be closed on narrow panes) ─────
    for (const p of [ownerPage, operatorPage, viewerPage]) {
      const isOpen = await p.locator('aside.sidebar').evaluate(
        (el) => !el.classList.contains('hidden'),
      );
      if (isOpen) {
        await p.getByRole('button', { name: 'Hide Sidebar' }).click();
        await p.locator('aside.sidebar.hidden').waitFor({ state: 'attached', timeout: 5_000 });
      }
    }

    // Apply side panes first, then main last. All three clients share one
    // PTY on the server; the PTY size follows whichever client most recently
    // sent a resize frame. Running owner *last* leaves the PTY at owner's
    // (larger) dimensions, so its canvas gets properly-wrapped bytes.
    await applyRecordingStyles(operatorPage, true,  SIDE_FONT_SIZE);
    await applyRecordingStyles(viewerPage,   true,  SIDE_FONT_SIZE);
    await applyRecordingStyles(ownerPage,    false, MAIN_FONT_SIZE);

    await ownerPage.waitForTimeout(800);

    // ── Pre-recording shell prep (runs in Owner PTY, all three mirror) ──────────
    // The default target is the user's login shell with a personalized prompt
    // plus macOS's "default shell is now zsh" notice and no git repo — neither
    // renders well in the GIF. Two steps:
    //   1. Create a throwaway git repo with four plausible commits.
    //   2. `exec env ... bash --noprofile --norc` to replace the personalized
    //      shell with a clean bash that has:
    //        - PS1='$ ' so the prompt is just "$ "
    //        - PAGER=cat / GIT_PAGER=cat so `git log` etc. don't block on `less`
    //        - --noprofile --norc to skip all startup scripts (also kills the
    //          macOS "default shell is now zsh" notice from /etc/profile)
    await ownerPage.locator('.xterm').click();

    await ownerPage.keyboard.type(
      `cd /tmp && rm -rf tp-demo && mkdir tp-demo && cd tp-demo && ` +
        `git init -q && git config user.email x@x && git config user.name x && ` +
        `git commit --allow-empty -qm 'feat: recording timeline' && ` +
        `git commit --allow-empty -qm 'fix: pty race on shutdown' && ` +
        `git commit --allow-empty -qm 'feat: invite tokens' && ` +
        `git commit --allow-empty -qm 'chore: bump solid to 1.9'`,
      { delay: 2 },
    );
    await ownerPage.keyboard.press('Enter');
    await ownerPage.waitForTimeout(900);

    await ownerPage.keyboard.type(
      `exec env PS1='$ ' PAGER=cat GIT_PAGER=cat TERM=xterm-256color ` +
        `bash --noprofile --norc`,
      { delay: 2 },
    );
    await ownerPage.keyboard.press('Enter');
    await ownerPage.waitForTimeout(1_200);

    // Reset every xterm so only a fresh prompt survives. With the WebGL
    // renderer disabled (see __DISABLE_WEBGL above), the canvas renderer
    // repaints from buffer state directly, so `term.reset()` followed by
    // an Enter keystroke (which makes bash emit `\n$ ` via the PTY) lands
    // the prompt at (row 1, col 0) on every pane.
    await Promise.all(
      [ownerPage, operatorPage, viewerPage].map((p) =>
        p.evaluate(() => {
          const divs = Array.from(document.querySelectorAll('.terminal-container > div')) as any[];
          const term = divs.find((d) => d.__xterm)?.__xterm;
          term?.reset();
        }),
      ),
    );
    await ownerPage.waitForTimeout(150);
    await ownerPage.keyboard.press('Enter');
    await ownerPage.waitForTimeout(500);

    // ── Frame capture loop (10 fps) ─────────────────────────────────────────────
    let frameIdx = 0;
    const captureFrame = async (): Promise<void> => {
      const pad = String(frameIdx++).padStart(4, '0');
      const [ob, opb, vb] = await Promise.all([
        ownerPage.screenshot(),
        operatorPage.screenshot(),
        viewerPage.screenshot(),
      ]);
      writeFileSync(join(FRAMES_DIR, `owner_${pad}.png`),    ob);
      writeFileSync(join(FRAMES_DIR, `operator_${pad}.png`), opb);
      writeFileSync(join(FRAMES_DIR, `viewer_${pad}.png`),   vb);
    };

    capturing = true;
    const captureStartedAt = Date.now();
    const captureLoop = async (): Promise<void> => {
      while (capturing) {
        const t0 = Date.now();
        await captureFrame();
        const elapsed = Date.now() - t0;
        const wait = Math.max(0, FRAME_MS - elapsed);
        if (wait > 0) await sleep(wait);
      }
    };
    capturePromise = captureLoop();

    const elapsed = () => (Date.now() - captureStartedAt) / 1000;

    // Focus the main terminal once so keystrokes land there.
    await ownerPage.locator('.xterm').click();

    // Track scene boundaries in absolute seconds-from-capture-start. These
    // drive the `enable=between(t,A,B)` gates in the ffmpeg filter.
    const mark = {
      establishEnd: 0,
      collabEnd: 0,
      handoffStart: 0,
      handoffEnd: 0,
      denyStart: 0,
      denyEnd: 0,
      wrapStart: 0,
      wrapEnd: 0,
      outroStart: 0,
      outroEnd: 0,
    };

    // ── Scene 1: Establish (~1.5 s) ─────────────────────────────────────────────
    await ownerPage.waitForTimeout(1_500);
    mark.establishEnd = elapsed();

    // ── Scene 2: Collaboration — Owner types, all three mirror ──────────────────
    await typeLine(ownerPage, 'git status');
    await ownerPage.keyboard.press('Enter');
    await ownerPage.waitForTimeout(1_200);
    await typeLine(ownerPage, 'git log --oneline -5');
    await ownerPage.keyboard.press('Enter');
    await ownerPage.waitForTimeout(1_400);
    mark.collabEnd = elapsed();

    // ── Scene 3: Handoff — Operator runs a health check ─────────────────────────
    mark.handoffStart = elapsed();
    await operatorPage.locator('.xterm').click();
    await typeLine(operatorPage, 'echo "-- health check --" && echo "db: ok  api: ok  worker: ok"');
    await operatorPage.keyboard.press('Enter');
    await operatorPage.waitForTimeout(2_500);
    mark.handoffEnd = elapsed();

    // ── Scene 4: Viewer is read-only ────────────────────────────────────────────
    mark.denyStart = elapsed();
    await viewerPage.locator('.xterm').click();
    await typeLine(viewerPage, 'rm -rf /', 110);
    await viewerPage.waitForTimeout(1_600);
    mark.denyEnd = elapsed();

    // ── Scene 5: Wrap — Owner confirms ──────────────────────────────────────────
    mark.wrapStart = elapsed();
    await ownerPage.locator('.xterm').click();
    await typeLine(ownerPage, 'echo "LGTM -- ready to merge"');
    await ownerPage.keyboard.press('Enter');
    await ownerPage.waitForTimeout(1_700);
    mark.wrapEnd = elapsed();

    // ── Scene 6: Outro ──────────────────────────────────────────────────────────
    mark.outroStart = elapsed();
    await ownerPage.waitForTimeout(1_600);
    mark.outroEnd = elapsed();

    // ── Stop capture ────────────────────────────────────────────────────────────
    capturing = false;
    await capturePromise;
    capturePromise = undefined;

    if (frameIdx === 0) throw new Error('No frames captured — check screenshot permissions');
    console.log(
      `Captured ${frameIdx} frames (${(frameIdx / FPS).toFixed(1)} s wall-clock, scene marks:`,
      mark,
    );

    // ── ffmpeg assembly ─────────────────────────────────────────────────────────
    const TMP_MP4     = join(TMP_DIR, 'telepair_demo.mp4');
    const TMP_PALETTE = join(TMP_DIR, 'telepair_palette.png');

    console.log('Compositing with ffmpeg...');

    // Border highlight timings (ease in/out a hair so they don't blink in
    // exactly on the typing start).
    const opBorder = `drawbox=x=${SIDE_X}:y=${OP_Y}:w=${SIDE_VIEWPORT.width}:h=${SIDE_VIEWPORT.height}:color=0x58a6ff@0.95:t=3:enable='between(t,${mark.handoffStart.toFixed(2)},${mark.handoffEnd.toFixed(2)})'`;
    const vwBorder = `drawbox=x=${SIDE_X}:y=${VW_Y}:w=${SIDE_VIEWPORT.width}:h=${SIDE_VIEWPORT.height}:color=0xf85149@0.95:t=3:enable='between(t,${mark.denyStart.toFixed(2)},${mark.denyEnd.toFixed(2)})'`;
    const gutter = `drawbox=x=${SIDE_X}:y=${SIDE_VIEWPORT.height}:w=${SIDE_VIEWPORT.width}:h=${SIDE_GAP}:color=0x0d1117:t=fill`;

    // Overlay positions. Chat bubbles live near the bottom of the main pane
    // (y ≈ 510) so they don't sit on top of the currently-typed command.
    const bubbleY = 510;
    const bubbleX = (w: number) => Math.round((MAIN_VIEWPORT.width - w) / 2);

    // Title pill: top-center over main pane.
    const titleY = 40;
    const titleX = (w: number) => Math.round((CANVAS.width - w) / 2);

    // Small epsilon delays let the bubble appear ~0.3s after the scene
    // starts, so the reader sees the action first, then the annotation.
    const bHandoffStart = (mark.handoffStart + 0.3).toFixed(2);
    const bHandoffEnd   = mark.handoffEnd.toFixed(2);
    const bDenyStart    = (mark.denyStart + 0.4).toFixed(2);
    const bDenyEnd      = mark.denyEnd.toFixed(2);
    const bWrapStart    = (mark.wrapStart + 0.9).toFixed(2);
    const bWrapEnd      = mark.outroStart.toFixed(2);

    const fadeStart = (mark.outroEnd - 0.7).toFixed(2);

    const filterComplex = [
      `[0:v] pad=${CANVAS.width}:${CANVAS.height}:0:0:0x0d1117 [bg]`,
      `[bg][1:v] overlay=${SIDE_X}:${OP_Y} [m1]`,
      `[m1][2:v] overlay=${SIDE_X}:${VW_Y} [m2]`,
      `[m2] ${gutter},${opBorder},${vwBorder} [m3]`,
      // Title pill (scene 1)
      `[m3][3:v] overlay=${titleX(titleSize.width)}:${titleY}:enable='between(t,0.2,${mark.establishEnd.toFixed(2)})' [m4]`,
      // Operator bubble (scene 3)
      `[m4][4:v] overlay=${bubbleX(bubbleOpSize.width)}:${bubbleY}:enable='between(t,${bHandoffStart},${bHandoffEnd})' [m5]`,
      // Viewer bubble (scene 4)
      `[m5][5:v] overlay=${bubbleX(bubbleVwSize.width)}:${bubbleY}:enable='between(t,${bDenyStart},${bDenyEnd})' [m6]`,
      // Wrap bubble (scene 5)
      `[m6][6:v] overlay=${bubbleX(bubbleWrSize.width)}:${bubbleY}:enable='between(t,${bWrapStart},${bWrapEnd})' [m7]`,
      // Outro full-canvas overlay (scene 6) — includes dim + wordmark in one PNG.
      `[m7][7:v] overlay=0:0:enable='between(t,${mark.outroStart.toFixed(2)},${mark.outroEnd.toFixed(2)})' [m8]`,
      // Final dissolve.
      `[m8] fade=t=out:st=${fadeStart}:d=0.7 [out]`,
    ].join(';');

    execSync(
      [
        'ffmpeg -y',
        `-framerate ${FPS} -start_number 0 -i "${FRAMES_DIR}/owner_%04d.png"`,
        `-framerate ${FPS} -start_number 0 -i "${FRAMES_DIR}/operator_%04d.png"`,
        `-framerate ${FPS} -start_number 0 -i "${FRAMES_DIR}/viewer_%04d.png"`,
        `-i "${titlePath}"`,
        `-i "${bubbleOpPath}"`,
        `-i "${bubbleVwPath}"`,
        `-i "${bubbleWrPath}"`,
        `-i "${outroPath}"`,
        `-filter_complex "${filterComplex}"`,
        `-map "[out]" -pix_fmt yuv420p "${TMP_MP4}"`,
      ].join(' '),
      { stdio: 'inherit' },
    );

    // palettegen max_colors=128 — ~30-40% smaller GIF with negligible
    // perceptual loss on telepair's flat dark UI.
    execSync(
      `ffmpeg -y -i "${TMP_MP4}" -vf "fps=${FPS},palettegen=max_colors=128" "${TMP_PALETTE}"`,
      { stdio: 'inherit' },
    );

    execSync(
      [
        `ffmpeg -y -i "${TMP_MP4}" -i "${TMP_PALETTE}"`,
        `-filter_complex "[0:v]fps=${FPS}[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=5"`,
        `"${OUTPUT_GIF}"`,
      ].join(' '),
      { stdio: 'inherit' },
    );

    const gifSize = statSync(OUTPUT_GIF).size;
    const gifMB = (gifSize / 1_048_576).toFixed(2);
    console.log(`Demo GIF: ${OUTPUT_GIF} (${gifMB} MB)`);

    if (gifSize > 2 * 1_048_576) {
      console.warn(`Warning: GIF is ${gifMB} MB — target is <2 MB. Consider dropping FPS to 8 or max_colors to 96.`);
    }

  } finally {
    capturing = false;
    if (capturePromise) await capturePromise.catch(() => {});

    if (sessionId) {
      await fetch(`${BASE_URL}/api/sessions/${sessionId}`, {
        method: 'DELETE',
        headers: { Authorization: `Bearer ${adminToken}` },
      }).catch(() => {});
    }

    await browser?.close().catch(() => {});
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
