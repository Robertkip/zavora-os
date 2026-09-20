/**
 * Hand-gesture classifier (M10-T5, local detection). Pure: takes per-frame hand facts and returns
 * one of the five UI gestures or null. Runs in the browser (gesture-detect.js feeds it MediaPipe
 * results) and in Node (scripts/test-gesture-classifier.mjs), so the timing rules are testable.
 *
 * Frame: { t: ms, present: bool, category: MediaPipe canned gesture name or 'None',
 *          palmX: 0..1 (image x of the palm centre), pinchDist: 0..1 (thumb tip ↔ index tip) }.
 *
 * Image coordinates come from an un-mirrored front camera: the user's left is the image's right.
 * A hand travelling to the user's LEFT therefore has increasing x, and that is `swipe_left`
 * (toward the Home world, like a touch swipe left). Pass { mirrored: true } for mirrored feeds.
 */
(function (root, factory) {
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  root.AgentrixGestureClassifier = api;
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict';

  const DEFAULTS = {
    mirrored: false,
    cooldownMs: 1500, // after a gesture fires, nothing else for this long
    windowMs: 1500, // how much history the wave / swipe rules look at
    pinchDist: 0.06, // thumb–index distance (normalised) that counts as a pinch
    pinchFrames: 2, // consecutive pinched frames required
    swipeTravel: 0.25, // net horizontal travel (fraction of frame width)
    swipeMs: 650, // …within this time
    waveReversals: 3, // direction changes for a wave…
    waveAmplitude: 0.05, // …each at least this wide
    holdMs: 800, // open palm held still this long → pause
    holdJitter: 0.06, // …with less than this much travel
  };

  function createGestureClassifier(opts) {
    const o = Object.assign({}, DEFAULTS, opts || {});
    let frames = [];
    let lastFiredAt = -Infinity;
    let pinchRun = 0;
    let palmArmed = true; // open_palm re-arms only after the palm leaves

    function reset() {
      frames = [];
      lastFiredAt = -Infinity;
      pinchRun = 0;
      palmArmed = true;
    }

    function fire(gesture, t) {
      lastFiredAt = t;
      frames = [];
      pinchRun = 0;
      return gesture;
    }

    function swipe(now) {
      // Walk back from the newest frame while the motion stays within swipeMs and does not
      // reverse; the net travel of that run decides.
      const recent = frames.filter((f) => f.present && now - f.t <= o.swipeMs);
      if (recent.length < 3) return null;
      const dx = recent[recent.length - 1].palmX - recent[0].palmX;
      if (Math.abs(dx) < o.swipeTravel) return null;
      let reversals = 0;
      for (let i = 2; i < recent.length; i++) {
        const a = recent[i - 1].palmX - recent[i - 2].palmX;
        const b = recent[i].palmX - recent[i - 1].palmX;
        if (Math.abs(a) > 0.01 && Math.abs(b) > 0.01 && Math.sign(a) !== Math.sign(b)) reversals++;
      }
      if (reversals > 1) return null;
      const toImageRight = dx > 0;
      const userLeft = o.mirrored ? !toImageRight : toImageRight;
      return userLeft ? 'swipe_left' : 'swipe_right';
    }

    function wave(now) {
      const recent = frames.filter((f) => f.present && now - f.t <= o.windowMs);
      if (recent.length < 6) return null;
      const palms = recent.filter((f) => f.category === 'Open_Palm').length;
      if (palms < recent.length * 0.5) return null;
      // count reversals of horizontal motion with enough amplitude between turning points
      let reversals = 0;
      let dir = 0;
      let extreme = recent[0].palmX;
      for (let i = 1; i < recent.length; i++) {
        const x = recent[i].palmX;
        const d = Math.sign(x - extreme);
        if (d === 0) continue;
        if (dir === 0) {
          dir = d;
          extreme = x;
        } else if (d === dir) {
          extreme = x;
        } else if (Math.abs(x - extreme) >= o.waveAmplitude) {
          reversals++;
          dir = d;
          extreme = x;
        }
      }
      return reversals >= o.waveReversals ? 'wave' : null;
    }

    function openPalmHeld(now) {
      const run = [];
      for (let i = frames.length - 1; i >= 0; i--) {
        const f = frames[i];
        if (!f.present || f.category !== 'Open_Palm') break;
        run.unshift(f);
      }
      if (run.length < 2) return null;
      if (now - run[0].t < o.holdMs) return null;
      const xs = run.map((f) => f.palmX);
      if (Math.max(...xs) - Math.min(...xs) > o.holdJitter) return null;
      if (run.some((f) => f.pinchDist < o.pinchDist)) return null;
      return 'open_palm';
    }

    /** Feed one frame; returns a gesture name once, or null. */
    function push(frame) {
      const f = {
        t: frame.t,
        present: !!frame.present,
        category: frame.category || 'None',
        palmX: typeof frame.palmX === 'number' ? frame.palmX : 0.5,
        pinchDist: typeof frame.pinchDist === 'number' ? frame.pinchDist : 1,
      };
      frames.push(f);
      frames = frames.filter((x) => f.t - x.t <= o.windowMs * 2);

      if (!f.present || f.category !== 'Open_Palm') palmArmed = true;
      pinchRun = f.present && f.pinchDist < o.pinchDist ? pinchRun + 1 : 0;

      if (f.t - lastFiredAt < o.cooldownMs) return null;
      if (!f.present) return null;

      if (pinchRun >= o.pinchFrames) return fire('pinch', f.t);
      const s = swipe(f.t);
      if (s) return fire(s, f.t);
      const w = wave(f.t);
      if (w) return fire(w, f.t);
      if (palmArmed) {
        const p = openPalmHeld(f.t);
        if (p) {
          palmArmed = false;
          return fire(p, f.t);
        }
      }
      return null;
    }

    return { push, reset, options: o };
  }

  return { createGestureClassifier, DEFAULTS };
});
