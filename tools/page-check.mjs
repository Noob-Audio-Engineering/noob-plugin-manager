// Draw the manager's page and say what came out.
//
//   node tools/page-check.mjs [out.png] [shot.png]      a state written here
//   node tools/page-check.mjs --real [out.png]          the state the program sends
//
// `--real` runs `noob view`, which prints the exact value the window is handed,
// and renders that. It needs a network and it needs the plug-ins to have
// published, and it is the only one of the two that can disagree with the
// program --- which is the only interesting thing a check of a page can do.
//
// Without it, the state below is written by hand: enough to work on the layout
// offline, and proof of nothing except that the page draws what it is given.

import { chromium } from 'playwright';
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { dirname, join, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const argv = process.argv.slice(2);
const real = argv[0] === '--real';
if (real) argv.shift();
const out = resolve(argv[0] || join(here, '..', 'page-check.png'));
process.argv = [process.argv[0], process.argv[1], ...argv];
// Inlined rather than linked: a page built with `setContent` has no origin,
// so a `file://` picture is refused. Over the wire the manager is handed an
// ordinary https address and this question does not arise.
const shot = process.argv[3]
  ? 'data:image/png;base64,' + readFileSync(resolve(process.argv[3])).toString('base64')
  : null;

let state = {
  plugins: [
    {
      id: 'noob-wave', version: '0.1.0', commit: 'de4a38e', built: '2026-09-05',
      state: 'missing', installed: null, banner: shot,
      display: {
        name: 'Noob Wave', tagline: 'a wavetable synth', kind: 'instrument',
        accent: '#f08a3c', features: ['Wavetables', 'Two envelopes', 'One LFO'],
      },
    },
    {
      id: 'noob-resonator', version: '0.1.0', commit: '5d4ef6e', built: '2026-09-05',
      state: 'behind', installed: '0.0.9', banner: shot,
      display: {
        name: 'Noob Resonator', tagline: 'the object rings; you supply the strike',
        kind: 'effect', accent: '#4fd6c8', features: ['Ten objects', 'A mode bank'],
      },
    },
    {
      id: 'noob-q', version: '0.1.0', commit: 'edfdef3', built: '2026-09-05',
      state: 'current', installed: '0.1.0', banner: null,
      display: { name: 'Noob-Q', tagline: 'an equaliser', kind: 'effect', accent: '#7aa2f7', features: [] },
    },
  ],
  paths: [['vst3', String.raw`C:\Program Files\Common Files\VST3`], ['clap', String.raw`C:\Program Files\Common Files\CLAP`]],
  problem: null, note: null, canElevate: false,
  // The same shape and the same spelling Rust sends, so a rename there shows
  // up here as a broken page rather than as nothing.
  settings: { prefer_user_dirs: false, token: null, shared_writable: true },
};

// The program's own answer, when asked for. Built by Rust, not here.
if (real) {
  const bin = join(here, '..', 'target', 'release', process.platform === 'win32' ? 'noob.exe' : 'noob');
  const r = spawnSync(bin, ['view'], { encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 });
  if (r.status !== 0) {
    console.error(`\`noob view\` failed (${r.status}): ${r.stderr || r.error}`);
    process.exit(1);
  }
  Object.assign(state, JSON.parse(r.stdout));
  console.log(`state from \`noob view\`: ${state.plugins.length} plug-ins`);
}

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1000, height: 860 }, deviceScaleFactor: 2 });
const problems = [];
page.on('pageerror', (e) => problems.push(String(e)));
page.on('console', (m) => { if (m.type() === 'error') problems.push(m.text()); });

// wry gives the page `window.ipc` before the page's own script runs, and the
// page asks for a refresh as it parses. The stand-in therefore goes into the
// document ahead of it, not into an init script --- which would arrive too
// late and leave one error that has nothing to do with the drawing.
const html = readFileSync(join(here, '..', 'ui', 'index.html'), 'utf8').replace(
  '<script>',
  '<script>window.ipc={sent:[],postMessage(m){this.sent.push(m)}}</script><script>',
);
await page.setContent(html, { waitUntil: 'load' });
// Rust evaluates `window.applyState({json})`, so it is handed the object
// itself, not a string. Handing it a string here would test a call that is
// never made.
await page.evaluate((s) => window.applyState(s), state);

// Both faces of the page: the home list, then a plug-in's own page.
await page.waitForTimeout(400);
await page.screenshot({ path: out.replace(/\.png$/, '-home.png') });

const heads = await page.$$eval('.grouphead', (n) => n.map((e) => e.textContent));
const cards = await page.$$eval('.card', (n) => n.length);
const thumbs = await page.$$eval('.card .thumb', (n) => n.length);

await page.evaluate(() => document.querySelectorAll('#navplugins .navitem')[0].click());
// Wait for the picture itself rather than for a guess at how long a download
// takes: with a real state the banner comes off the network, and a fixed pause
// reports "no photograph" whenever the network is slower than the pause.
await page
  .waitForFunction(
    () => {
      const i = document.querySelector('.banner img');
      return !i || (i.complete && i.naturalWidth > 0);
    },
    { timeout: 30_000 },
  )
  .catch(() => console.error('the banner picture did not load in 30 s'));
await page.waitForTimeout(300);
await page.screenshot({ path: out.replace(/\.png$/, '-plugin.png') });
const banner = await page.$eval('.banner', (e) => ({
  shot: e.classList.contains('shot'),
  img: !!e.querySelector('img'),
}));

await browser.close();

console.log('groups on the home page :', heads.join(' | ') || '(none)');
console.log('cards / with a picture  :', cards, '/', thumbs);
console.log('plug-in page banner     :', JSON.stringify(banner));
if (problems.length) {
  console.error('the page reported problems:');
  for (const p of problems.slice(0, 8)) console.error('  ' + p);
  process.exit(1);
}
