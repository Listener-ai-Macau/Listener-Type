import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { detectOS, type OS } from './WindowChrome';
import {
  getCapsuleHostMetrics,
  getCapsuleMessageLayout,
  getCapsulePillMetrics,
  getCapsuleProcessingTextMaxWidth,
} from '../lib/capsuleLayout';
import { invokeOrMock, isTauri } from '../lib/ipc';
import { capsuleCancelEnabled, capsuleConfirmEnabled } from '../lib/capsuleActionRules';
import { getCapsuleDisplayMessage, shouldRetainCapsulePreview } from '../lib/capsuleDisplayMessage';
import {
  buildPreviewRevealFrames,
  CAPSULE_APPEARANCE,
  previewRevealIntervalMs,
  truncatePreview,
  PREVIEW_FINAL_TRANSITION,
  shouldShowStopAcknowledgement,
} from '../lib/capsulePreviewRules';
import {
  applyCapsulePayloadOrdering,
  createCapsuleOrderingTracker,
} from '../lib/capsuleEventOrdering';
import type { CapsulePayload, CapsuleState } from '../lib/types';

interface AudioBarsProps {
  level: number;
}

function AudioBars({ level }: AudioBarsProps) {
  const envelope = [0.55, 0.85, 1.0, 0.85, 0.55];
  const base = 2;
  const max = 24;
  const voice = Math.min(1, Math.max(0, level));
  const silenceGate = 0.012;
  const responseCeiling = 0.34;
  const gatedVoice = Math.min(1, Math.max(0, (voice - silenceGate) / (responseCeiling - silenceGate)));
  const easedVoice = gatedVoice * gatedVoice * (3 - 2 * gatedVoice);
  const visualVoice = Math.pow(easedVoice, 0.42);
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 3,
        width: 42,
        height: max,
      }}
    >
      {envelope.map((env, i) => (
        <span
          key={i}
          style={{
            display: 'inline-block',
            width: 3,
            height: base + (max - base) * visualVoice * env,
            borderRadius: 999,
            background: 'var(--ol-blue)',
            opacity: 0.82,
            transformOrigin: 'center',
            transition: 'height 0.18s cubic-bezier(0.22, 1, 0.36, 1)',
          }}
        />
      ))}
    </div>
  );
}

interface CenterTextProps {
  os: OS;
  kind: 'default' | 'processing' | 'error';
  text: string;
  color?: string;
}

function compactCapsuleText(
  text: string,
  os: OS,
  kind: CenterTextProps['kind'],
  truncate: boolean,
): string {
  return truncate ? truncatePreview(text, os, kind) : text.replace(/\s+/g, ' ').trim();
}

function CenterText({ os, kind, text, color = 'var(--ol-ink-3)' }: CenterTextProps) {
  const metrics = getCapsulePillMetrics(os);
  const layout = getCapsuleMessageLayout(os, kind);
  const compactText = compactCapsuleText(text, os, kind, false);
  const lineHeight = layout.allowWrap ? 1.2 : 1;
  const fontSize = 11;
  return (
    <span
      style={{
        fontSize,
        fontWeight: 500,
        color,
        width: '100%',
        maxWidth: metrics.textWidth,
        minWidth: 0,
        flex: '0 1 auto',
        textAlign: 'center',
        lineHeight,
        maxHeight: fontSize * lineHeight * layout.lineClamp,
        whiteSpace: layout.allowWrap ? 'normal' : 'nowrap',
        overflow: 'hidden',
        textOverflow: 'ellipsis',
        overflowWrap: 'anywhere',
        wordBreak: 'break-word',
        display: '-webkit-box',
        WebkitBoxOrient: 'vertical',
        WebkitLineClamp: layout.lineClamp,
      }}
    >
      {compactText}
    </span>
  );
}

function CompletionMark({ label }: { label: string }) {
  return (
    <div
      aria-hidden="true"
      style={{
        minWidth: 0,
        height: 26,
        padding: '0 10px 0 8px',
        borderRadius: 999,
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 7,
        color: 'var(--ol-blue)',
        background: 'color-mix(in srgb, var(--ol-blue-soft) 74%, rgba(255,255,255,.72))',
        boxShadow: '0 0 0 0.5px rgba(101, 123, 112, 0.16) inset, 0 6px 14px -10px rgba(101, 123, 112, 0.42)',
        animation: 'cap-complete-chip 280ms var(--ol-motion-soft) both',
      }}
    >
      <span
        style={{
          width: 16,
          height: 16,
          borderRadius: 999,
          display: 'inline-flex',
          alignItems: 'center',
          justifyContent: 'center',
          flex: '0 0 auto',
          background: 'rgba(255,255,255,.72)',
          boxShadow: '0 0 0 0.5px rgba(101, 123, 112, 0.18) inset',
        }}
      >
        <svg width="11" height="11" viewBox="0 0 11 11">
          <path
            d="M2.3 5.8l2 2.1 4.4-4.8"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.7"
            strokeLinecap="round"
            strokeLinejoin="round"
            style={{
              strokeDasharray: 11,
              strokeDashoffset: 11,
              animation: 'cap-complete-check 200ms 40ms var(--ol-motion-soft) forwards',
            }}
          />
        </svg>
      </span>
      <span
        style={{
          minWidth: 0,
          fontSize: 11,
          fontWeight: 650,
          lineHeight: 1,
          letterSpacing: 0,
          whiteSpace: 'nowrap',
          overflow: 'hidden',
          textOverflow: 'ellipsis',
          color: '#171714',
          animation: 'cap-complete-label 200ms 60ms var(--ol-motion-soft) both',
        }}
      >
        {label}
      </span>
    </div>
  );
}

interface CircleButtonProps {
  variant: 'cancel' | 'confirm';
  enabled: boolean;
  subdued?: boolean;
  onClick: () => void;
}

function CircleButton({ variant, enabled, subdued = false, onClick }: CircleButtonProps) {
  const { t } = useTranslation();
  const isCancel = variant === 'cancel';
  // confirm 是主操作锚点，纯白；cancel 半透 + 自带 backdrop blur 跟 pill 拉开层级。
  const useBackdrop = isCancel;
  return (
    <button
      onClick={enabled ? onClick : undefined}
      aria-label={isCancel ? t('common.cancel') : t('settings.shortcuts.confirm')}
      disabled={!enabled}
      style={{
        width: 28,
        height: 28,
        borderRadius: 999,
        background: isCancel ? 'rgba(255, 255, 255, 0.55)' : 'rgba(255, 255, 255, 0.92)',
        backdropFilter: useBackdrop ? 'blur(12px) saturate(160%)' : 'none',
        WebkitBackdropFilter: useBackdrop ? 'blur(12px) saturate(160%)' : 'none',
        color: '#171714',
        border: '0.8px solid rgba(0, 0, 0, 0.08)',
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        cursor: enabled ? 'default' : 'not-allowed',
        opacity: subdued ? 0 : (enabled ? 1 : 0.42),
        visibility: 'visible',
        pointerEvents: subdued ? 'none' : 'auto',
        flexShrink: 0,
        padding: 0,
        boxShadow: '0 1px 2px rgba(0, 0, 0, 0.06)',
        transition: 'opacity 0.18s var(--ol-motion-soft), background 0.16s var(--ol-motion-quick), transform 0.12s var(--ol-motion-quick)',
      }}
    >
      {isCancel ? (
        <svg width="11" height="11" viewBox="0 0 11 11">
          <path d="M1.5 1.5l8 8M9.5 1.5l-8 8" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
        </svg>
      ) : (
        <svg width="13" height="13" viewBox="0 0 13 13">
          <path d="M2 6.5l3.2 3.5L11 3.5" stroke="currentColor" strokeWidth="1.7" fill="none" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      )}
    </button>
  );
}

interface PillProps {
  os: OS;
  state: CapsuleState;
  level: number;
  message?: string;
  stopRequested?: boolean;
  stopAcknowledged?: boolean;
  recordingBarsActive?: boolean;
  netBadge?: string | null;
  netBadgeTone?: 'warn' | 'bad';
  onCancel: () => void;
  onConfirm: () => void;
  onDismiss: () => void;
  onRetry: () => void;
}

function Pill({
  os,
  state,
  level,
  message,
  stopRequested = false,
  stopAcknowledged = false,
  recordingBarsActive = false,
  netBadge = null,
  netBadgeTone = 'warn',
  onCancel,
  onConfirm,
  onDismiss,
  onRetry,
}: PillProps) {
  const { t } = useTranslation();
  const metrics = getCapsulePillMetrics(os);
  const processingLayout = getCapsuleMessageLayout(os, 'processing');
  const processingTextMaxWidth = getCapsuleProcessingTextMaxWidth(os);
  const stopPending = state === 'recording' && stopRequested;
  const showStopAck = shouldShowStopAcknowledgement(state, stopPending || stopAcknowledged);
  const errorActive = state === 'error';
  const dismissOnly = errorActive || state === 'done' || state === 'cancelled';
  const controlsSubdued = state === 'done' || state === 'cancelled';
  const cancelEnabled = capsuleCancelEnabled(state);
  const confirmEnabled = capsuleConfirmEnabled(state, stopPending);

  // Apple-style: during transcribing/polishing the partial text preview stays
  // visible with a subtle pulse, plus a small spinner on the right — no overlay.

  let center: JSX.Element;
  const renderProcessingCenter = (displayText: string, truncate: boolean): JSX.Element => {
    const compactText = compactCapsuleText(displayText, os, 'processing', truncate);
    return (
      <div
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          gap: 5,
          width: '100%',
          maxWidth: metrics.textWidth,
          minWidth: 0,
          justifyContent: 'center',
          overflow: 'hidden',
          animation: showStopAck
            ? 'cap-stop-ack-center 420ms var(--ol-motion-soft) both'
            : 'cap-state-enter 220ms var(--ol-motion-soft) both',
        }}
      >
        <span
          style={{
            fontSize: 11,
            fontWeight: 500,
            color: '#171714',
            flex: '1 1 auto',
            minWidth: 0,
            maxWidth: processingTextMaxWidth,
            textAlign: 'center',
            lineHeight: processingLayout.allowWrap ? 1.2 : 1,
            whiteSpace: processingLayout.allowWrap ? 'normal' : 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            overflowWrap: 'anywhere',
            wordBreak: 'break-word',
            display: '-webkit-box',
            WebkitBoxOrient: 'vertical',
            WebkitLineClamp: processingLayout.lineClamp,
            // Apple-style: text stays static, only the spinner conveys "processing".
          }}
        >
          {compactText}
        </span>
        <svg
          width="12"
          height="12"
          viewBox="0 0 12 12"
          style={{ flexShrink: 0, animation: 'cap-spin 0.8s linear infinite' }}
        >
          <circle
            cx="6" cy="6" r="4.5"
            fill="none"
            stroke="var(--ol-blue)"
            strokeWidth="1.5"
            strokeDasharray="8 4"
            strokeLinecap="round"
          />
        </svg>
      </div>
    );
  };
  const renderRecordingPreview = (displayText: string): JSX.Element => {
    const compactText = truncatePreview(displayText, os, 'recording');
    return (
      <div
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          width: '100%',
          maxWidth: metrics.textWidth,
          minWidth: 0,
          justifyContent: 'center',
          overflow: 'hidden',
        }}
      >
        <span
          style={{
            fontSize: 11,
            fontWeight: 500,
            color: '#171714',
            flex: '1 1 100%',
            minWidth: 0,
            maxWidth: metrics.textWidth,
            textAlign: 'center',
            lineHeight: processingLayout.allowWrap ? 1.2 : 1,
            whiteSpace: processingLayout.allowWrap ? 'normal' : 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            overflowWrap: 'anywhere',
            wordBreak: 'break-word',
            display: '-webkit-box',
            WebkitBoxOrient: 'vertical',
            WebkitLineClamp: processingLayout.lineClamp,
          }}
        >
          {compactText}
        </span>
      </div>
    );
  };
  switch (state) {
    case 'reconnecting':
      center = renderProcessingCenter(message || t('capsule.thinking'), false);
      break;
    case 'recording':
      center = stopPending
        ? renderProcessingCenter(message || t('capsule.thinking'), Boolean(message))
        : message && !recordingBarsActive
          ? renderRecordingPreview(message)
          : netBadge
            // 网络掉级且暂无预览正文：中央直接显示"网络不佳/无网络"
            // （2026-09-23 用户拍板：胶囊里显示网络不佳就好）。有正文时
            // 保留正文，徽章在上方提示。
            ? <CenterText os={os} kind="error" text={netBadge} color={netBadgeTone === 'bad' ? 'var(--ol-err)' : '#B25E09'} />
            : <AudioBars level={level} />;
      break;
    case 'transcribing':
    case 'polishing': {
      const displayText = message || t('capsule.thinking');
      center = renderProcessingCenter(displayText, Boolean(message));
      break;
    }
    case 'done':
      center = message
        ? <CenterText os={os} kind="default" text={message} />
        : <CompletionMark label={t('capsule.inserted')} />;
      break;
    case 'cancelled':
      center = <CenterText os={os} kind="default" text={t('capsule.cancelled')} />;
      break;
    case 'error':
      center = <CenterText os={os} kind="error" text={message || t('capsule.error')} color="var(--ol-err)" />;
      break;
    default:
      center = <AudioBars level={0} />;
  }

  const ambient = state === 'recording' ? Math.min(1, Math.max(0, level)) : 0;
  const scale = os === 'win' ? 1 : 1 + ambient * 0.018;
  const shadowAlpha = 0.20 + ambient * 0.10;
  const useBackdrop = true;
  const stopAckRing = showStopAck
    ? '0 0 0 3px rgba(0, 122, 255, 0.16), '
    : '';

  return (
    <div
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        gap: 4,
        padding: '0 8px',
        width: metrics.width,
        height: metrics.height,
        boxSizing: metrics.boxSizing,
        borderRadius: 999,
        background: 'rgba(255, 255, 255, 0.85)',
        backdropFilter: useBackdrop ? 'blur(28px) saturate(180%)' : 'none',
        WebkitBackdropFilter: useBackdrop ? 'blur(28px) saturate(180%)' : 'none',
        border: '1px solid rgba(255, 255, 255, 0.55)',
        boxShadow: os === 'win'
          ? `${stopAckRing}0 10px 24px -14px rgba(0, 0, 0, ${(0.24 + ambient * 0.06).toFixed(3)}), 0 0 0 0.5px rgba(0, 0, 0, 0.08), inset 0 0.5px 0 rgba(255, 255, 255, 0.55)`
          : `${stopAckRing}0 18px 50px -10px rgba(0, 0, 0, ${shadowAlpha.toFixed(3)}), 0 0 0 0.5px rgba(0, 0, 0, 0.08), inset 0 0.5px 0 rgba(255, 255, 255, 0.55)`,
        color: 'var(--ol-ink)',
        fontFamily: 'var(--ol-font-sans)',
        transform: `scale(${scale.toFixed(4)})`,
        transformOrigin: 'center',
        transition: 'transform 0.08s var(--ol-motion-quick), box-shadow 0.12s var(--ol-motion-quick)',
        willChange: 'transform, box-shadow',
      }}
    >
      <CircleButton variant="cancel" enabled={cancelEnabled} subdued={controlsSubdued} onClick={dismissOnly ? onDismiss : onCancel} />
      <div style={{ flex: 1, minWidth: 0, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
        {center}
      </div>
      <CircleButton variant="confirm" enabled={confirmEnabled} subdued={controlsSubdued} onClick={errorActive ? onRetry : onConfirm} />
    </div>
  );
}

// 与 @keyframes capsule-out 的时长一致。BLE 听写的 final text 已在胶囊里预览，
// 退出动画只负责视觉收尾，避免文字落屏前出现一段空白等待。
const EXIT_ANIM_MS = PREVIEW_FINAL_TRANSITION.exitAnimMs;
const STOP_ACK_MS = PREVIEW_FINAL_TRANSITION.stopAckMs;
const DEV_CAPSULE_PREVIEW_MESSAGE = !isTauri && import.meta.env.DEV
  ? new URLSearchParams(window.location.search).get('preview')?.trim() || undefined
  : undefined;
const DISMISSED_NON_SESSION_SUPPRESS_MS = 13_000;
const ERROR_AUTO_DISMISS_MS = 2_500;
// 初始可见 state：Tauri 内运行从 idle 开始（等后端 capsule:state 事件），
// 浏览器 dev 模式从 recording 开始以便直接看到胶囊。
const INITIAL_VISIBLE_STATE: CapsuleState = isTauri ? 'idle' : 'recording';

function traceCapsule(
  event: string,
  payload: { state?: CapsuleState; elapsedMs?: number; detail?: Record<string, unknown> } = {},
) {
  if (!isTauri) return;
  void invokeOrMock<void>(
    'record_ui_timeline_event',
    {
      payload: {
        source: 'frontend.capsule',
        event,
        state: payload.state,
        elapsedMs: payload.elapsedMs,
        detail: payload.detail ?? {},
      },
    },
    () => undefined,
  ).catch(() => undefined);
}

export function Capsule() {
  const { t } = useTranslation();
  const os = detectOS();
  const metrics = getCapsulePillMetrics(os);
  const [state, setState] = useState<CapsuleState>(INITIAL_VISIBLE_STATE);
  const [level, setLevel] = useState<number>(isTauri ? 0 : 0.6);
  const [message, setMessage] = useState<string | undefined>(DEV_CAPSULE_PREVIEW_MESSAGE);
  const [translation, setTranslation] = useState<boolean>(false);
  // 网络状态徽章（2026-09-23）：后端 net-health 分级非 all_good 时在胶囊
  // 上方挂一枚橙/红小徽章。录音时用户盯的就是胶囊，断网要让他一眼看到；
  // 托盘图标变色只当辅助（Windows 11 常把托盘折叠进 ^ 隐藏区）。
  const [netBadge, setNetBadge] = useState<string | null>(null);
  const [netBadgeTone, setNetBadgeTone] = useState<'warn' | 'bad'>('warn');
  // `leaving` 与 `lastVisibleState` 协同实现「退出动画」：
  // - 当 state 从非 idle 变成 idle 时，不立即卸载，而是把 leaving 置为 true 并保留
  //   最后一帧的可见 state（lastVisibleState），让胶囊用 capsule-out 动画收缩淡出。
  // - 动画结束（EXIT_ANIM_MS）后再把 leaving 置回 false，组件回到「真正未挂载」分支。
  // - 若期间 state 又切回非 idle（例如用户连按热键），立刻中止 leaving 并恢复显示。
  const [leaving, setLeaving] = useState<boolean>(false);
  const [lastVisibleState, setLastVisibleState] = useState<CapsuleState>(INITIAL_VISIBLE_STATE);
  const previousStateRef = useRef<CapsuleState>(INITIAL_VISIBLE_STATE);
  const previousElapsedMsRef = useRef<number>(0);
  const messageSessionIdRef = useRef<string | null>(null);
  const messageRef = useRef<string | undefined>(DEV_CAPSULE_PREVIEW_MESSAGE);
  const previewTargetRef = useRef<string | undefined>(DEV_CAPSULE_PREVIEW_MESSAGE);
  const previewSessionIdRef = useRef<string | null>(null);
  const previewRevealFramesRef = useRef<string[]>([]);
  const previewRevealFrameRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const capsuleOrderingRef = useRef(createCapsuleOrderingTracker());
  const capsuleIngressTraceRef = useRef<{
    elapsedMs: number;
    sessionId: string | null | undefined;
    state: CapsuleState | null;
  }>({ elapsedMs: 0, sessionId: null, state: null });
  const suppressNonSessionEventsUntilRef = useRef<number>(0);
  const stopAckTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const errorAutoDismissTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [stopRequested, setStopRequested] = useState<boolean>(false);
  const [stopAcknowledged, setStopAcknowledged] = useState<boolean>(false);
  // 录音开始先显示音波：预览文本现在一秒内就会到，但 owner 验收过的视觉合同是
  // 「先音波、有正文再上字」，给音波一个短暂节拍再切换。
  const [recordingBarsActive, setRecordingBarsActive] = useState<boolean>(false);
  // Windows 端 host 在翻译模式从 84 长到 118；macOS / Linux 上 capsuleLayout 已固定 42 忽略此参数。
  const hostMetrics = getCapsuleHostMetrics(os, translation);

  const clearStopAcknowledgement = () => {
    if (stopAckTimerRef.current !== null) {
      clearTimeout(stopAckTimerRef.current);
      stopAckTimerRef.current = null;
    }
    setStopAcknowledged(false);
  };

  const armStopAcknowledgement = () => {
    if (stopAckTimerRef.current !== null) {
      clearTimeout(stopAckTimerRef.current);
    }
    setStopAcknowledged(true);
    stopAckTimerRef.current = setTimeout(() => {
      stopAckTimerRef.current = null;
      setStopAcknowledged(false);
    }, STOP_ACK_MS);
  };

  const clearErrorAutoDismiss = () => {
    if (errorAutoDismissTimerRef.current !== null) {
      clearTimeout(errorAutoDismissTimerRef.current);
      errorAutoDismissTimerRef.current = null;
    }
  };

  const commitMessage = (next: string | undefined) => {
    messageRef.current = next;
    setMessage(next);
  };

  const clearPreviewRevealFrame = () => {
    if (previewRevealFrameRef.current !== null) {
      clearTimeout(previewRevealFrameRef.current);
      previewRevealFrameRef.current = null;
    }
    previewRevealFramesRef.current = [];
  };

  const resetPreviewReveal = (flushTarget: boolean) => {
    clearPreviewRevealFrame();
    if (flushTarget && previewTargetRef.current !== undefined) {
      commitMessage(previewTargetRef.current);
    }
    previewTargetRef.current = undefined;
    previewSessionIdRef.current = null;
  };

  const schedulePreviewReveal = () => {
    const intervalMs = previewRevealIntervalMs(previewRevealFramesRef.current.length + 1);
    const revealNextFrame = () => {
      previewRevealFrameRef.current = null;
      const next = previewRevealFramesRef.current.shift();
      if (next !== undefined) commitMessage(next);
      if (previewRevealFramesRef.current.length > 0) {
        previewRevealFrameRef.current = setTimeout(revealNextFrame, intervalMs);
      }
    };
    if (previewRevealFramesRef.current.length > 0) {
      previewRevealFrameRef.current = setTimeout(revealNextFrame, intervalMs);
    }
  };

  const applyRecordingPreview = (target: string, sessionId: string | null) => {
    const sessionChanged = previewSessionIdRef.current !== sessionId;
    const current = messageRef.current;
    clearPreviewRevealFrame();
    previewTargetRef.current = target;
    previewSessionIdRef.current = sessionId;
    if (sessionChanged || !current) {
      commitMessage(target);
      return;
    }

    const frames = buildPreviewRevealFrames(current, target);
    if (frames.length === 1) {
      commitMessage(target);
      return;
    }
    const first = frames.shift();
    previewRevealFramesRef.current = frames;
    if (first !== undefined) commitMessage(first);
    schedulePreviewReveal();
  };

  const hideCapsuleLocally = () => {
    clearErrorAutoDismiss();
    const activeSessionId = capsuleOrderingRef.current.activeSessionId;
    if (activeSessionId) {
      capsuleOrderingRef.current.closedSessionStates.set(activeSessionId, 'idle');
      capsuleOrderingRef.current.activeSessionId = null;
    } else {
      suppressNonSessionEventsUntilRef.current = Date.now() + DISMISSED_NON_SESSION_SUPPRESS_MS;
    }
    setStopRequested(false);
    clearStopAcknowledgement();
    setState('idle');
    messageSessionIdRef.current = null;
    resetPreviewReveal(false);
    commitMessage(undefined);
    setTranslation(false);
  };

  useEffect(() => {
    if (!isTauri) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    (async () => {
      const { listen } = await import('@tauri-apps/api/event');
      const applyPayload = (p: CapsulePayload, replayed: boolean) => {
        if (p.sessionId) {
          suppressNonSessionEventsUntilRef.current = 0;
        } else if (p.state === 'idle') {
          suppressNonSessionEventsUntilRef.current = 0;
        } else if (suppressNonSessionEventsUntilRef.current > Date.now()) {
          traceCapsule('event_dropped_local_non_session_dismiss', {
            state: p.state,
            elapsedMs: p.elapsedMs,
            detail: {
              seq: p.seq,
              suppressUntil: suppressNonSessionEventsUntilRef.current,
            },
          });
          return;
        }
        const previousTrace = capsuleIngressTraceRef.current;
        const significantRecordingEvent =
          p.state !== 'recording'
          || Boolean(p.message)
          || p.insertedChars != null
          || previousTrace.state !== p.state
          || previousTrace.sessionId !== p.sessionId
          || p.elapsedMs < previousTrace.elapsedMs
          || p.elapsedMs - previousTrace.elapsedMs >= 1000;
        if (significantRecordingEvent) {
          capsuleIngressTraceRef.current = {
            elapsedMs: p.elapsedMs,
            sessionId: p.sessionId,
            state: p.state,
          };
          traceCapsule('event_received', {
            state: p.state,
            elapsedMs: p.elapsedMs,
            detail: {
              level: p.level,
              insertedChars: p.insertedChars ?? null,
              hasMessage: Boolean(p.message),
              translation: p.translation === true,
              seq: p.seq,
              sessionId: p.sessionId,
              replayed,
            },
          });
        }
        const ordering = applyCapsulePayloadOrdering(capsuleOrderingRef.current, p);
        if (!ordering.accepted) {
          traceCapsule('event_dropped_stale', {
            state: p.state,
            elapsedMs: p.elapsedMs,
            detail: {
              reason: ordering.reason,
              seq: p.seq,
              sessionId: p.sessionId,
            },
          });
          return;
        }
        const previousState = previousStateRef.current;
        const previousElapsedMs = previousElapsedMsRef.current;
        previousElapsedMsRef.current = p.elapsedMs;
        if (
          p.state === 'recording' &&
          (previousState !== 'recording' || p.elapsedMs < previousElapsedMs)
        ) {
          setStopRequested(false);
          clearStopAcknowledgement();
        }
        setState(p.state);
        if (p.state === 'idle') {
          resetPreviewReveal(true);
          setLevel(0);
          messageSessionIdRef.current = null;
          setTranslation(false);
          return;
        }
        setLevel(p.level ?? 0);
        const displayMessage = getCapsuleDisplayMessage(p.state, p.message);
        if (displayMessage) {
          messageSessionIdRef.current = p.sessionId ?? null;
          if (p.state === 'recording') {
            applyRecordingPreview(displayMessage, p.sessionId ?? null);
          } else {
            resetPreviewReveal(false);
            commitMessage(displayMessage);
          }
        } else if (!shouldRetainCapsulePreview({
          state: p.state,
          sessionId: p.sessionId ?? null,
          messageSessionId: messageSessionIdRef.current,
          currentMessage: messageRef.current,
        })) {
          messageSessionIdRef.current = null;
          resetPreviewReveal(false);
          commitMessage(undefined);
        }
        setTranslation(p.translation === true);
      };
      const handle = await listen<CapsulePayload>('capsule:state', event => {
        applyPayload(event.payload, false);
      });
      if (cancelled) handle();
      else {
        unlisten = handle;
        // Register the event listener first, then replay the latest backend
        // snapshot. Sequence ordering makes a live event racing this request
        // win over an older replay. This restores the capsule after a hidden
        // or recreated WebView without touching dictation or insertion.
        try {
          const latest = await invokeOrMock<CapsulePayload | null>(
            'get_capsule_state',
            undefined,
            () => null,
          );
          if (!cancelled && latest) {
            traceCapsule('snapshot_replayed', {
              state: latest.state,
              elapsedMs: latest.elapsedMs,
              detail: { seq: latest.seq, sessionId: latest.sessionId },
            });
            applyPayload(latest, true);
          }
        } catch (error) {
          traceCapsule('snapshot_replay_failed', {
            detail: { error: String(error) },
          });
        }
      }
    })();
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  // 网络状态徽章数据源：灯线程每次分级变化 + 每次巡检（60s）都重发，
  // 这里幂等收敛——all_good 隐藏徽章，其余按分级上色。
  useEffect(() => {
    if (!isTauri) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    (async () => {
      const { listen } = await import('@tauri-apps/api/event');
      const handle = await listen<{ class: string; badge: string }>('net-health:changed', event => {
        const cls = event.payload.class;
        if (cls === 'all_good' || !event.payload.badge) {
          setNetBadge(null);
          return;
        }
        setNetBadge(event.payload.badge);
        setNetBadgeTone(cls === 'all_bad' ? 'bad' : 'warn');
      });
      if (cancelled) handle();
      else unlisten = handle;
    })();
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  // Stop feedback: common voice UIs acknowledge the stop action immediately by
  // switching from waveform/recording to a processing affordance before final text.
  useEffect(() => {
    const previous = previousStateRef.current;
    previousStateRef.current = state;
    traceCapsule('state_applied', {
      state,
      detail: { previous, leaving, stopRequested, stopAcknowledged },
    });
    if (previous === 'recording' && (state === 'transcribing' || state === 'polishing')) {
      armStopAcknowledgement();
    }
    if (state !== 'recording') {
      setStopRequested(false);
    }
    if (!shouldShowStopAcknowledgement(state, true)) {
      clearStopAcknowledgement();
    }
    return undefined;
  }, [state]);

  useEffect(() => {
    if (state !== 'recording') {
      setRecordingBarsActive(false);
      return undefined;
    }
    setRecordingBarsActive(true);
    const timer = window.setTimeout(() => setRecordingBarsActive(false), 900);
    return () => window.clearTimeout(timer);
  }, [state]);

  useEffect(() => {
    clearErrorAutoDismiss();
    if (state !== 'error') {
      return undefined;
    }
    errorAutoDismissTimerRef.current = setTimeout(() => {
      errorAutoDismissTimerRef.current = null;
      traceCapsule('error_auto_dismiss', {
        state: 'error',
        detail: { errorAutoDismissMs: ERROR_AUTO_DISMISS_MS },
      });
      hideCapsuleLocally();
    }, ERROR_AUTO_DISMISS_MS);
    return clearErrorAutoDismiss;
    // state is the only trigger: message churn must not keep an error capsule
    // occupying the screen longer.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state]);

  useEffect(() => {
    return () => {
      if (stopAckTimerRef.current !== null) {
        clearTimeout(stopAckTimerRef.current);
      }
      clearPreviewRevealFrame();
      clearErrorAutoDismiss();
    };
  }, []);

  // 退出动画调度：在 state 真正进入 idle 时，先用 capsule-out 播放 EXIT_ANIM_MS，再卸载。
  // 设计要点：
  // 1. 进入非 idle：清掉 leaving，记录最新可见 state；
  // 2. 进入 idle 且之前可见：开启 leaving 并启动定时器；
  // 3. 期间又被打回非 idle：cleanup 直接 clearTimeout，定时器不会触发，
  //    新一轮 effect 会立即恢复可见态，避免错误地把可见状态切到 idle。
  useEffect(() => {
    if (state !== 'idle') {
      // 立即恢复可见，并取消上一轮可能挂着的离场。
      if (leaving) setLeaving(false);
      setLastVisibleState(state);
      traceCapsule('visible', {
        state,
        detail: {
          leaving,
          sessionId: capsuleOrderingRef.current.activeSessionId,
        },
      });
      return undefined;
    }
    // state === 'idle'：判断是不是从可见态过渡过来。
    if (lastVisibleState === 'idle') return undefined;
    setLeaving(true);
    traceCapsule('exit_animation_start', {
      state: lastVisibleState,
      detail: { exitAnimMs: EXIT_ANIM_MS },
    });
    const timer = setTimeout(() => {
      setLeaving(false);
      setLastVisibleState('idle');
      traceCapsule('exit_animation_end', { state: lastVisibleState });
    }, EXIT_ANIM_MS);
    return () => clearTimeout(timer);
    // 故意只依赖 state —— lastVisibleState / leaving 是内部派生量，
    // 把它们加进依赖会让定时器被反复重建。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state]);

  const onCancel = () => {
    traceCapsule('cancel_click', { state });
    void invokeOrMock<void>('cancel_dictation', undefined, () => undefined);
    hideCapsuleLocally();
  };

  const onConfirm = () => {
    traceCapsule('confirm_click', { state });
    if (state === 'recording') {
      setStopRequested(true);
      armStopAcknowledgement();
    }
    void invokeOrMock<void>('stop_dictation', undefined, () => undefined).catch(() => {
      setStopRequested(false);
      clearStopAcknowledgement();
    });
  };

  const onDismiss = () => {
    traceCapsule('dismiss_click', { state });
    hideCapsuleLocally();
  };

  const onRetry = () => {
    traceCapsule('retry_click', { state });
    hideCapsuleLocally();
    suppressNonSessionEventsUntilRef.current = 0;
    void invokeOrMock<void>('start_dictation', undefined, () => undefined);
  };

  // 真正卸载：state 已是 idle，且不在离场动画中。
  if (state === 'idle' && !leaving) {
    return <div style={{ width: 0, height: 0 }} />;
  }

  // 离场时用 lastVisibleState 渲染最后一帧内容，避免把 idle 当作 fallback 走到 AudioBars(0)。
  const renderedState: CapsuleState = state === 'idle' ? lastVisibleState : state;

  return (
    <div
      style={{
        width: '100%',
        height: '100%',
        position: 'relative',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        paddingLeft: hostMetrics.horizontalInset,
        paddingRight: hostMetrics.horizontalInset,
        boxSizing: hostMetrics.boxSizing,
        paddingTop: os === 'win'
          ? Math.max(0, hostMetrics.height - metrics.height - hostMetrics.bottomInset)
          : 0,
        paddingBottom: os === 'win' ? hostMetrics.bottomInset : 0,
        background: 'transparent',
        // 入场：中央 scaleX 由 0.18 长到 1（视觉上像从中心向两端展开）+ 淡入。
        // 离场：scaleX 由 1 收缩回 0.18 + 向下偏移 8px + 淡出。
        // 三平台一致 —— 旧版 Windows 走 animation:'none' 的分支已删除。
        // transformOrigin 默认就是 50% 50%，所以 scaleX 天然以中央为锚点。
        animation: leaving
          // 入场保留一点弹性；出场压短，让 final text 上屏和胶囊消失更贴近同一瞬间。
          ? `capsule-out ${EXIT_ANIM_MS}ms cubic-bezier(.55,.06,.68,.19) forwards`
          : `capsule-in ${CAPSULE_APPEARANCE.enterAnimMs}ms cubic-bezier(.16,.86,.32,1.18) both`,
        transformOrigin: 'center',
        willChange: 'transform, opacity',
      }}
    >
      {/* 网络状态徽章：断网/识别不可达时挂在胶囊上方（"正在翻译"徽章
          再往上叠一层，两者同现不重叠）。圆点呼吸闪烁表示持续状态。 */}
      {netBadge && (
        <div
          style={{
            position: 'absolute',
            left: '50%',
            bottom: os === 'win'
              ? `${hostMetrics.bottomInset + metrics.height + hostMetrics.badgeGap + (translation ? 26 : 0)}px`
              : `calc(50% + 21px + 8px + ${(translation ? 26 : 0)}px)`,
            transform: 'translateX(-50%)',
            pointerEvents: 'none',
            display: 'inline-flex',
            alignItems: 'center',
            gap: 5,
            padding: '3px 10px',
            borderRadius: 999,
            fontSize: 10.5,
            fontWeight: 600,
            color: netBadgeTone === 'bad' ? '#8C2F23' : '#8A5410',
            background: netBadgeTone === 'bad'
              ? 'rgba(255, 226, 218, 0.92)'
              : 'rgba(255, 231, 195, 0.92)',
            border: `0.5px solid ${netBadgeTone === 'bad' ? 'rgba(210, 74, 58, 0.4)' : 'rgba(232, 139, 30, 0.4)'}`,
            boxShadow: '0 4px 12px -4px rgba(120, 70, 20, 0.28), 0 0 0 0.5px rgba(0,0,0,0.04)',
            letterSpacing: 0,
            whiteSpace: 'nowrap',
            animation: 'cap-state-enter 220ms var(--ol-motion-soft) both',
          }}
        >
          <span
            style={{
              width: 5,
              height: 5,
              borderRadius: 999,
              background: netBadgeTone === 'bad' ? '#D24A3A' : '#E88B1E',
            }}
          />
          {netBadge}
        </div>
      )}
      {/* "正在翻译" 徽章 — 嵌套两层：
          外层只负责"绝对定位 + 水平居中（translateX(-50%)）"，不参与动画；
          内层只负责"垂直位移 + 渐变透明度"——这样不会跟 translateX(-50%) 冲突，
          也不存在 keyframe 与 inline transform 互相覆盖导致的视觉跳变。 */}
      <div
        style={{
          position: 'absolute',
          left: '50%',
          // macOS / Linux：胶囊窗口 220×110、pill 居中，badge 锚到 pill 中线上方 21+8。
          // Windows：host 比 pill 多出左右 12px / 底部 12px 的阴影空间，pill 仍保持居中。
          bottom: os === 'win'
            ? `${hostMetrics.bottomInset + metrics.height + hostMetrics.badgeGap}px`
            : 'calc(50% + 21px + 8px)',
          transform: 'translateX(-50%)',
          pointerEvents: 'none',
        }}
      >
        <div
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: 5,
            padding: '3px 10px',
            borderRadius: 999,
            fontSize: 10.5,
            fontWeight: 600,
            color: 'var(--ol-blue)',
            background: 'rgba(255, 255, 255, 0.78)',
            backdropFilter: 'blur(20px) saturate(180%)',
            WebkitBackdropFilter: 'blur(20px) saturate(180%)',
            border: '0.5px solid rgba(101, 123, 112, 0.25)',
            boxShadow: '0 4px 12px -4px rgba(101, 123, 112, 0.25), 0 0 0 0.5px rgba(0,0,0,0.04)',
            letterSpacing: 0,
            whiteSpace: 'nowrap',
            // 隐藏：从 pill 中线偏下出发；显示：归位到 wrapper（pill 上方 25px）
            opacity: translation ? 1 : 0,
            transform: translation ? 'translateY(0) scale(1)' : 'translateY(40px) scale(.88)',
            transformOrigin: 'center bottom',
            transition: 'opacity .24s ease-out, transform .34s cubic-bezier(.2,.9,.3,1.1)',
            willChange: 'opacity, transform',
          }}
        >
          <span style={{ width: 5, height: 5, borderRadius: 999, background: 'var(--ol-blue)' }} />
          {t('capsule.translating')}
        </div>
      </div>
      <Pill
        os={os}
        state={renderedState}
        level={leaving ? 0 : level}
        message={message}
        netBadge={netBadge}
        netBadgeTone={netBadgeTone}
        stopRequested={!leaving && renderedState === 'recording' && stopRequested}
        stopAcknowledged={!leaving && shouldShowStopAcknowledgement(renderedState, stopAcknowledged)}
        recordingBarsActive={!leaving && renderedState === 'recording' && recordingBarsActive}
        onCancel={onCancel}
        onConfirm={onConfirm}
        onDismiss={onDismiss}
        onRetry={onRetry}
      />
      <style>{`
        /* 入场首帧即完整可见，只让几何从中央快速展开。 */
        @keyframes capsule-in {
          /* Start almost settled: the first frame should already read as a full capsule. */
          from { opacity: ${CAPSULE_APPEARANCE.initialOpacity}; transform: scaleX(${CAPSULE_APPEARANCE.initialScaleX}) scaleY(.995); }
          to   { opacity: 1; transform: scaleX(1)   scaleY(1); }
        }
        /* 离场：scaleX 由 1 收回 0.18 + 整体向下偏移 8px + 淡出。
           forwards 让最终帧（opacity:0、scaleX:.18）保持到组件被卸载。 */
        @keyframes capsule-out {
          from { opacity: 1; transform: scaleX(1)   translateY(0); }
          to   { opacity: 0; transform: scaleX(.18) translateY(8px); }
        }
        @keyframes cap-spin {
          to { transform: rotate(360deg); }
        }
        @keyframes cap-state-enter {
          from { opacity: 0; transform: translateY(2px); }
          to   { opacity: 1; transform: translateY(0); }
        }
        @keyframes cap-stop-ack-center {
          0%   { opacity: 0; transform: translateY(4px) scale(.96); }
          45%  { opacity: 1; transform: translateY(0) scale(1.03); }
          100% { opacity: 1; transform: translateY(0) scale(1); }
        }
        @keyframes cap-complete-chip {
          0%   { opacity: 0; transform: translateY(2px) scale(.88); filter: saturate(.9); }
          58%  { opacity: 1; transform: translateY(0) scale(1.035); filter: saturate(1.08); }
          100% { opacity: 1; transform: translateY(0) scale(1); filter: saturate(1); }
        }
        @keyframes cap-complete-label {
          from { opacity: 0; transform: translateX(-3px); }
          to   { opacity: 1; transform: translateX(0); }
        }
        @keyframes cap-complete-check {
          to { stroke-dashoffset: 0; }
        }
      `}</style>
    </div>
  );
}
