// Unit tests for web/static/gesture-classifier.js — run with `node scripts/test-gesture-classifier.mjs`.
import { createRequire } from 'node:module';
import assert from 'node:assert/strict';

const require = createRequire(import.meta.url);
const { createGestureClassifier } = require('../web/static/gesture-classifier.js');

const FPS = 11; // gesture-detect.js ticks every 90 ms
const dt = Math.round(1000 / FPS);

function feed(c, seq) {
  const out = [];
  let t = 0;
  for (const f of seq) {
    const g = c.push({ t, ...f });
    if (g) out.push(g);
    t += dt;
  }
  return out;
}
const palm = (x, extra) => ({ present: true, category: 'Open_Palm', palmX: x, pinchDist: 0.3, ...extra });
const still = (n, x, cat = 'Open_Palm') => Array.from({ length: n }, () => palm(x, { category: cat }));
const sweep = (from, to, n, cat = 'None') =>
  Array.from({ length: n }, (_, i) => palm(from + ((to - from) * i) / (n - 1), { category: cat }));
const absent = (n) => Array.from({ length: n }, () => ({ present: false, category: 'None' }));

// 1. A hand moving to the image's right (the user's left) in half a second is swipe_left.
{
  const c = createGestureClassifier();
  assert.deepEqual(feed(c, [...still(2, 0.3, 'None'), ...sweep(0.3, 0.7, 6)]), ['swipe_left']);
}
// 2. The same motion on a mirrored feed is swipe_right.
{
  const c = createGestureClassifier({ mirrored: true });
  assert.deepEqual(feed(c, sweep(0.3, 0.7, 6)), ['swipe_right']);
}
// 3. Right-to-left travel is swipe_right; a slow drift is nothing.
{
  assert.deepEqual(feed(createGestureClassifier(), sweep(0.8, 0.4, 6)), ['swipe_right']);
  assert.deepEqual(feed(createGestureClassifier(), sweep(0.4, 0.6, 30)), []); // 0.2 over 2.7 s
}
// 4. Open palm held still for a second pauses — once, until the palm leaves and comes back.
{
  const c = createGestureClassifier();
  const out = feed(c, [...still(14, 0.5), ...still(20, 0.5), ...absent(5), ...still(14, 0.5)]);
  assert.deepEqual(out, ['open_palm', 'open_palm']);
}
// 5. An open palm that keeps moving is not a pause.
{
  assert.deepEqual(feed(createGestureClassifier(), sweep(0.4, 0.5, 14, 'Open_Palm')), []);
}
// 6. Three reversals of an open palm inside 1.5 s is a wave (not three swipes).
{
  const c = createGestureClassifier();
  const seq = [...sweep(0.4, 0.55, 3, 'Open_Palm'), ...sweep(0.55, 0.4, 3, 'Open_Palm'), ...sweep(0.4, 0.55, 3, 'Open_Palm'), ...sweep(0.55, 0.4, 3, 'Open_Palm')];
  const out = feed(c, seq);
  assert.deepEqual(out, ['wave'], JSON.stringify(out));
}
// 7. Thumb and index tip together for two frames is a pinch; it wins over everything else.
{
  const c = createGestureClassifier();
  const out = feed(c, [palm(0.5), palm(0.5, { pinchDist: 0.03, category: 'None' }), palm(0.5, { pinchDist: 0.03, category: 'None' })]);
  assert.deepEqual(out, ['pinch']);
}
// 8. Cooldown: a second gesture inside 1.5 s is ignored; after it, detection resumes.
{
  const c = createGestureClassifier();
  const out = feed(c, [...sweep(0.3, 0.7, 6), ...sweep(0.7, 0.3, 6), ...absent(20), ...sweep(0.7, 0.3, 6)]);
  assert.deepEqual(out, ['swipe_left', 'swipe_right'], JSON.stringify(out));
}
// 9. No hand, no gesture; malformed frames are tolerated.
{
  const c = createGestureClassifier();
  assert.deepEqual(feed(c, absent(30)), []);
  assert.equal(c.push({ t: 99999 }), null);
}
console.log('gesture-classifier: 9 scenarios passed');
