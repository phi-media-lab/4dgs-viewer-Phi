const video = document.querySelector('#video');
const statusNode = document.querySelector('#status');
const errorNode = document.querySelector('#error');
const QUERY = new URLSearchParams(location.search);
const DIAGNOSTICS_ENABLED = QUERY.get('diagnostics') === '1';
const HUD_ENABLED = QUERY.get('hud') !== '0';
const RECEIVER_PROGRESS_SCHEMA = 1;
const RECEIVER_TELEMETRY_SCHEMA = 7;

function parseJitterBufferRequest(rawValue) {
  const normalized = rawValue === null ? 'browser' : rawValue.trim().toLowerCase();
  if (normalized === 'browser' || normalized === 'null' || normalized === 'auto') {
    return { mode: 'browser', targetMs: null };
  }
  if (/^(0|[1-9][0-9]*)(\.[0-9]+)?$/.test(normalized)) {
    const targetMs = Number(normalized);
    if (Number.isFinite(targetMs) && targetMs >= 0 && targetMs <= 4000) {
      return { mode: 'numeric', targetMs };
    }
  }
  return { mode: 'browser', targetMs: null };
}

const JITTER_BUFFER_REQUEST = parseJitterBufferRequest(QUERY.get('jitter_buffer_ms'));
const previewOwnerIdQuery = QUERY.get('owner_id');
const PREVIEW_OWNER_ID = previewOwnerIdQuery !== null
    && /^[a-zA-Z0-9_-]{16,64}$/.test(previewOwnerIdQuery)
  ? previewOwnerIdQuery : '';
const CLIENT_PROTOCOL = 2;
const CONTROL_EPOCH_STORAGE_KEY = 'phi-4dgs-camera-epoch-v1';
const RELOAD_RECLAIM_STORAGE_KEY = 'phi-4dgs-owner-reclaim-v1';
const RELOAD_RECLAIM_TTL_MS = 10000;
const CONTROL_EPOCH_MIGRATION_SEED = 0x4000_0000;
const MAX_CONTROL_BUFFERED_BYTES = 1024;
const CONTROL_HEARTBEAT_MS = 100;
const CONTROL_SEND_INTERVAL_MS = 8;
const WHEEL_TAIL_DEBOUNCE_MS = 100;
const DISCONNECTED_GRACE_MS = 2000;
const SIGNALING_TIMEOUT_MS = 12000;
const MEDIA_KEYFRAME_REQUEST_AFTER_MS = 900;
const MEDIA_PROGRESS_TIMEOUT_MS = 2500;
const MEDIA_FIRST_FRAME_TIMEOUT_MS = 6000;
const MEDIA_KEYFRAME_REQUEST_GRACE_MS = 1000;
const GET_STATS_TIMEOUT_MS = 1500;
const STATUS_FETCH_TIMEOUT_MS = 1500;
const GET_STATS_TIMEOUTS_BEFORE_RECONNECT = 2;
const LIFECYCLE_GATE_ENABLED = QUERY.get('lifecycle_gate') === '1';
const LIFECYCLE_RELOAD_DELAY_MS = 500;
const RECOVERY_BACKOFF_RESET_MS = 30000;
const MAX_RECOVERY_BACKOFF_MS = 12000;
const BACKGROUND_OWNER_GRACE_MS = 5000;
const BACKGROUND_PRESENTATION_STALL_MS = 3000;
const DIAGNOSTIC_SAMPLE_CAPACITY = 600;
const HAS_VIDEO_FRAME_CALLBACK = DIAGNOSTICS_ENABLED
  && typeof video.requestVideoFrameCallback === 'function';
const HAS_ANIMATION_FRAME_PROBE = DIAGNOSTICS_ENABLED;

class FixedRingBuffer {
  constructor(capacity) {
    this.capacity = capacity;
    this.values = new Array(capacity);
    this.start = 0;
    this.size = 0;
  }

  get length() { return this.size; }

  push(value) {
    const index = (this.start + this.size) % this.capacity;
    this.values[index] = value;
    if (this.size < this.capacity) {
      this.size += 1;
    } else {
      this.start = (this.start + 1) % this.capacity;
    }
  }

  *[Symbol.iterator]() {
    for (let offset = 0; offset < this.size; offset += 1) {
      yield this.values[(this.start + offset) % this.capacity];
    }
  }
}

const diagnosticWindow = () => new FixedRingBuffer(DIAGNOSTIC_SAMPLE_CAPACITY);

function restoredControlEpoch() {
  try {
    const stored = localStorage.getItem(CONTROL_EPOCH_STORAGE_KEY);
    if (stored !== null && /^(0|[1-9][0-9]*)$/.test(stored)) {
      const value = Number(stored);
      if (Number.isSafeInteger(value) && value >= 0 && value <= 0xffff_fffe) return value;
    }
  } catch {
    // A storage-disabled context still works within this document. The normal
    // isolated preview profile provides storage for monotonicity across reloads.
  }
  return CONTROL_EPOCH_MIGRATION_SEED;
}

function persistControlEpoch(epoch) {
  try { localStorage.setItem(CONTROL_EPOCH_STORAGE_KEY, String(epoch)); } catch {
    // See restoredControlEpoch: persistence hardens reloads but is not needed
    // for monotonic reconnects within the current document.
  }
}

function consumeReloadReclaimGrant() {
  try {
    const stored = sessionStorage.getItem(RELOAD_RECLAIM_STORAGE_KEY);
    sessionStorage.removeItem(RELOAD_RECLAIM_STORAGE_KEY);
    if (!stored) return { active: false, expiresAt: 0 };
    const grant = JSON.parse(stored);
    const expiresAt = Number(grant?.expires_at || 0);
    return expiresAt > Date.now()
      ? { active: true, expiresAt } : { active: false, expiresAt: 0 };
  } catch {
    return { active: false, expiresAt: 0 };
  }
}

function armReloadReclaimGrant() {
  if (!hasOwnedRenderer || standbyUntilFocus || rendererBusy) return;
  try {
    sessionStorage.setItem(RELOAD_RECLAIM_STORAGE_KEY, JSON.stringify({
      expires_at: Date.now() + RELOAD_RECLAIM_TTL_MS,
    }));
  } catch {
    // Storage failure falls back to the safe behavior: a focused page can
    // reconnect, while an unfocused page cannot silently claim the renderer.
  }
}

function requestLifecycleReload(delayMs = LIFECYCLE_RELOAD_DELAY_MS) {
  if (!LIFECYCLE_GATE_ENABLED || lifecycleReloadPending
      || !hasOwnedRenderer || standbyUntilFocus || rendererBusy) return false;
  armReloadReclaimGrant();
  try {
    if (!sessionStorage.getItem(RELOAD_RECLAIM_STORAGE_KEY)) return false;
  } catch {
    return false;
  }
  const requestedDelay = Number(delayMs);
  const boundedDelay = Number.isFinite(requestedDelay)
    ? Math.max(100, Math.min(5000, requestedDelay)) : LIFECYCLE_RELOAD_DELAY_MS;
  lifecycleReloadPending = true;
  setTimeout(() => location.reload(), boundedDelay);
  return true;
}

statusNode.hidden = !HUD_ENABLED;
const reloadReclaimGrant = consumeReloadReclaimGrant();
let peer;
let control;
let config;
let mediaReceiver;
let retryTimer;
let disconnectedTimer;
let connectionReadyTimer;
let connecting = false;
let connectionGeneration = 0;
let offerAbortController;
let statsPending = false;
let statsRequestGeneration = 0;
let statsSnapshotInFlight;
let statsSnapshotPeer;
let statsTimeoutStreak = 0;
let statusPollInFlight;
let statusPollExplicitActivation = false;
let controlTimer = 0;
let wheelTailTimer = 0;
let reliableCameraTailPending = false;
let lastControlSentAt = 0;
let controlEpoch = restoredControlEpoch();
let controlSequence = 0;
let cumulativeOrbitX = 0;
let cumulativeOrbitY = 0;
let cumulativeZoom = 0;
let controlDirty = true;
let presentedFrames = 0;
let lastPresentedFrames = 0;
let lastStatsAt = performance.now();
let lastPresentedAt = 0;
let lastMetadataPresentedFrames = null;
let presentationGeneration = 0;
let presentationIntervals = diagnosticWindow();
let presentationGapCount = 0;
let presentationTimingTotalSamples = 0;
let presentationTimingCensoredFrames = 0;
let videoFrameCallbacks = 0;
let videoFrameCallbackMissed = 0;
let videoFrameCallbackLead = diagnosticWindow();
let pendingInputAt = 0;
let pendingDragInput = false;
let pendingWheelInput = false;
let controlMessagesSent = 0;
let controlInputMessagesSent = 0;
let controlDragInputMessagesSent = 0;
let controlWheelInputMessagesSent = 0;
let controlInputToSendIntervals = diagnosticWindow();
let controlBufferedAmountMaxBytes = 0;
let controlBackpressureSkipCount = 0;
let pageVisibilityTransitionCount = 0;
let pageFocusTransitionCount = 0;
let animationFrames = 0;
let lastAnimationFrames = 0;
let lastAnimationFrameAt = 0;
let animationFrameIntervals = diagnosticWindow();
let captureToDisplayIntervals = diagnosticWindow();
let captureToReceiveIntervals = diagnosticWindow();
let receiveToDisplayIntervals = diagnosticWindow();
let frameProcessingIntervals = diagnosticWindow();
let receiverJitterBufferTargetApi = 'browser-default';
let inboundFramesSeen = false;
let lastInboundFrames = 0;
let lastInboundSsrc = 0;
let lastInboundFrameProgressAt = 0;
let mediaConnectedAt = 0;
let mediaKeyframeRequestAt = 0;
let mediaKeyframeRequestId = 0;
let mediaKeyframeRequestsSent = 0;
let mediaRecoveryState = 'waiting-first-frame';
let mediaHealthySince = 0;
let retryNotBefore = 0;
let reconnectFailureStreak = 0;
let reconnectCount = 0;
let lastReconnectReason = '';
let lastReconnectGeneration = 0;
let lastReconnectAtUnixMs = 0;
let lastRetryDelayMs = 0;
let lastJitterBufferSample;
let maximumObservedRttMs = 0;
let mediaAttachCount = 0;
let mediaDetachCount = 0;
let hasOwnedRenderer = reloadReclaimGrant.active;
let rendererBusy = false;
let reloadReclaimActive = reloadReclaimGrant.active;
let reloadReclaimExpiresAt = reloadReclaimGrant.expiresAt;
let standbyUntilFocus = document.visibilityState !== 'visible'
  || (!document.hasFocus() && !reloadReclaimActive);
let ownershipDecodedFrames = 0;
let ownershipSsrc = 0;
let ownershipDisplayedFrames = 0;
let ownershipVideoTime = 0;
let ownershipPresentationProgressAt = performance.now();
let longTaskProbeEnabled = false;
let longTaskCount = 0;
let longTaskTotalMs = 0;
let longTaskMaxMs = 0;
let lifecycleReloadPending = false;

if (DIAGNOSTICS_ENABLED) {
  try {
    const longTaskObserver = new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        longTaskCount += 1;
        longTaskTotalMs += entry.duration;
        longTaskMaxMs = Math.max(longTaskMaxMs, entry.duration);
      }
    });
    longTaskObserver.observe({ type: 'longtask', buffered: true });
    longTaskProbeEnabled = true;
  } catch {
    // Long Tasks is diagnostic attribution only; media/control stay functional.
  }
}

let drag = null;
video.addEventListener('pointerdown', (event) => {
  if (event.button !== 0) return;
  resumeOwnershipForUser();
  drag = { x: event.clientX, y: event.clientY };
  video.classList.add('dragging');
  video.setPointerCapture(event.pointerId);
});
video.addEventListener('pointermove', (event) => {
  if (!drag) return;
  if ((event.buttons & 1) === 0) {
    finishDrag();
    return;
  }
  const dx = event.clientX - drag.x;
  const dy = event.clientY - drag.y;
  drag = { x: event.clientX, y: event.clientY };
  cumulativeOrbitX += dx;
  cumulativeOrbitY += dy;
  markInputPending('drag');
  controlDirty = true;
  scheduleControlFlush();
});
const finishDrag = () => {
  if (!drag) return;
  drag = null;
  video.classList.remove('dragging');
  commitCameraTail();
};
video.addEventListener('pointerup', finishDrag);
video.addEventListener('pointercancel', finishDrag);
video.addEventListener('lostpointercapture', finishDrag);
video.addEventListener('wheel', (event) => {
  event.preventDefault();
  resumeOwnershipForUser();
  const unit = event.deltaMode === WheelEvent.DOM_DELTA_LINE
    ? 16
    : event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? video.clientHeight : 1;
  const delta = Math.max(-240, Math.min(240, event.deltaY * unit));
  cumulativeZoom += delta;
  markInputPending('wheel');
  controlDirty = true;
  scheduleControlFlush();
  clearTimeout(wheelTailTimer);
  wheelTailTimer = setTimeout(() => {
    wheelTailTimer = 0;
    commitCameraTail();
  }, WHEEL_TAIL_DEBOUNCE_MS);
}, { passive: false });
addEventListener('keydown', (event) => {
  if (event.code === 'KeyL' && event.ctrlKey && event.altKey && event.shiftKey) {
    if (!LIFECYCLE_GATE_ENABLED) return;
    event.preventDefault();
    if (!requestLifecycleReload()) {
      errorNode.textContent = 'Lifecycle reload was not armed from a live owner';
    }
    return;
  }
  resumeOwnershipForUser();
  if (event.code === 'KeyR') resetCameraInput();
  if (event.code === 'Space') {
    event.preventDefault();
    if (!config || config.readyState !== 'open') return;
    config.send(JSON.stringify({ type: 'set-playing', value: statusNode.dataset.playing !== 'false' ? false : true }));
    statusNode.dataset.playing = statusNode.dataset.playing !== 'false' ? 'false' : 'true';
  }
});

if (rendererClaimEligible()) connect();
else statusNode.textContent = 'REMOTE RENDERER · STANDBY · FOCUS TO CONNECT';
observePresentedFrame();
// The animation cadence probe is diagnostic attribution only. Do not keep a
// permanent rAF loop alive in the default receiver.
if (HAS_ANIMATION_FRAME_PROBE) requestAnimationFrame(observeAnimationFrame);
addEventListener('focus', () => {
  pageFocusTransitionCount += 1;
  deferMediaProgressWatchdog();
  resetPresentationWindow();
  resumeOwnershipForUser();
});
addEventListener('blur', () => {
  pageFocusTransitionCount += 1;
  finishDrag();
  resetPresentationWindow();
  if (!peer && !connecting && !hasOwnedRenderer) standbyUntilFocus = true;
});
addEventListener('pagehide', () => {
  hasOwnedRenderer = false;
  rendererBusy = false;
  reloadReclaimActive = false;
  reloadReclaimExpiresAt = 0;
  standbyUntilFocus = true;
  detachSession();
});
addEventListener('pageshow', () => {
  if (document.visibilityState === 'visible' && document.hasFocus()) {
    resumeOwnershipForUser();
  }
});
document.addEventListener('visibilitychange', () => {
  pageVisibilityTransitionCount += 1;
  resetPresentationWindow();
  if (document.visibilityState !== 'visible') {
    finishDrag();
    hasOwnedRenderer = false;
    rendererBusy = false;
    reloadReclaimActive = false;
    reloadReclaimExpiresAt = 0;
    standbyUntilFocus = true;
    detachSession();
    statusNode.textContent = 'REMOTE RENDERER · STANDBY · FOCUS TO CONNECT';
    return;
  }
  if (document.hasFocus()) resumeOwnershipForUser();
});
setInterval(() => {
  flushControls(true);
  flushReliableCameraTail();
}, CONTROL_HEARTBEAT_MS);

function scheduleControlFlush() {
  if (controlTimer) return;
  const delay = Math.max(0, CONTROL_SEND_INTERVAL_MS - (performance.now() - lastControlSentAt));
  controlTimer = setTimeout(() => flushControls(false), delay);
}

function markInputPending(kind) {
  if (!pendingInputAt) pendingInputAt = performance.now();
  if (kind === 'drag') pendingDragInput = true;
  if (kind === 'wheel') pendingWheelInput = true;
}

function observeControlBufferedAmount() {
  const bufferedAmount = Number(control?.bufferedAmount || 0);
  if (Number.isFinite(bufferedAmount)) {
    controlBufferedAmountMaxBytes = Math.max(controlBufferedAmountMaxBytes, bufferedAmount);
  }
  return bufferedAmount;
}

function cameraState(sequence) {
  return {
    type: 'camera-state',
    epoch: controlEpoch,
    sequence,
    client_time_ms: Date.now(),
    orbit_x: cumulativeOrbitX,
    orbit_y: cumulativeOrbitY,
    zoom: cumulativeZoom,
  };
}

function sendCameraState(channel) {
  if (!channel || channel.readyState !== 'open') return false;
  const sequence = controlSequence + 1;
  try {
    channel.send(JSON.stringify(cameraState(sequence)));
  } catch {
    return false;
  }
  controlSequence = sequence;
  controlMessagesSent += 1;
  lastControlSentAt = performance.now();
  if (pendingInputAt) {
    controlInputToSendIntervals.push(performance.now() - pendingInputAt);
    controlInputMessagesSent += 1;
    pendingInputAt = 0;
  }
  if (pendingDragInput) {
    controlDragInputMessagesSent += 1;
    pendingDragInput = false;
  }
  if (pendingWheelInput) {
    controlWheelInputMessagesSent += 1;
    pendingWheelInput = false;
  }
  controlDirty = false;
  return true;
}

function flushControls(force = false, bypassBackpressure = false) {
  if (controlTimer) clearTimeout(controlTimer);
  controlTimer = 0;
  if (document.visibilityState !== 'visible'
      || !control || control.readyState !== 'open') return;
  if (!bypassBackpressure && observeControlBufferedAmount() > MAX_CONTROL_BUFFERED_BYTES) {
    // Wait for bufferedamountlow (with the heartbeat as a fallback). Retrying
    // immediately here can become a zero-delay timer loop once the previous
    // send is older than CONTROL_SEND_INTERVAL_MS, starving pointer and paint
    // work precisely while the data channel is backpressured.
    if (force || controlDirty) controlBackpressureSkipCount += 1;
    return;
  }
  if (!force && !controlDirty) return;
  if (!sendCameraState(control)) return;
  observeControlBufferedAmount();
}

function commitCameraTail() {
  if (document.visibilityState !== 'visible') return;
  // First enqueue the final cumulative state on the low-latency unordered
  // channel, even if its normal soft backpressure limit is active. Then send
  // exactly one higher sequence on the reliable ordered channel. The server's
  // epoch/sequence filter makes a delayed unordered packet harmless.
  flushControls(true, true);
  reliableCameraTailPending = true;
  flushReliableCameraTail();
}

function flushReliableCameraTail() {
  if (!reliableCameraTailPending || document.visibilityState !== 'visible') return;
  if (sendCameraState(config)) reliableCameraTailPending = false;
}

function clearPendingControls() {
  if (controlTimer) clearTimeout(controlTimer);
  controlTimer = 0;
  if (wheelTailTimer) clearTimeout(wheelTailTimer);
  wheelTailTimer = 0;
  reliableCameraTailPending = false;
  controlDirty = true;
}

function configureReceiverJitterBuffer(receiver) {
  receiverJitterBufferTargetApi = 'browser-default';
  if (JITTER_BUFFER_REQUEST.mode === 'browser') return;

  if ('jitterBufferTarget' in receiver) {
    try {
      receiver.jitterBufferTarget = JITTER_BUFFER_REQUEST.targetMs;
      receiverJitterBufferTargetApi = 'jitterBufferTarget';
    } catch {
      receiverJitterBufferTargetApi = 'jitterBufferTarget-rejected';
    }
    return;
  }

  if ('playoutDelayHint' in receiver) {
    try {
      receiver.playoutDelayHint = JITTER_BUFFER_REQUEST.targetMs / 1000;
      receiverJitterBufferTargetApi = 'playoutDelayHint';
    } catch {
      receiverJitterBufferTargetApi = 'playoutDelayHint-rejected';
    }
    return;
  }

  receiverJitterBufferTargetApi = 'unsupported';
}

function receiverPropertyReadbackMs(property, scale) {
  if (!mediaReceiver || !(property in mediaReceiver)) return null;
  try {
    const value = mediaReceiver[property];
    if (value === null || value === undefined) return null;
    const numeric = Number(value) * scale;
    return Number.isFinite(numeric) ? numeric : null;
  } catch {
    return null;
  }
}

function nullableStatsCounter(report, property) {
  const supported = report !== null && typeof report === 'object' && property in report;
  const value = supported ? Number(report[property]) : Number.NaN;
  return {
    supported,
    value: Number.isSafeInteger(value) && value >= 0 ? value : null,
  };
}

function nullableStatsString(report, property) {
  const supported = report !== null && typeof report === 'object' && property in report;
  return {
    supported,
    value: supported && typeof report[property] === 'string' ? report[property] : null,
  };
}

function nullableStatsBoolean(report, property) {
  const supported = report !== null && typeof report === 'object' && property in report;
  return {
    supported,
    value: supported && typeof report[property] === 'boolean' ? report[property] : null,
  };
}

function optionalReceiverStatsFields(inbound) {
  const framesDropped = nullableStatsCounter(inbound, 'framesDropped');
  const framesRendered = nullableStatsCounter(inbound, 'framesRendered');
  const decoderImplementation = nullableStatsString(inbound, 'decoderImplementation');
  const powerEfficientDecoder = nullableStatsBoolean(inbound, 'powerEfficientDecoder');
  return {
    frames_dropped_supported: framesDropped.supported,
    frames_dropped: framesDropped.value,
    frames_rendered_supported: framesRendered.supported,
    frames_rendered: framesRendered.value,
    decoder_implementation_supported: decoderImplementation.supported,
    decoder_implementation: decoderImplementation.value,
    power_efficient_decoder_supported: powerEfficientDecoder.supported,
    power_efficient_decoder: powerEfficientDecoder.value,
  };
}

function rendererClaimEligible() {
  if (document.visibilityState !== 'visible' || rendererBusy) return false;
  if (document.hasFocus()) return true;
  expireReloadReclaimGrant();
  return hasOwnedRenderer && !standbyUntilFocus;
}

function expireReloadReclaimGrant() {
  if (!reloadReclaimActive || Date.now() <= reloadReclaimExpiresAt) return;
  reloadReclaimActive = false;
  reloadReclaimExpiresAt = 0;
  if (!peer && !connecting) hasOwnedRenderer = false;
  if (!document.hasFocus()) standbyUntilFocus = true;
}

function resumeOwnershipForUser() {
  if (document.visibilityState !== 'visible') return;
  if (document.hasFocus()) {
    reloadReclaimActive = false;
    reloadReclaimExpiresAt = 0;
  }
  standbyUntilFocus = false;
  if (rendererBusy) {
    statusNode.textContent = 'REMOTE RENDERER · WAITING FOR RENDERER';
    pollRendererStatus(true);
    return;
  }
  if (!peer && !connecting) connect(true);
}

function playbackCursor() {
  const quality = video.getVideoPlaybackQuality?.();
  const totalFrames = Number(quality?.totalVideoFrames || 0);
  const droppedFrames = Number(quality?.droppedVideoFrames || 0);
  return {
    hasPlaybackQuality: Boolean(quality),
    displayedFrames: Math.max(0, totalFrames - droppedFrames),
    videoTime: Math.max(0, Number(video.currentTime || 0)),
  };
}

function resetOwnershipPlaybackProgress(now = performance.now()) {
  const cursor = playbackCursor();
  ownershipDecodedFrames = 0;
  ownershipSsrc = 0;
  ownershipDisplayedFrames = cursor.displayedFrames;
  ownershipVideoTime = cursor.videoTime;
  ownershipPresentationProgressAt = now;
}

function enterOwnershipStandby(generation, connection) {
  if (!sessionIsCurrent(generation, connection)) return false;
  hasOwnedRenderer = false;
  rendererBusy = false;
  reloadReclaimActive = false;
  reloadReclaimExpiresAt = 0;
  standbyUntilFocus = true;
  detachSession();
  errorNode.textContent = '';
  statusNode.textContent = 'REMOTE RENDERER · STANDBY · FOCUS TO CONNECT';
  return true;
}

function enterRendererBusy(generation, connection) {
  if (!sessionIsCurrent(generation, connection)) return false;
  expireReloadReclaimGrant();
  hasOwnedRenderer = reloadReclaimActive;
  rendererBusy = true;
  standbyUntilFocus = !document.hasFocus() && !reloadReclaimActive;
  retryNotBefore = 0;
  reconnectFailureStreak = 0;
  detachSession();
  errorNode.textContent = 'Another preview window owns the renderer';
  statusNode.textContent = 'REMOTE RENDERER · WAITING FOR RENDERER';
  return true;
}

function observePlaybackProgress(framesDecoded, ssrc, now, generation, connection) {
  if (!sessionIsCurrent(generation, connection)) return false;
  const decoded = Number(framesDecoded || 0);
  const currentSsrc = Number(ssrc || 0);
  const cursor = playbackCursor();
  const mediaGenerationChanged = ownershipSsrc > 0 && currentSsrc > 0
    && currentSsrc !== ownershipSsrc;
  const playbackClockReset = cursor.displayedFrames < ownershipDisplayedFrames
    || cursor.videoTime + 0.001 < ownershipVideoTime;
  if (mediaGenerationChanged || playbackClockReset) {
    ownershipDecodedFrames = 0;
    ownershipPresentationProgressAt = now;
  }
  const presentationAdvanced = cursor.hasPlaybackQuality
    ? cursor.displayedFrames > ownershipDisplayedFrames
    : cursor.videoTime > ownershipVideoTime + 0.001;
  if (presentationAdvanced) ownershipPresentationProgressAt = now;
  ownershipDisplayedFrames = cursor.displayedFrames;
  ownershipVideoTime = cursor.videoTime;
  if (currentSsrc > 0) ownershipSsrc = currentSsrc;

  const decodedAdvanced = Number.isFinite(decoded) && decoded > ownershipDecodedFrames;
  if (Number.isFinite(decoded) && decoded >= 0) ownershipDecodedFrames = decoded;
  if (!decodedAdvanced || !hasOwnedRenderer
      || connection.connectionState !== 'connected' || !mediaConnectedAt) return false;
  if (now - mediaConnectedAt < BACKGROUND_OWNER_GRACE_MS
      || now - ownershipPresentationProgressAt < BACKGROUND_PRESENTATION_STALL_MS) {
    return false;
  }

  if (!document.hasFocus()) return enterOwnershipStandby(generation, connection);
  scheduleRetry(
    mediaRetryError(
      'playback-stall-timeout',
      'WebRTC frames decode but the video playback clock is not progressing',
    ),
    generation,
    connection,
  );
  return true;
}

function resetMediaProgressWatchdog() {
  inboundFramesSeen = false;
  lastInboundFrames = 0;
  lastInboundSsrc = 0;
  lastInboundFrameProgressAt = 0;
  mediaConnectedAt = 0;
  mediaKeyframeRequestAt = 0;
  mediaRecoveryState = 'waiting-first-frame';
  mediaHealthySince = 0;
}

function deferMediaProgressWatchdog() {
  const now = performance.now();
  if (inboundFramesSeen) lastInboundFrameProgressAt = now;
  else if (mediaConnectedAt) mediaConnectedAt = now;
}

function sendMediaKeyframeRequest(reason, now, generation, connection) {
  if (mediaKeyframeRequestAt || !sessionIsCurrent(generation, connection)
      || !config || config.readyState !== 'open') return false;
  const requestId = mediaKeyframeRequestId + 1;
  try {
    config.send(JSON.stringify({
      type: 'keyframe-request',
      connection_generation: generation,
      active_ssrc: lastInboundSsrc,
      request_id: requestId,
      last_frames_received: lastInboundFrames,
      client_time_ms: Date.now(),
      reason,
    }));
  } catch {
    return false;
  }
  mediaKeyframeRequestId = requestId;
  mediaKeyframeRequestsSent += 1;
  mediaKeyframeRequestAt = now;
  mediaRecoveryState = 'recovery-requested';
  return true;
}

function mediaRetryError(reason, message) {
  const error = new Error(message);
  error.recoveryReason = reason;
  return error;
}

function observeMediaProgress(framesReceived, ssrc, now, generation, connection) {
  const currentFrames = framesReceived === null || framesReceived === undefined
    ? Number.NaN : Number(framesReceived);
  const currentSsrc = Number(ssrc || 0);
  const ssrcChanged = lastInboundSsrc > 0 && currentSsrc > 0
    && currentSsrc !== lastInboundSsrc;
  if (ssrcChanged) {
    // getStats counters are scoped to an SSRC. Treat a same-peer rollover as
    // a new media generation instead of comparing its zero-based frame count
    // with the previous stream and eventually declaring a false stall.
    inboundFramesSeen = false;
    lastInboundFrames = 0;
    lastInboundFrameProgressAt = 0;
    mediaConnectedAt = now;
    mediaKeyframeRequestAt = 0;
    mediaRecoveryState = 'waiting-first-frame';
    mediaHealthySince = 0;
  }
  if (Number.isFinite(currentSsrc) && currentSsrc >= 0) lastInboundSsrc = currentSsrc;
  if (Number.isFinite(currentFrames) && currentFrames >= 0) {
    if (!inboundFramesSeen && currentFrames > 0) {
      inboundFramesSeen = true;
      lastInboundFrames = currentFrames;
      lastInboundFrameProgressAt = now;
      mediaKeyframeRequestAt = 0;
      mediaRecoveryState = 'healthy';
      mediaHealthySince = now;
      return false;
    }
    if (inboundFramesSeen && currentFrames > lastInboundFrames) {
      lastInboundFrames = currentFrames;
      lastInboundFrameProgressAt = now;
      mediaKeyframeRequestAt = 0;
      mediaRecoveryState = 'healthy';
      if (!mediaHealthySince) mediaHealthySince = now;
      if (reconnectFailureStreak > 0
          && now - mediaHealthySince >= RECOVERY_BACKOFF_RESET_MS) {
        reconnectFailureStreak = 0;
        retryNotBefore = 0;
      }
      return false;
    }
  }
  if (document.visibilityState !== 'visible' || !document.hasFocus()
      || connection.connectionState !== 'connected') return false;

  if (!mediaConnectedAt) mediaConnectedAt = now;
  if (!inboundFramesSeen) {
    mediaHealthySince = 0;
    mediaRecoveryState = mediaKeyframeRequestAt
      ? 'recovery-requested' : 'waiting-first-frame';
    const firstFrameWait = now - mediaConnectedAt;
    if (firstFrameWait >= MEDIA_KEYFRAME_REQUEST_AFTER_MS && !mediaKeyframeRequestAt) {
      sendMediaKeyframeRequest('first-frame', now, generation, connection);
    }
    if (firstFrameWait >= MEDIA_FIRST_FRAME_TIMEOUT_MS
        && (!mediaKeyframeRequestAt
          || now - mediaKeyframeRequestAt >= MEDIA_KEYFRAME_REQUEST_GRACE_MS)) {
      scheduleRetry(
        mediaRetryError(
          'first-frame-timeout',
          `WebRTC received no video frame for ${Math.round(firstFrameWait)} ms`,
        ),
        generation,
        connection,
      );
      return true;
    }
    return false;
  }

  const stallAge = now - lastInboundFrameProgressAt;
  if (stallAge < MEDIA_KEYFRAME_REQUEST_AFTER_MS) return false;
  mediaHealthySince = 0;
  mediaRecoveryState = mediaKeyframeRequestAt ? 'recovery-requested' : 'stalled';
  if (!mediaKeyframeRequestAt) {
    sendMediaKeyframeRequest('frame-stall', now, generation, connection);
  }
  if (stallAge < MEDIA_PROGRESS_TIMEOUT_MS
      || (mediaKeyframeRequestAt
        && now - mediaKeyframeRequestAt < MEDIA_KEYFRAME_REQUEST_GRACE_MS)) return false;
  scheduleRetry(
    mediaRetryError(
      'media-stall-timeout',
      `WebRTC media stopped progressing for ${Math.round(stallAge)} ms`,
    ),
    generation,
    connection,
  );
  return true;
}

function resetCameraInput(sendImmediately = true) {
  if (wheelTailTimer) clearTimeout(wheelTailTimer);
  wheelTailTimer = 0;
  controlEpoch = controlEpoch >= 0xffff_fffe ? 1 : controlEpoch + 1;
  persistControlEpoch(controlEpoch);
  controlSequence = 0;
  cumulativeOrbitX = 0;
  cumulativeOrbitY = 0;
  cumulativeZoom = 0;
  controlDirty = true;
  reliableCameraTailPending = true;
  if (sendImmediately) {
    markInputPending();
    flushControls(true, true);
    flushReliableCameraTail();
  }
}

function sessionIsCurrent(generation, connection) {
  return generation === connectionGeneration
    && peer === connection
    && document.visibilityState === 'visible';
}

function detachSession() {
  connectionGeneration += 1;
  clearTimeout(retryTimer);
  retryTimer = 0;
  clearTimeout(disconnectedTimer);
  disconnectedTimer = 0;
  clearTimeout(connectionReadyTimer);
  connectionReadyTimer = 0;
  offerAbortController?.abort();
  offerAbortController = undefined;
  connecting = false;
  statsRequestGeneration += 1;
  statsPending = false;
  statsSnapshotInFlight = undefined;
  statsSnapshotPeer = undefined;
  statsTimeoutStreak = 0;
  clearPendingControls();
  const detachedPeer = peer;
  peer = undefined;
  control = undefined;
  config = undefined;
  mediaReceiver = undefined;
  resetMediaProgressWatchdog();
  // Closing the RTCPeerConnection owns remote-track shutdown. Calling load()
  // on a MediaStream-backed video element (which has no URL source), or
  // stopping its track separately, can leave Chromium in HAVE_NOTHING even
  // while the replacement receiver continues to decode frames.
  try {
    video.pause();
    video.srcObject = null;
    if (detachedPeer) mediaDetachCount += 1;
  } finally {
    detachedPeer?.close();
  }
}

function requireCurrentSession(generation, connection) {
  if (!sessionIsCurrent(generation, connection)) {
    throw new DOMException('Stale WebRTC negotiation', 'AbortError');
  }
}

async function connect(explicitUserActivation = false) {
  if (connecting) return;
  if (document.visibilityState !== 'visible') {
    statusNode.textContent = 'REMOTE RENDERER · STANDBY · FOCUS TO CONNECT';
    return;
  }
  if (rendererBusy) {
    statusNode.textContent = 'REMOTE RENDERER · WAITING FOR RENDERER';
    return;
  }
  if (!explicitUserActivation && !rendererClaimEligible()) {
    statusNode.textContent = 'REMOTE RENDERER · STANDBY · FOCUS TO CONNECT';
    return;
  }
  if (document.hasFocus()) standbyUntilFocus = false;
  const retryRemaining = retryNotBefore - performance.now();
  if (retryRemaining > 0) {
    statusNode.textContent = 'REMOTE RENDERER · RETRYING';
    armRetryTimer(retryRemaining, explicitUserActivation);
    return;
  }
  retryNotBefore = 0;

  detachSession();
  // Establish the session epoch and its cumulative origin before either data
  // channel can open. This makes channel open order irrelevant and persists a
  // monotonic epoch across a full page reload.
  resetCameraInput(false);
  const generation = connectionGeneration;
  connecting = true;
  let connection;
  let controlChannel;
  let configChannel;
  let abortController;
  try {
    connection = new RTCPeerConnection({ bundlePolicy: 'max-bundle' });
    controlChannel = connection.createDataChannel(
      'control', { ordered: false, maxRetransmits: 0 },
    );
    configChannel = connection.createDataChannel('config', { ordered: true });
    abortController = new AbortController();
    const videoTransceiver = connection.addTransceiver('video', { direction: 'recvonly' });
    mediaReceiver = videoTransceiver.receiver;
    configureReceiverJitterBuffer(videoTransceiver.receiver);
  } catch (error) {
    connection?.close();
    if (generation === connectionGeneration) scheduleRetry(error, generation);
    return;
  }
  peer = connection;
  control = controlChannel;
  config = configChannel;
  offerAbortController = abortController;
  resetPresentationStats();
  errorNode.textContent = '';
  statusNode.textContent = 'REMOTE RENDERER · CONNECTING';

  connection.ontrack = ({ track, receiver, streams }) => {
    if (!sessionIsCurrent(generation, connection)) {
      track.stop();
      return;
    }
    try {
      // Chromium accepts a pre-negotiation jitterBufferTarget assignment but
      // may not apply it to the native video receive stream created later.
      // Reapply after the receiver has an active track; the readback alone is
      // not proof that the underlying jitter buffer adopted the target.
      mediaReceiver = receiver;
      configureReceiverJitterBuffer(receiver);
      const eventStream = streams.find(
        (stream) => stream.getVideoTracks().includes(track),
      ) || streams[0];
      video.srcObject = eventStream || new MediaStream([track]);
      mediaAttachCount += 1;
      resetPresentationWindow();
      resetOwnershipPlaybackProgress();
      video.play().catch((error) => {
        if (sessionIsCurrent(generation, connection) && error.name !== 'AbortError') {
          scheduleRetry(
            new Error(`Video playback failed: ${error.message}`), generation, connection,
          );
        }
      });
    } catch (error) {
      track.stop();
      if (sessionIsCurrent(generation, connection)) {
        scheduleRetry(
          new Error(`Video track attach failed: ${error?.message || String(error)}`),
          generation,
          connection,
        );
      }
    }
  };
  connection.onconnectionstatechange = () => {
    if (!sessionIsCurrent(generation, connection)) return;
    if (connection.connectionState === 'failed' || connection.connectionState === 'closed') {
      scheduleRetry(new Error(`WebRTC ${connection.connectionState}`), generation, connection);
      return;
    }
    if (connection.connectionState === 'disconnected') {
      // Keep one recovery deadline across disconnected -> connecting. Clearing
      // it on the intermediate connecting state can leave a wedged peer alive
      // forever while the video is no longer progressing.
      if (!disconnectedTimer) {
        disconnectedTimer = setTimeout(() => {
          disconnectedTimer = 0;
          if (sessionIsCurrent(generation, connection)
              && connection.connectionState !== 'connected') {
            scheduleRetry(
              new Error(`WebRTC recovery timed out (${connection.connectionState})`),
              generation,
              connection,
            );
          }
        }, DISCONNECTED_GRACE_MS);
      }
      return;
    }
    if (connection.connectionState === 'connected') {
      clearTimeout(disconnectedTimer);
      disconnectedTimer = 0;
      clearTimeout(connectionReadyTimer);
      connectionReadyTimer = 0;
      if (!mediaConnectedAt) mediaConnectedAt = performance.now();
    }
  };
  const channelFailed = (channel, label, force = false) => {
    if (!sessionIsCurrent(generation, connection)) return;
    if (force || channel.readyState === 'closing' || channel.readyState === 'closed') {
      scheduleRetry(new Error(`${label} DataChannel closed`), generation, connection);
    }
  };
  controlChannel.onopen = () => {
    if (!sessionIsCurrent(generation, connection)) return;
    controlChannel.bufferedAmountLowThreshold = MAX_CONTROL_BUFFERED_BYTES / 2;
    controlChannel.onbufferedamountlow = scheduleControlFlush;
    controlDirty = true;
    flushControls(true, true);
    reliableCameraTailPending = true;
    flushReliableCameraTail();
    updateConnectionReady();
  };
  controlChannel.onclose = () => channelFailed(controlChannel, 'control');
  controlChannel.onerror = () => channelFailed(controlChannel, 'control', true);
  configChannel.onopen = () => {
    if (!sessionIsCurrent(generation, connection)) return;
    flushReliableCameraTail();
    updateConnectionReady();
  };
  configChannel.onclose = () => channelFailed(configChannel, 'config');
  configChannel.onerror = () => channelFailed(configChannel, 'config', true);

  // Keep one total setup deadline through the first CONNECTED transition.
  // setRemoteDescription succeeding only means signaling completed; a peer
  // that remains in CONNECTING forever must still be replaced.
  connectionReadyTimer = setTimeout(() => {
    connectionReadyTimer = 0;
    abortController.abort();
    if (sessionIsCurrent(generation, connection)) {
      scheduleRetry(new Error('WebRTC connection setup timed out'), generation, connection);
    }
  }, SIGNALING_TIMEOUT_MS);
  try {
    const offer = await connection.createOffer();
    requireCurrentSession(generation, connection);
    await connection.setLocalDescription(offer);
    requireCurrentSession(generation, connection);
    await waitForIce(connection, abortController.signal);
    requireCurrentSession(generation, connection);
    const response = await fetch('/offer', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      signal: abortController.signal,
      body: JSON.stringify({
        type: 'offer',
        sdp: connection.localDescription.sdp,
        client_build: document.documentElement.dataset.build,
        client_protocol: CLIENT_PROTOCOL,
      }),
    });
    requireCurrentSession(generation, connection);
    if (response.status === 409) {
      enterRendererBusy(generation, connection);
      return;
    }
    if (!response.ok) throw new Error(`Signaling failed: HTTP ${response.status}`);
    const answer = await response.json();
    requireCurrentSession(generation, connection);
    await connection.setRemoteDescription(answer);
    requireCurrentSession(generation, connection);
  } catch (error) {
    if (sessionIsCurrent(generation, connection)
        && error.name !== 'AbortError') {
      scheduleRetry(error, generation, connection);
    }
  } finally {
    if (generation === connectionGeneration && peer === connection) {
      connecting = false;
      if (offerAbortController === abortController) offerAbortController = undefined;
    } else {
      connection.close();
    }
  }
}

function resetPresentationStats() {
  presentationGeneration += 1;
  presentedFrames = 0;
  lastPresentedFrames = currentPresentedFrames();
  lastPresentedAt = 0;
  lastMetadataPresentedFrames = null;
  presentationIntervals = diagnosticWindow();
  presentationGapCount = 0;
  presentationTimingTotalSamples = 0;
  presentationTimingCensoredFrames = 0;
  videoFrameCallbacks = 0;
  videoFrameCallbackMissed = 0;
  videoFrameCallbackLead = diagnosticWindow();
  pendingInputAt = 0;
  pendingDragInput = false;
  pendingWheelInput = false;
  controlMessagesSent = 0;
  controlInputMessagesSent = 0;
  controlDragInputMessagesSent = 0;
  controlWheelInputMessagesSent = 0;
  controlInputToSendIntervals = diagnosticWindow();
  controlBufferedAmountMaxBytes = 0;
  controlBackpressureSkipCount = 0;
  animationFrames = 0;
  lastAnimationFrames = 0;
  lastAnimationFrameAt = 0;
  animationFrameIntervals = diagnosticWindow();
  captureToDisplayIntervals = diagnosticWindow();
  captureToReceiveIntervals = diagnosticWindow();
  receiveToDisplayIntervals = diagnosticWindow();
  frameProcessingIntervals = diagnosticWindow();
  lastJitterBufferSample = undefined;
  maximumObservedRttMs = 0;
  resetOwnershipPlaybackProgress();
  resetMediaProgressWatchdog();
  lastStatsAt = performance.now();
}

function resetPresentationWindow() {
  presentationGeneration += 1;
  lastPresentedFrames = currentPresentedFrames();
  lastPresentedAt = 0;
  lastMetadataPresentedFrames = null;
  presentationIntervals = diagnosticWindow();
  videoFrameCallbackLead = diagnosticWindow();
  controlInputToSendIntervals = diagnosticWindow();
  pendingInputAt = 0;
  pendingDragInput = false;
  pendingWheelInput = false;
  lastAnimationFrames = animationFrames;
  lastAnimationFrameAt = 0;
  animationFrameIntervals = diagnosticWindow();
  lastStatsAt = performance.now();
}

function currentPresentedFrames() {
  if (HAS_VIDEO_FRAME_CALLBACK) return presentedFrames;
  const quality = video.getVideoPlaybackQuality?.();
  return Math.max(0, (quality?.totalVideoFrames || 0) - (quality?.droppedVideoFrames || 0));
}

function waitForIce(connection, signal) {
  if (connection.iceGatheringState === 'complete') return Promise.resolve();
  if (signal.aborted) return Promise.reject(new DOMException('Aborted', 'AbortError'));
  return new Promise((resolve, reject) => {
    let timeout;
    const finish = (error) => {
      connection.removeEventListener('icegatheringstatechange', changed);
      signal.removeEventListener('abort', aborted);
      clearTimeout(timeout);
      if (error) reject(error); else resolve();
    };
    const changed = () => {
      if (connection.iceGatheringState === 'complete') finish();
    };
    const aborted = () => finish(new DOMException('Aborted', 'AbortError'));
    connection.addEventListener('icegatheringstatechange', changed);
    signal.addEventListener('abort', aborted, { once: true });
    timeout = setTimeout(finish, 3000);
  });
}

function showTelemetry(sample) {
  if (!HUD_ENABLED) return;
  const progress = sample.receiver_progress || {};
  const lines = [
    `REMOTE RENDERER · ${sample.state}`,
    `${sample.resolution[0]}×${sample.resolution[1]}  ${sample.fps.toFixed(1)} fps`,
    `render ${sample.render_ms.toFixed(2)} ms  media ${sample.encode_ms.toFixed(2)} ms`,
    `receive ${Number(progress.frames_received || 0).toLocaleString()} frames  ${Number(progress.packets_received || 0).toLocaleString()} packets`,
    `visible ${sample.visible.toLocaleString()} / active ${sample.active.toLocaleString()}`,
    'DMA-BUF → VA-API H.264 → WebRTC',
  ];
  if (DIAGNOSTICS_ENABLED
      && sample.browser?.telemetry_schema === RECEIVER_TELEMETRY_SCHEMA) {
    lines.splice(
      4,
      0,
      `present ${Number(sample.browser.presentation_fps || 0).toFixed(1)} fps  loss ${Number(sample.browser.packets_lost || 0).toLocaleString()}`,
    );
  }
  statusNode.textContent = lines.join('\n');
}

function updateConnectionReady() {
  if (control?.readyState === 'open' && config?.readyState === 'open') {
    hasOwnedRenderer = true;
    rendererBusy = false;
    reloadReclaimActive = false;
    reloadReclaimExpiresAt = 0;
    standbyUntilFocus = false;
    resetOwnershipPlaybackProgress();
    statusNode.textContent = 'REMOTE RENDERER · CONNECTED';
  }
}

function scheduleRetry(error, generation = connectionGeneration, connection = peer) {
  if (connection && !sessionIsCurrent(generation, connection)) return;
  reconnectFailureStreak += 1;
  reconnectCount += 1;
  const exponentialDelay = Math.min(
    1500 * (2 ** Math.max(0, reconnectFailureStreak - 1)),
    MAX_RECOVERY_BACKOFF_MS,
  );
  const retryDelay = Math.max(Number(error.retryDelay || 0), exponentialDelay);
  lastReconnectReason = String(error.recoveryReason || error.message || 'unknown');
  lastReconnectGeneration = generation;
  lastReconnectAtUnixMs = Date.now();
  lastRetryDelayMs = retryDelay;
  retryNotBefore = performance.now() + retryDelay;
  detachSession();
  errorNode.textContent = error.message;
  statusNode.textContent = 'REMOTE RENDERER · RETRYING';
  if (rendererClaimEligible()) armRetryTimer(retryDelay);
}

function armRetryTimer(delay, explicitUserActivation = false) {
  clearTimeout(retryTimer);
  retryTimer = setTimeout(() => {
    retryTimer = 0;
    connect(explicitUserActivation);
  }, Math.max(0, delay));
}

function pollRendererStatus(explicitUserActivation = false) {
  statusPollExplicitActivation ||= explicitUserActivation;
  if (statusPollInFlight) return statusPollInFlight;
  const activateRequested = statusPollExplicitActivation;
  statusPollExplicitActivation = false;
  statusPollInFlight = (async () => {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), STATUS_FETCH_TIMEOUT_MS);
    try {
      const response = await fetch('/status', {
        cache: 'no-store',
        signal: controller.signal,
      });
      if (!response.ok) return;
      const remote = await response.json();
      showTelemetry(remote);
      if (remote.client_build && remote.client_build !== document.documentElement.dataset.build) {
        armReloadReclaimGrant();
        location.reload();
        return;
      }
      expireReloadReclaimGrant();
      if (remote.state === 'WAITING_FOR_BROWSER'
          && !peer && !connecting
          && (retryTimer || rendererBusy || hasOwnedRenderer)
          && (activateRequested || document.hasFocus()
            || (hasOwnedRenderer && !standbyUntilFocus))
          && performance.now() >= retryNotBefore) {
        clearTimeout(retryTimer);
        retryTimer = 0;
        rendererBusy = false;
        connect(activateRequested);
      }
    } catch {
      // A timeout or service restart is handled by the WebRTC retry loop.
    } finally {
      clearTimeout(timeout);
      statusPollInFlight = undefined;
    }
  })();
  return statusPollInFlight;
}

// Keep receiver progress and the HTTP build/liveness poll off the same
// event-loop turn so they do not form a periodic control-plane burst.
setTimeout(() => {
  pollRendererStatus();
  setInterval(pollRendererStatus, 2000);
}, 500);
setTimeout(() => {
  sampleReceiverProgress();
  setInterval(sampleReceiverProgress, 1000);
}, 1000);

function getStatsSnapshot(statsPeer, generation) {
  if (statsSnapshotInFlight && statsSnapshotPeer === statsPeer) {
    return statsSnapshotInFlight;
  }
  let timeoutId;
  const timeout = new Promise((_, reject) => {
    timeoutId = setTimeout(() => {
      reject(mediaRetryError(
        'get-stats-timeout',
        `WebRTC getStats timed out after ${GET_STATS_TIMEOUT_MS} ms`,
      ));
    }, GET_STATS_TIMEOUT_MS);
  });
  const nativeStats = Promise.resolve().then(() => statsPeer.getStats());
  const request = Promise.race([nativeStats, timeout])
    .then((reports) => {
      if (sessionIsCurrent(generation, statsPeer)) statsTimeoutStreak = 0;
      return reports;
    })
    .catch((error) => {
      if (error?.recoveryReason === 'get-stats-timeout'
          && sessionIsCurrent(generation, statsPeer)) {
        statsTimeoutStreak += 1;
        if (statsTimeoutStreak >= GET_STATS_TIMEOUTS_BEFORE_RECONNECT) {
          scheduleRetry(error, generation, statsPeer);
        }
      }
      throw error;
    })
    .finally(() => {
      clearTimeout(timeoutId);
      if (statsSnapshotInFlight === request) {
        statsSnapshotInFlight = undefined;
        statsSnapshotPeer = undefined;
      }
    });
  statsSnapshotPeer = statsPeer;
  statsSnapshotInFlight = request;
  return request;
}

function receiverProgressPayload(inbound, sampleTimeMs, generation) {
  return {
    type: 'receiver-progress',
    progress_schema: RECEIVER_PROGRESS_SCHEMA,
    sample_time_ms: sampleTimeMs,
    connection_generation: generation,
    active_ssrc: Number(inbound.ssrc || 0),
    frames_received: Number(inbound.framesReceived || 0),
    packets_received: Number(inbound.packetsReceived || 0),
    pli_count: Number(inbound.pliCount || 0),
    fir_count: Number(inbound.firCount || 0),
  };
}

async function sampleReceiverProgress() {
  if (document.visibilityState !== 'visible') return;
  const statsPeer = peer;
  const statsConnectionGeneration = connectionGeneration;
  const telemetryChannel = config?.readyState === 'open' ? config : control;
  if (statsPending || !statsPeer) return;
  statsPending = true;
  const requestGeneration = ++statsRequestGeneration;
  const statsPresentationGeneration = presentationGeneration;
  try {
    const reports = await getStatsSnapshot(statsPeer, statsConnectionGeneration);
    if (!sessionIsCurrent(statsConnectionGeneration, statsPeer)
        || statsPresentationGeneration !== presentationGeneration) return;
    let selectedInbound;
    for (const report of reports.values()) {
      if (report.type === 'inbound-rtp' && !report.isRemote
          && (report.kind === 'video' || report.mediaType === 'video')
          && (!selectedInbound
            || (report.bytesReceived || 0) > (selectedInbound.bytesReceived || 0))) {
        selectedInbound = report;
      }
    }
    const inbound = selectedInbound || {};
    const now = performance.now();
    if (observePlaybackProgress(
      inbound.framesDecoded,
      inbound.ssrc,
      now,
      statsConnectionGeneration,
      statsPeer,
    )) return;
    if (!selectedInbound) {
      observeMediaProgress(null, 0, now, statsConnectionGeneration, statsPeer);
    } else {
      observeMediaProgress(
        inbound.framesReceived || 0,
        inbound.ssrc || 0,
        now,
        statsConnectionGeneration,
        statsPeer,
      );
    }
    if (!sessionIsCurrent(statsConnectionGeneration, statsPeer)
        || !telemetryChannel
        || telemetryChannel.readyState !== 'open'
        || telemetryChannel.bufferedAmount > MAX_CONTROL_BUFFERED_BYTES) return;
    telemetryChannel.send(JSON.stringify(receiverProgressPayload(
      inbound,
      now,
      statsConnectionGeneration,
    )));
    if (!DIAGNOSTICS_ENABLED) return;

    const nominatedCandidatePairs = [];
    for (const report of reports.values()) {
      if (report.type === 'candidate-pair' && report.state === 'succeeded' && report.nominated) {
        nominatedCandidatePairs.push(report);
      }
    }
    const transport = inbound.transportId ? reports.get(inbound.transportId) : undefined;
    let candidatePair = transport?.selectedCandidatePairId
      ? reports.get(transport.selectedCandidatePairId) : undefined;
    if (!candidatePair) {
      candidatePair = nominatedCandidatePairs.sort(
        (a, b) => ((b.bytesReceived || 0) + (b.bytesSent || 0))
          - ((a.bytesReceived || 0) + (a.bytesSent || 0)),
      )[0];
    }
    const localCandidate = candidatePair?.localCandidateId
      ? reports.get(candidatePair.localCandidateId) : undefined;
    const remoteCandidate = candidatePair?.remoteCandidateId
      ? reports.get(candidatePair.remoteCandidateId) : undefined;
    const emitted = inbound.jitterBufferEmittedCount || 0;
    const jitterDelayTotalMs = 1000 * (inbound.jitterBufferDelay || 0);
    const jitterTargetDelayTotalMs = 1000 * (inbound.jitterBufferTargetDelay || 0);
    const jitterMinimumDelayTotalMs = 1000 * (inbound.jitterBufferMinimumDelay || 0);
    const jitterMs = emitted > 0 ? jitterDelayTotalMs / emitted : 0;
    const targetJitterMs = emitted > 0
      ? jitterTargetDelayTotalMs / emitted : 0;
    const minimumJitterMs = emitted > 0
      ? jitterMinimumDelayTotalMs / emitted : 0;
    const ssrc = Number(inbound.ssrc || 0);
    let intervalJitterMs = 0;
    let intervalTargetJitterMs = 0;
    let intervalMinimumJitterMs = 0;
    if (lastJitterBufferSample
        && lastJitterBufferSample.generation === statsConnectionGeneration
        && lastJitterBufferSample.ssrc === ssrc
        && emitted > lastJitterBufferSample.emitted) {
      const emittedDelta = emitted - lastJitterBufferSample.emitted;
      intervalJitterMs = Math.max(
        0, (jitterDelayTotalMs - lastJitterBufferSample.delayTotalMs) / emittedDelta,
      );
      intervalTargetJitterMs = Math.max(
        0, (jitterTargetDelayTotalMs - lastJitterBufferSample.targetTotalMs) / emittedDelta,
      );
      intervalMinimumJitterMs = Math.max(
        0, (jitterMinimumDelayTotalMs - lastJitterBufferSample.minimumTotalMs) / emittedDelta,
      );
    }
    lastJitterBufferSample = {
      generation: statsConnectionGeneration,
      ssrc,
      emitted,
      delayTotalMs: jitterDelayTotalMs,
      targetTotalMs: jitterTargetDelayTotalMs,
      minimumTotalMs: jitterMinimumDelayTotalMs,
    };
    const rttMs = 1000 * (candidatePair?.currentRoundTripTime || 0);
    maximumObservedRttMs = Math.max(maximumObservedRttMs, rttMs);
    const statsSeconds = Math.max((now - lastStatsAt) / 1000, 1e-3);
    const quality = video.getVideoPlaybackQuality?.();
    const currentPresented = currentPresentedFrames();
    const presentationFps = (currentPresented - lastPresentedFrames) / statsSeconds;
    const intervals = [...presentationIntervals].sort((a, b) => a - b);
    const p99Index = Math.max(0, Math.ceil(intervals.length * 0.99) - 1);
    const callbackLead = [...videoFrameCallbackLead].sort((a, b) => a - b);
    const callbackP01Index = Math.max(0, Math.floor(callbackLead.length * 0.01));
    const controlLatency = [...controlInputToSendIntervals].sort((a, b) => a - b);
    const controlP99Index = Math.max(0, Math.ceil(controlLatency.length * 0.99) - 1);
    const animationIntervals = HAS_ANIMATION_FRAME_PROBE
      ? [...animationFrameIntervals].sort((a, b) => a - b) : [];
    const animationP99Index = Math.max(0, Math.ceil(animationIntervals.length * 0.99) - 1);
    const captureDisplay = [...captureToDisplayIntervals].sort((a, b) => a - b);
    const captureReceive = [...captureToReceiveIntervals].sort((a, b) => a - b);
    const receiveDisplay = [...receiveToDisplayIntervals].sort((a, b) => a - b);
    const frameProcessing = [...frameProcessingIntervals].sort((a, b) => a - b);
    const optionalInboundStats = optionalReceiverStatsFields(inbound);
    const p50Index = (values) => Math.max(0, Math.ceil(values.length * 0.50) - 1);
    const p99TimingIndex = (values) => Math.max(0, Math.ceil(values.length * 0.99) - 1);
    if (!sessionIsCurrent(statsConnectionGeneration, statsPeer)
        || !telemetryChannel
        || telemetryChannel.readyState !== 'open'
        || telemetryChannel.bufferedAmount > MAX_CONTROL_BUFFERED_BYTES) return;
    telemetryChannel.send(JSON.stringify({
      type: 'receiver-stats',
      telemetry_schema: RECEIVER_TELEMETRY_SCHEMA,
      preview_owner_id: PREVIEW_OWNER_ID,
      stats_sample_time_ms: now,
      frames_received: inbound.framesReceived || 0,
      frames_decoded: inbound.framesDecoded || 0,
      frames_dropped_supported: optionalInboundStats.frames_dropped_supported,
      frames_dropped: optionalInboundStats.frames_dropped,
      key_frames_decoded: inbound.keyFramesDecoded || 0,
      packets_received: inbound.packetsReceived || 0,
      packets_lost: inbound.packetsLost || 0,
      bytes_received: inbound.bytesReceived || 0,
      retransmitted_packets_received: inbound.retransmittedPacketsReceived || 0,
      retransmitted_bytes_received: inbound.retransmittedBytesReceived || 0,
      jitter_buffer_delay_ms: jitterMs,
      jitter_buffer_target_delay_ms: targetJitterMs,
      jitter_buffer_minimum_delay_ms: minimumJitterMs,
      jitter_buffer_delay_interval_ms: intervalJitterMs,
      jitter_buffer_target_delay_interval_ms: intervalTargetJitterMs,
      jitter_buffer_minimum_delay_interval_ms: intervalMinimumJitterMs,
      jitter_buffer_emitted_count: emitted,
      jitter_buffer_delay_total_ms: jitterDelayTotalMs,
      jitter_buffer_target_delay_total_ms: jitterTargetDelayTotalMs,
      jitter_buffer_minimum_delay_total_ms: jitterMinimumDelayTotalMs,
      receiver_jitter_buffer_target_mode: JITTER_BUFFER_REQUEST.mode,
      receiver_jitter_buffer_target_ms: JITTER_BUFFER_REQUEST.targetMs,
      receiver_jitter_buffer_target_api: receiverJitterBufferTargetApi,
      receiver_jitter_buffer_target_readback_ms:
        receiverPropertyReadbackMs('jitterBufferTarget', 1),
      receiver_playout_delay_hint_readback_ms:
        receiverPropertyReadbackMs('playoutDelayHint', 1000),
      rtt_ms: rttMs,
      rtt_max_ms: maximumObservedRttMs,
      selected_candidate_pair_id: String(candidatePair?.id || ''),
      selected_candidate_pair_available_incoming_bitrate_bps:
        Number(candidatePair?.availableIncomingBitrate || 0),
      selected_candidate_pair_available_outgoing_bitrate_bps:
        Number(candidatePair?.availableOutgoingBitrate || 0),
      selected_candidate_pair_bytes_sent: Number(candidatePair?.bytesSent || 0),
      selected_candidate_pair_bytes_received: Number(candidatePair?.bytesReceived || 0),
      local_candidate_type: String(localCandidate?.candidateType || ''),
      local_candidate_protocol: String(localCandidate?.protocol || ''),
      local_candidate_address: String(localCandidate?.address || localCandidate?.ip || ''),
      local_candidate_port: Number(localCandidate?.port || 0),
      remote_candidate_type: String(remoteCandidate?.candidateType || ''),
      remote_candidate_protocol: String(remoteCandidate?.protocol || ''),
      remote_candidate_address: String(remoteCandidate?.address || remoteCandidate?.ip || ''),
      remote_candidate_port: Number(remoteCandidate?.port || 0),
      nack_count: inbound.nackCount || 0,
      pli_count: inbound.pliCount || 0,
      fir_count: inbound.firCount || 0,
      freeze_count: inbound.freezeCount || 0,
      total_freezes_duration_ms: 1000 * (inbound.totalFreezesDuration || 0),
      total_decode_time_ms: 1000 * (inbound.totalDecodeTime || 0),
      frames_rendered_supported: optionalInboundStats.frames_rendered_supported,
      frames_rendered: optionalInboundStats.frames_rendered,
      packets_discarded: inbound.packetsDiscarded || 0,
      total_processing_delay_ms: 1000 * (inbound.totalProcessingDelay || 0),
      total_inter_frame_delay_ms: 1000 * (inbound.totalInterFrameDelay || 0),
      decoder_implementation_supported:
        optionalInboundStats.decoder_implementation_supported,
      decoder_implementation: optionalInboundStats.decoder_implementation,
      power_efficient_decoder_supported:
        optionalInboundStats.power_efficient_decoder_supported,
      power_efficient_decoder: optionalInboundStats.power_efficient_decoder,
      presented_frames: currentPresented,
      presentation_probe_enabled: HAS_VIDEO_FRAME_CALLBACK,
      presentation_fps: presentationFps,
      presentation_p99_ms: intervals.length >= 120 ? intervals[p99Index] : 0,
      presentation_max_ms: intervals.length >= 120 ? intervals.at(-1) : 0,
      presentation_gaps_over_50ms: presentationGapCount,
      presentation_timing_samples: intervals.length,
      presentation_timing_total_samples: presentationTimingTotalSamples,
      presentation_timing_censored_frames: presentationTimingCensoredFrames,
      video_frame_callbacks: videoFrameCallbacks,
      video_frame_callback_missed: videoFrameCallbackMissed,
      video_frame_callback_lead_p01_ms: callbackLead[callbackP01Index] || 0,
      video_frame_callback_lead_min_ms: callbackLead[0] || 0,
      capture_to_display_samples: captureDisplay.length,
      capture_to_display_p50_ms: captureDisplay[p50Index(captureDisplay)] || 0,
      capture_to_display_p99_ms: captureDisplay[p99TimingIndex(captureDisplay)] || 0,
      capture_to_display_max_ms: captureDisplay.at(-1) || 0,
      capture_to_receive_p99_ms: captureReceive[p99TimingIndex(captureReceive)] || 0,
      receive_to_display_p99_ms: receiveDisplay[p99TimingIndex(receiveDisplay)] || 0,
      frame_processing_p99_ms: frameProcessing[p99TimingIndex(frameProcessing)] || 0,
      video_playback_total_frames: quality?.totalVideoFrames || 0,
      video_playback_dropped_frames: quality?.droppedVideoFrames || 0,
      control_messages_sent: controlMessagesSent,
      control_input_messages_sent: controlInputMessagesSent,
      control_drag_input_messages_sent: controlDragInputMessagesSent,
      control_wheel_input_messages_sent: controlWheelInputMessagesSent,
      control_input_to_send_p99_ms: controlLatency[controlP99Index] || 0,
      control_input_to_send_max_ms: controlLatency.at(-1) || 0,
      control_buffered_amount_max_bytes: controlBufferedAmountMaxBytes,
      control_backpressure_skip_count: controlBackpressureSkipCount,
      animation_probe_enabled: HAS_ANIMATION_FRAME_PROBE,
      animation_frame_fps: (animationFrames - lastAnimationFrames) / statsSeconds,
      animation_frame_p99_ms: animationIntervals[animationP99Index] || 0,
      animation_frame_max_ms: animationIntervals.at(-1) || 0,
      long_task_probe_enabled: longTaskProbeEnabled,
      long_task_count: longTaskCount,
      long_task_total_ms: longTaskTotalMs,
      long_task_max_ms: longTaskMaxMs,
      page_visible: document.visibilityState === 'visible',
      page_focused: document.hasFocus(),
      connection_generation: statsConnectionGeneration,
      page_visibility_transition_count: pageVisibilityTransitionCount,
      page_focus_transition_count: pageFocusTransitionCount,
      active_ssrc: ssrc,
      width: inbound.frameWidth || video.videoWidth || 0,
      height: inbound.frameHeight || video.videoHeight || 0,
      viewport_width: window.innerWidth,
      viewport_height: window.innerHeight,
      device_pixel_ratio: window.devicePixelRatio || 1,
      video_client_width: video.clientWidth,
      video_client_height: video.clientHeight,
      video_paused: video.paused,
      video_ready_state: video.readyState,
      video_current_time_s: Number(video.currentTime || 0),
      video_live_track_count: video.srcObject?.getVideoTracks?.()
        .filter((track) => track.readyState === 'live').length || 0,
      video_track_ready_state: String(video.srcObject?.getVideoTracks?.()[0]?.readyState || ''),
      video_track_muted: Boolean(video.srcObject?.getVideoTracks?.()[0]?.muted),
      media_attach_count: mediaAttachCount,
      media_detach_count: mediaDetachCount,
      media_recovery_state: mediaRecoveryState,
      media_stall_age_ms: inboundFramesSeen && lastInboundFrameProgressAt
        ? Math.max(0, now - lastInboundFrameProgressAt) : 0,
      media_first_frame_wait_ms: !inboundFramesSeen && mediaConnectedAt
        ? Math.max(0, now - mediaConnectedAt) : 0,
      media_keyframe_requests_sent: mediaKeyframeRequestsSent,
      media_keyframe_request_id: mediaKeyframeRequestId,
      media_keyframe_request_age_ms: mediaKeyframeRequestAt
        ? Math.max(0, now - mediaKeyframeRequestAt) : 0,
      reconnect_count: reconnectCount,
      reconnect_failure_streak: reconnectFailureStreak,
      last_reconnect_reason: lastReconnectReason,
      last_reconnect_generation: lastReconnectGeneration,
      last_reconnect_at_unix_ms: lastReconnectAtUnixMs,
      last_retry_delay_ms: lastRetryDelayMs,
    }));
    lastPresentedFrames = currentPresented;
    lastAnimationFrames = animationFrames;
    lastStatsAt = now;
  } catch {
    // Renderer telemetry remains available if a browser omits a WebRTC stats field.
  } finally {
    if (requestGeneration === statsRequestGeneration) statsPending = false;
  }
}

function observePresentedFrame() {
  if (!HAS_VIDEO_FRAME_CALLBACK) return;
  video.requestVideoFrameCallback((now, metadata) => {
    // This signal is supplementary only. The ownership watchdog always samples
    // currentTime and playback quality, so its behavior is identical when the
    // diagnostics-only callback probe is disabled.
    if (peer) ownershipPresentationProgressAt = performance.now();
    const foreground = document.visibilityState === 'visible' && document.hasFocus();
    const metadataFrames = Number(metadata.presentedFrames || 0);
    const hasMetadataBaseline = lastMetadataPresentedFrames !== null;
    let frameDelta = hasMetadataBaseline
      ? metadataFrames - lastMetadataPresentedFrames : 1;
    if (!Number.isFinite(frameDelta) || frameDelta < 1) frameDelta = 1;
    const displayAt = Number(metadata.expectedDisplayTime || metadata.presentationTime || now);
    const presentationAt = Number(metadata.presentationTime || displayAt);
    const captureAt = Number(metadata.captureTime);
    const receiveAt = Number(metadata.receiveTime);
    const processingMs = Number(metadata.processingDuration) * 1000;
    if (foreground && hasMetadataBaseline && lastPresentedAt > 0
        && presentationAt >= lastPresentedAt) {
      if (frameDelta === 1) {
        const frameInterval = presentationAt - lastPresentedAt;
        presentationIntervals.push(frameInterval);
        presentationTimingTotalSamples += 1;
        if (frameInterval > 50) presentationGapCount += 1;
      } else {
        // The browser may omit main-thread callbacks even though all of these
        // frames reached the compositor. Their individual intervals cannot be
        // reconstructed, so keep them out of the tail distribution.
        presentationTimingCensoredFrames += frameDelta;
      }
    }
    if (foreground) {
      videoFrameCallbackLead.push(displayAt - now);
      if (Number.isFinite(captureAt) && displayAt >= captureAt) {
        captureToDisplayIntervals.push(displayAt - captureAt);
      }
      if (Number.isFinite(captureAt) && Number.isFinite(receiveAt) && receiveAt >= captureAt) {
        captureToReceiveIntervals.push(receiveAt - captureAt);
      }
      if (Number.isFinite(receiveAt) && displayAt >= receiveAt) {
        receiveToDisplayIntervals.push(displayAt - receiveAt);
      }
      if (Number.isFinite(processingMs) && processingMs >= 0) {
        frameProcessingIntervals.push(processingMs);
      }
    }
    lastPresentedAt = foreground ? presentationAt : 0;
    lastMetadataPresentedFrames = metadataFrames;
    if (foreground) {
      presentedFrames += frameDelta;
      videoFrameCallbacks += 1;
      videoFrameCallbackMissed += Math.max(0, frameDelta - 1);
    }
    observePresentedFrame();
  });
}

function observeAnimationFrame(now) {
  if (!HAS_ANIMATION_FRAME_PROBE) return;
  const foreground = document.visibilityState === 'visible' && document.hasFocus();
  if (foreground && lastAnimationFrameAt > 0) {
    animationFrameIntervals.push(now - lastAnimationFrameAt);
  }
  lastAnimationFrameAt = foreground ? now : 0;
  animationFrames += 1;
  requestAnimationFrame(observeAnimationFrame);
}
