// scripts/record-demo.ts
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
const FRAMES_DIR = join(__dirname, '../tmp/demo-frames');
const OUTPUT_GIF = join(__dirname, '../web/public/demo.gif');
const VIEWPORT = { width: 720, height: 480 };
const FPS = 10;
const FRAME_MS = Math.round(1000 / FPS);

// Override the production xterm fontSize (14px) to something more legible
// in an inline README GIF. 20px gives ~60 cols × ~18 rows in a 720x480
// pane — still wide enough for htop to render its CPU/mem panel and tall
// enough to show ~10 process rows, while making each glyph ~43% bigger
// than the production default.
const RECORDING_FONT_SIZE = 20;

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

// ── Terminal buffer reader ────────────────────────────────────────────────────
async function readTerminal(page: Page): Promise<string> {
  return page.evaluate(() => {
    const el = document.querySelector('.terminal-container > div') as any;
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

// ── Main ──────────────────────────────────────────────────────────────────────
async function main() {
  const adminToken = getAdminToken();
  const healthRes = await fetch(`${BASE_URL}/api/health`);
  if (!healthRes.ok) {
    console.error('Server not reachable at', BASE_URL, '— run `cargo run` first');
    process.exit(1);
  }
  console.log('Server OK');

  // ── Frame directory ───────────────────────────────────────────────────────────
  if (existsSync(FRAMES_DIR)) rmSync(FRAMES_DIR, { recursive: true });
  mkdirSync(FRAMES_DIR, { recursive: true });

  // Cleanup handles — declared before try so finally can always reach them
  let browser: Browser | undefined;
  let sessionId: string | undefined;
  let capturing = false;
  let capturePromise: Promise<void> | undefined;

  try {
    // ── Browser setup ───────────────────────────────────────────────────────────
    browser = await chromium.launch({ headless: false });

    // Lock locale to en-US so the i18n provider resolves to English copy
    // (matches e2e tests). The "Hide Sidebar" button text below depends on it.
    const ctxOpts = { viewport: VIEWPORT, locale: 'en-US' };
    const ownerCtx    = await browser.newContext(ctxOpts);
    const operatorCtx = await browser.newContext(ctxOpts);
    const viewerCtx   = await browser.newContext(ctxOpts);

    const ownerPage    = await ownerCtx.newPage();
    const operatorPage = await operatorCtx.newPage();
    const viewerPage   = await viewerCtx.newPage();

    // ── Owner: log in via UI, create session via dashboard ──────────────────────
    await ownerPage.goto(`${BASE_URL}/login`);
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
    console.log('Owner connected');

    // ── Create invite tokens via REST ───────────────────────────────────────────
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

    // ── Operator joins via invite link ──────────────────────────────────────────
    await operatorPage.goto(`${BASE_URL}/join/${operatorInvite.token}`);
    await operatorPage.waitForURL(/\/session\/.+/, { timeout: 10_000 });
    await operatorPage.locator('.xterm').waitFor({ state: 'visible', timeout: 10_000 });
    await operatorPage.locator('.status-dot[data-status="connected"]').waitFor({
      state: 'attached', timeout: 10_000,
    });
    console.log('Operator connected');

    // ── Viewer joins via invite link ────────────────────────────────────────────
    await viewerPage.goto(`${BASE_URL}/join/${viewerInvite.token}`);
    await viewerPage.waitForURL(/\/session\/.+/, { timeout: 10_000 });
    await viewerPage.locator('.xterm').waitFor({ state: 'visible', timeout: 10_000 });
    await viewerPage.locator('.status-dot[data-status="connected"]').waitFor({
      state: 'attached', timeout: 10_000,
    });
    console.log('Viewer connected');

    // ── Close sidebars + bump font + hide cursors (clean recording) ─────────────
    // Sidebar default-open eats half the viewport at this size; close it on
    // each page so the terminal gets the full width and htop renders cleanly.
    // Clicking the actual button (vs. CSS-hide) lets Solid's reactive resize
    // run so xterm refits to the new container width.
    for (const p of [ownerPage, operatorPage, viewerPage]) {
      await p.getByRole('button', { name: 'Hide Sidebar' }).click();
      await p.locator('.sidebar').waitFor({ state: 'detached', timeout: 5_000 });

      // Bump xterm font size for the recording. Setting `term.options.fontSize`
      // triggers an internal refresh; the container's ResizeObserver then
      // catches the new dimensions and refits cols/rows.
      await p.evaluate((size) => {
        const el = document.querySelector('.terminal-container > div') as any;
        const term = el?.__xterm;
        if (term) term.options.fontSize = size;
      }, RECORDING_FONT_SIZE);

      await p.addStyleTag({ content: '* { cursor: none !important; }' });
    }
    // Give xterm time to refit after the resize + font swap (debounced 100ms
    // ResizeObserver in Terminal.tsx, plus a margin for the actual fit pass).
    await ownerPage.waitForTimeout(800);

    // ── Frame capture ───────────────────────────────────────────────────────────
    let frameIdx = 0;
    const captureFrame = async (): Promise<void> => {
      const pad = String(frameIdx++).padStart(4, '0');
      const [ob, opb, vb] = await Promise.all([
        ownerPage.screenshot(),
        operatorPage.screenshot(),
        viewerPage.screenshot(),
      ]);
      writeFileSync(join(FRAMES_DIR, `owner_${pad}.png`), ob);
      writeFileSync(join(FRAMES_DIR, `operator_${pad}.png`), opb);
      writeFileSync(join(FRAMES_DIR, `viewer_${pad}.png`), vb);
    };

    capturing = true;
    const captureLoop = async (): Promise<void> => {
      while (capturing) {
        const t = Date.now();
        await captureFrame();
        const elapsed = Date.now() - t;
        const wait = Math.max(0, FRAME_MS - elapsed);
        if (wait > 0) await sleep(wait);
      }
    };
    capturePromise = captureLoop();

    // ── Demo sequence ───────────────────────────────────────────────────────────

    // Scene 1: 1.5s static — let viewer appreciate 3 windows are connected
    await ownerPage.waitForTimeout(1_500);

    // Scene 2: Owner launches htop
    await ownerPage.locator('.xterm').click();
    await ownerPage.keyboard.type('htop', { delay: 80 });
    await ownerPage.keyboard.press('Enter');

    // Wait for htop TUI to appear in Owner's buffer
    const htopDeadline = Date.now() + 10_000;
    while (Date.now() < htopDeadline) {
      const txt = await readTerminal(ownerPage);
      if (txt.includes('%CPU') || txt.includes('Load average')) break;
      await ownerPage.waitForTimeout(200);
    }
    if (Date.now() >= htopDeadline) {
      console.warn('Warning: htop did not appear within 10s; proceeding anyway');
    }

    // Scene 3: 2s on htop — all three windows show the same TUI
    await ownerPage.waitForTimeout(2_000);

    // Scene 4: Viewer tries `q` — silently blocked
    await viewerPage.locator('.xterm').click();
    await viewerPage.keyboard.type('q');
    await viewerPage.waitForTimeout(1_500);

    // Scene 5: Operator presses `q` — htop exits in all three windows
    await operatorPage.locator('.xterm').click();
    await operatorPage.keyboard.type('q');

    // Wait for htop to exit (shell prompt returns); 300ms initial delay lets the
    // PTY process `q` before the first buffer read to avoid a false-negative.
    await ownerPage.waitForTimeout(300);
    const exitDeadline = Date.now() + 8_000;
    while (Date.now() < exitDeadline) {
      const txt = await readTerminal(ownerPage);
      if (!txt.includes('%CPU')) break;
      await ownerPage.waitForTimeout(200);
    }

    // Scene 6: 2s hold after htop exits
    await ownerPage.waitForTimeout(2_000);

    // ── Stop capture ────────────────────────────────────────────────────────────
    capturing = false;
    await capturePromise;
    capturePromise = undefined;

    if (frameIdx === 0) throw new Error('No frames captured — check screenshot permissions');
    console.log(`Captured ${frameIdx} frames`);

    // ── ffmpeg assembly ─────────────────────────────────────────────────────────
    const TMP_MP4     = join(__dirname, '../tmp/telepair_demo.mp4');
    const TMP_PALETTE = join(__dirname, '../tmp/telepair_palette.png');

    console.log('Stitching frames with ffmpeg...');

    // vstack three image sequences into one video — vertical stacking keeps
    // each pane at full viewport width, which fits a typical README column
    // far better than three side-by-side panes squeezed into the same width.
    execSync(
      [
        'ffmpeg -y',
        `-framerate ${FPS} -start_number 0 -i "${FRAMES_DIR}/owner_%04d.png"`,
        `-framerate ${FPS} -start_number 0 -i "${FRAMES_DIR}/operator_%04d.png"`,
        `-framerate ${FPS} -start_number 0 -i "${FRAMES_DIR}/viewer_%04d.png"`,
        `-filter_complex "[0:v][1:v][2:v]vstack=inputs=3[out]"`,
        `-map "[out]" "${TMP_MP4}"`,
      ].join(' '),
      { stdio: 'inherit' },
    );

    // Generate colour palette from the merged video
    execSync(
      `ffmpeg -y -i "${TMP_MP4}" -vf "fps=${FPS},palettegen" "${TMP_PALETTE}"`,
      { stdio: 'inherit' },
    );

    // Render final GIF using palette
    execSync(
      [
        `ffmpeg -y -i "${TMP_MP4}" -i "${TMP_PALETTE}"`,
        `-filter_complex "[0:v]fps=${FPS}[x];[x][1:v]paletteuse"`,
        `"${OUTPUT_GIF}"`,
      ].join(' '),
      { stdio: 'inherit' },
    );

    const gifSize = statSync(OUTPUT_GIF).size;
    const gifMB = (gifSize / 1_048_576).toFixed(2);
    console.log(`Demo GIF: ${OUTPUT_GIF} (${gifMB} MB)`);

    if (gifSize > 4 * 1_048_576) {
      console.warn('Warning: GIF exceeds 4 MB target. Consider reducing FPS or viewport.');
    }

  } finally {
    // Stop capture loop if still running (demo sequence threw before reaching stop-capture)
    capturing = false;
    if (capturePromise) await capturePromise.catch(() => {});

    // Delete the session from the server
    if (sessionId) {
      await fetch(`${BASE_URL}/api/sessions/${sessionId}`, {
        method: 'DELETE',
        headers: { Authorization: `Bearer ${adminToken}` },
      }).catch(() => {});
    }

    // Close browser (optional chaining — browser may not have opened if launch failed)
    await browser?.close().catch(() => {});
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
