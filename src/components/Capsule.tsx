import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { detectOS, type OS } from './WindowChrome';
import {
  getCapsuleHostMetrics,
  getCapsuleMessageLayout,
  getCapsulePillMetrics,
} from '../lib/capsuleLayout';
import { invokeOrMock, isTauri } from '../lib/ipc';
import { capsuleCancelEnabled, capsuleConfirmEnabled } from '../lib/capsuleActionRules';
import { getCapsuleDisplayMessage } from '../lib/capsuleDisplayMessage';
import {
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
            // 0.08s 在 60Hz audio-level 更新下太快，每次 re-render 都重启 transition，
            // 视觉上是阶梯式跳变。延长到 0.18s 让多次 update 在曲线内平滑混合，
            // easeOutExpo-like 缓动让圆点→长条的形变自然顺滑（用户原话"圆形跳成矩形"）。
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

function compactCapsuleText(text: string, os: OS, kind: CenterTextProps['kind']): string {
  return truncatePreview(text, os, kind);
}

function CenterText({ os, kind, text, color = 'var(--ol-ink-3)' }: CenterTextProps) {
  const metrics = getCapsulePillMetrics(os);
  const layout = getCapsuleMessageLayout(os, kind);
  const compactText = compactCapsuleText(text, os, kind);
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

interface CircleButtonProps {
  variant: 'cancel' | 'confirm';
  enabled: boolean;
  onClick: () => void;
}

function CircleButton({ variant, enabled, onClick }: CircleButtonProps) {
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
        opacity: enabled ? 1 : 0.42,
        visibility: 'visible',
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
  insertedChars: number;
  message?: string;
  stopRequested?: boolean;
  stopAcknowledged?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
  onDismiss: () => void;
  onRetry: () => void;
}

function Pill({
  os,
  state,
  level,
  insertedChars,
  message,
  stopRequested = false,
  stopAcknowledged = false,
  onCancel,
  onConfirm,
  onDismiss,
  onRetry,
}: PillProps) {
  const { t } = useTranslation();
  const metrics = getCapsulePillMetrics(os);
  const processingLayout = getCapsuleMessageLayout(os, 'processing');
  const stopPending = state === 'recording' && stopRequested;
  const showStopAck = shouldShowStopAcknowledgement(state, stopPending || stopAcknowledged);
  const errorActive = state === 'error';
  const dismissOnly = errorActive || state === 'done' || state === 'cancelled';
  const cancelEnabled = capsuleCancelEnabled(state);
  const confirmEnabled = capsuleConfirmEnabled(state, stopPending);

  // Apple-style: during transcribing/polishing the partial text preview stays
  // visible with a subtle pulse, plus a small spinner on the right — no overlay.

  let center: JSX.Element;
  const renderProcessingCenter = (displayText: string): JSX.Element => {
    const compactText = compactCapsuleText(displayText, os, 'processing');
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
            minWidth: 0,
            textAlign: 'center',
            lineHeight: processingLayout.allowWrap ? 1.2 : 1,
            whiteSpace: processingLayout.allowWrap ? 'normal' : 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
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
    const compactText = compactCapsuleText(displayText, os, 'processing');
    return (
      <div
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          width: '100%',
          maxWidth: metrics.textWidth,
          minWidth: 0,
          justifyContent: 'center',
        }}
      >
        <span
          style={{
            fontSize: 11,
            fontWeight: 500,
            color: '#171714',
            minWidth: 0,
            textAlign: 'center',
            lineHeight: processingLayout.allowWrap ? 1.2 : 1,
            whiteSpace: processingLayout.allowWrap ? 'normal' : 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
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
      center = renderProcessingCenter(message || t('capsule.thinking'));
      break;
    case 'recording':
      center = stopPending
        ? renderProcessingCenter(message || t('capsule.thinking'))
        : message
          ? renderRecordingPreview(message)
          : <AudioBars level={level} />;
      break;
    case 'transcribing':
    case 'polishing': {
      const displayText = message || t('capsule.thinking');
      center = renderProcessingCenter(displayText);
      break;
    }
    case 'done':
      center = <CenterText os={os} kind="default" text={message || t('capsule.inserted', { count: insertedChars })} />;
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
      <CircleButton variant="cancel" enabled={cancelEnabled} onClick={dismissOnly ? onDismiss : onCancel} />
      <div style={{ flex: 1, minWidth: 0, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
        {center}
      </div>
      <CircleButton variant="confirm" enabled={confirmEnabled} onClick={errorActive ? onRetry : onConfirm} />
    </div>
  );
}

// 与 @keyframes capsule-out 的时长一致。BLE 听写的 final text 已在胶囊里预览，
// 退出动画只负责视觉收尾，避免文字落屏前出现一段空白等待。
const EXIT_ANIM_MS = PREVIEW_FINAL_TRANSITION.exitAnimMs;
const STOP_ACK_MS = PREVIEW_FINAL_TRANSITION.stopAckMs;
const DISMISSED_NON_SESSION_SUPPRESS_MS = 13_000;
const ERROR_AUTO_DISMISS_MS = 2_500;
const STARTUP_MESSAGE_CARRYOVER_MS = 3_000;
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

function shouldPreserveMessageWithoutPayload(
  state: CapsuleState,
  sessionId: string | null,
  messageSessionId: string | null,
  previousState: CapsuleState,
  elapsedMs: number,
): boolean {
  if (state !== 'recording' && state !== 'transcribing' && state !== 'polishing') {
    return false;
  }
  if (sessionId && messageSessionId === sessionId) {
    return true;
  }
  return (
    messageSessionId === null &&
    previousState === 'recording' &&
    elapsedMs <= STARTUP_MESSAGE_CARRYOVER_MS
  );
}

export function Capsule() {
  const { t } = useTranslation();
  const os = detectOS();
  const metrics = getCapsulePillMetrics(os);
  const [state, setState] = useState<CapsuleState>(INITIAL_VISIBLE_STATE);
  const [level, setLevel] = useState<number>(isTauri ? 0 : 0.6);
  const [insertedChars, setInsertedChars] = useState<number>(0);
  const [message, setMessage] = useState<string | undefined>();
  const [translation, setTranslation] = useState<boolean>(false);
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
  const capsuleOrderingRef = useRef(createCapsuleOrderingTracker());
  const suppressNonSessionEventsUntilRef = useRef<number>(0);
  const stopAckTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const errorAutoDismissTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [stopRequested, setStopRequested] = useState<boolean>(false);
  const [stopAcknowledged, setStopAcknowledged] = useState<boolean>(false);
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
    setMessage(undefined);
    setTranslation(false);
  };

  useEffect(() => {
    if (!isTauri) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    (async () => {
      const { listen } = await import('@tauri-apps/api/event');
      const handle = await listen<CapsulePayload>('capsule:state', event => {
        const p = event.payload;
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
          },
        });
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
          setLevel(0);
          messageSessionIdRef.current = null;
          setTranslation(false);
          return;
        }
        setLevel(p.level ?? 0);
        const displayMessage = getCapsuleDisplayMessage(p.state, p.message);
        if (displayMessage) {
          messageSessionIdRef.current = p.sessionId ?? null;
          setMessage(displayMessage);
        } else if (p.message) {
          messageSessionIdRef.current = null;
          setMessage(undefined);
        } else if (!shouldPreserveMessageWithoutPayload(
          p.state,
          p.sessionId ?? null,
          messageSessionIdRef.current,
          previousState,
          p.elapsedMs,
        )) {
          messageSessionIdRef.current = null;
          setMessage(undefined);
        }
        if (p.insertedChars != null) setInsertedChars(p.insertedChars);
        setTranslation(p.translation === true);
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
      traceCapsule('visible', { state, detail: { leaving } });
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
          : 'capsule-in .38s cubic-bezier(.16,.86,.32,1.18) both',
        transformOrigin: 'center',
        willChange: 'transform, opacity',
      }}
    >
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
        insertedChars={insertedChars}
        message={message}
        stopRequested={!leaving && renderedState === 'recording' && stopRequested}
        stopAcknowledged={!leaving && shouldShowStopAcknowledgement(renderedState, stopAcknowledged)}
        onCancel={onCancel}
        onConfirm={onConfirm}
        onDismiss={onDismiss}
        onRetry={onRetry}
      />
      <style>{`
        /* 入场：从中央很窄的一小条（scaleX 0.18）+ 略压扁（scaleY 0.95）+ 透明，
           长出到 scaleX 1 / scaleY 1 / 不透明。配合 wrapper 的 transformOrigin:center，
           视觉上是「从中心向左右展开」。 */
        @keyframes capsule-in {
          from { opacity: 0; transform: scaleX(.18) scaleY(.95); }
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
      `}</style>
    </div>
  );
}
