// Draw the manager's page against a made-up state, so the interface can be
// worked on without a network and without waiting for a release.
//
// What this proves is only that the page renders what it is handed: the state
// here is written by hand, so it can never disagree with the manager. Whether
// the manager sends this shape is a question for the manager's own tests and
// for running it.
//
//   node tools/page-check.mjs [out.png]

import { chromium } from 'playwright';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const out = resolve(process.argv[2] || join(here, '..', 'page-check.png'));
// Inlined rather than linked: a page built with `setContent` has no origin,
// so a `file://` picture is refused. Over the wire the manager is handed an
// ordinary https address and this question does not arise.
const shot = process.argv[3]
  ? 'data:image/png;base64,' + readFileSync(resolve(process.argv[3])).toString('base64')
  : null;

const state = {
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
await page.waitForTimeout(700);
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
