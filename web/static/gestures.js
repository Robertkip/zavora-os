/**
 * Camera gestures → UI verbs (M10-T5).
 * live-voice.js relays Suzy's `ui_gesture` tool call as a `agentrix:gesture` event; this maps it
 * to the same actions a click or key would take, through the normal routes, so the permission
 * gate and audit apply unchanged. Live mode only — the demo tour never sees a camera.
 */
(function () {
  'use strict';

  const BRIEFING = 'What do I need to know today?';

  function toast(msg) {
    window.__AGENTRIX_UI__?.showSuzyCustom?.(msg);
  }

  function sessionId() {
    return window.__AGENTRIX_LIVE__?.getSessionId?.() || sessionStorage.getItem('agentrix_session_id') || null;
  }

  async function pauseAgents() {
    try {
      const res = await fetch('/api/pause', {
        method: 'POST',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ session_id: sessionId() }),
      });
      if (res.status === 401 || res.status === 403) {
        toast('Sign in to pause the agents.');
        return;
      }
      if (!res.ok) {
        toast('Could not pause the agents.');
        return;
      }
      toast('Agents paused — nothing runs until you resume.');
    } catch (_) {
      toast('Could not reach the server.');
    }
  }

  // Pinch closes a window: the card in front (focused, else the newest) through the field's own
  // fling — the same dismiss a drag would do — or, with no card open, the camera window itself.
  function closeWindow() {
    const card = document.querySelector('#cards .card.focused') || [...document.querySelectorAll('#cards .card')].pop();
    if (card && typeof window.fling === 'function') {
      window.fling(card);
      return;
    }
    const live = window.AgentrixLiveVoice;
    if (live?.isCameraActive?.()) {
      live.stopCamera();
      document.getElementById('cam')?.classList.remove('listening');
      toast('Camera window closed.');
    }
  }

  // The local detector (gesture-detect.js) and Suzy's ui_gesture tool call can both report the
  // same gesture; the second report inside 1.5 s is the same gesture, not a new one.
  let lastGesture = null;
  let lastAt = 0;

  function onGesture(ev) {
    if (window.__AGENTRIX_DEMO__) return;
    const gesture = ev.detail?.gesture;
    const now = Date.now();
    if (gesture === lastGesture && now - lastAt < 1500) return;
    lastGesture = gesture;
    lastAt = now;
    const lens = window.__AGENTRIX_LENS__;
    switch (gesture) {
      case 'swipe_left':
        lens?.step?.(1); // toward Home, like a touch swipe
        break;
      case 'swipe_right':
        lens?.step?.(-1); // toward Work
        break;
      case 'open_palm':
        pauseAgents();
        break;
      case 'pinch':
        closeWindow();
        break;
      case 'wave':
        window.dispatchEvent(
          new CustomEvent('agentrix:voice-intent', { detail: { sessionId: sessionId(), args: { text: BRIEFING } } })
        );
        break;
      default:
        return;
    }
    // Audible confirmation that the gesture was seen — the same ding a completed action plays.
    window.__AGENTRIX_UI__?.sfx?.('ding');
    const world = document.body.dataset.world;
    window.__AGENTRIX_LIVE__?.recordUiEvent?.('ui_gesture', {
      domain: world === 'work' || world === 'home' ? world : 'shared',
    });
  }

  window.addEventListener('agentrix:gesture', onGesture);
})();
