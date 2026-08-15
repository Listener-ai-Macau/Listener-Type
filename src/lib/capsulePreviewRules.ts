/**
 * Capsule partial preview behavior rules.
 *
 * These constants define how partial ASR text is displayed in the capsule
 * during recording, ensuring stable layout and predictable UX across
 * all capsule states (recording → transcribing → polishing → done).
 *
 * Cross-repo contract: firmware matrix validates preview behavior using
 * these same thresholds via run_ble_stream_smoke.ps1 fields.
 */

// ── Truncation ──────────────────────────────────────────────

/** Max visible characters in the pill center text (OS + kind dependent). */
export const PREVIEW_MAX_CHARS: Record<string, {
  default: number;
  processing: number;
  recording: number;
  error: number;
}> = {
  // Processing shares a 175px two-line column with the spinner. Recording has
  // the full 192px column, so retain 32 tail characters to fill both CJK lines
  // more evenly without forcing an artificial line break.
  win: { default: 28, processing: 28, recording: 32, error: 28 },
  mac: { default: 14, processing: 18, recording: 18, error: 14 },
};

/**
 * Truncate preview text for display. Shows the *tail* of the text so the
 * user always sees the latest words (partial ASR grows from left to right).
 */
export function truncatePreview(
  text: string,
  os: string,
  kind: 'default' | 'processing' | 'recording' | 'error',
): string {
  const normalized = text.replace(/\s+/g, ' ').trim();
  const max = (PREVIEW_MAX_CHARS[os] ?? PREVIEW_MAX_CHARS.win)[kind];
  const chars = Array.from(normalized);
  if (chars.length <= max) return normalized;
  return `...${chars.slice(chars.length - max).join('')}`;
}

// ── Deduplication ───────────────────────────────────────────

/**
 * Before emitting a capsule:state event with a new preview message,
 * compare against the last emitted text. Skip if identical.
 * Implemented in Rust: update_embedded_audio_partial_preview().
 */
export const PREVIEW_DEDUP_POLICY = 'exact-match' as const;

/**
 * Provider partials can arrive as a larger pure append after a pause. Split
 * only that visual append into a bounded number of rendering frames.
 */
export const PREVIEW_BURST_REVEAL = {
  minimumAppendChars: 3,
  maxFrames: 8,
  maxCatchUpMs: 160,
} as const;

/** The wake capsule is visible immediately; only its geometry settles. */
export const CAPSULE_APPEARANCE = {
  // The backend-to-visible path is already about one frame on Windows. Keep
  // the remaining geometry settle inside a short response beat so a
  // confirmed wake reads as immediate instead of adding a second visual wait.
  enterAnimMs: 55,
  initialOpacity: 1,
  initialScaleX: 0.97,
} as const;

/**
 * Build the visual path between two authoritative previews. Rewrites and
 * routine short updates return one immediate frame.
 */
export function buildPreviewRevealFrames(current: string, target: string): string[] {
  if (!current || !target.startsWith(current)) return [target];

  const appended = Array.from(target.slice(current.length));
  if (appended.length < PREVIEW_BURST_REVEAL.minimumAppendChars) return [target];

  const frameCount = Math.min(PREVIEW_BURST_REVEAL.maxFrames, appended.length);
  const charsPerFrame = Math.ceil(appended.length / frameCount);
  const frames: string[] = [];
  for (let end = charsPerFrame; end < appended.length; end += charsPerFrame) {
    frames.push(current + appended.slice(0, end).join(''));
  }
  frames.push(target);
  return frames;
}

// ── Final transition ────────────────────────────────────────

/**
 * When ASR produces the final transcript, the capsule transitions:
 *   Recording(message=partialPreview) → Transcribing(message=partialPreview)
 *   → Polishing(message=partialPreview) → Done(successMark)
 *
 * On the Recording → Transcribing edge, the capsule shows a short stop
 * acknowledgement and enters the processing spinner as soon as the stop action
 * is accepted. The hardware AI LED follows that accepted-stop edge and remains
 * active through ASR/polish/insert work; host completion then drives the OK LED.
 * The preview text stays visible through transcribing/polishing so the user sees
 * continuity. Done state replaces it with a compact success mark; actionable
 * fallback messages may still show text.
 * After Done, the normal success path lingers ~380ms (schedule_capsule_idle) then
 * fades to Idle with EXIT_ANIM_MS = 120ms. Text is already on-screen; this is
 * only a brief visual close (owner: 结尾拖 was the old 1050ms hang).
 */
export const PREVIEW_FINAL_TRANSITION = {
  /** Capsule states that display partial preview text. */
  previewStates: ['recording', 'transcribing', 'polishing'] as const,
  /** Immediate visual feedback after the stop key/button is accepted. */
  stopAckMs: 620,
  /** Terminal state where preview is replaced by completion feedback. */
  finalState: 'done' as const,
  /** How long the normal success done toast stays visible before idle (ms). */
  lingerMs: 380,
  /** Exit animation duration (ms). */
  exitAnimMs: 120,
};

export type StopFeedbackCapsuleState =
  | 'idle'
  | 'reconnecting'
  | 'recording'
  | 'transcribing'
  | 'polishing'
  | 'done'
  | 'cancelled'
  | 'error';

export function shouldShowStopAcknowledgement(
  state: StopFeedbackCapsuleState,
  stopFeedbackActive: boolean,
): boolean {
  return stopFeedbackActive && PREVIEW_FINAL_TRANSITION.previewStates.includes(
    state as (typeof PREVIEW_FINAL_TRANSITION.previewStates)[number],
  );
}

// ── Layout stability ────────────────────────────────────────

/**
 * The pill height is fixed per OS (getCapsulePillMetrics). Preview text
 * is truncated to PREVIEW_MAX_CHARS and clamped to lineClamp lines, so
 * the pill never resizes during a session. Audio bars ↔ text transitions
 * happen within the same fixed-height container.
 */
export const LAYOUT_RULES = {
  /** Pill height is constant per OS — no resize on preview change. */
  fixedHeight: { win: 52, mac: 42 },
  /** Processing text allows 2-line wrap on Windows, 1-line elsewhere. */
  processingWrap: { win: { allowWrap: true, lineClamp: 2 }, default: { allowWrap: false, lineClamp: 1 } },
};
