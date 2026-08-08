// RecordingSection + 内嵌子组件，从 Settings.tsx 拆出。
// 包含：WaylandHotkeyCallout, HotkeyRecorder, MicrophonePickerDialog,
// normalizeKeyboardHotkeyCode, mouseButtonToHotkeyCode,
// LevelMeter, AutostartRow, autostartIsEnabled/Enable/Disable, RecordingSection。

import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { Icon } from '../../components/Icon';
import { ShortcutRecorder } from '../../components/ShortcutRecorder';
import {
  getHotkeyBindingCodes,
  getHotkeyBindingLabel,
  getHotkeyCodeLabel,
} from '../../lib/hotkey';
import { createHotkeyRecorderState, orderHotkeyCodes, updateHotkeyRecorderState } from '../../lib/hotkeyRecorder';
import {
  isTauri,
  isWaylandCliMode,
  listMicrophoneDevices,
  setDictationHotkey,
  startMicrophoneLevelMonitor,
  stopMicrophoneLevelMonitor,
} from '../../lib/ipc';
import { windowMouseHotkeyCode, SUPPORTED_KEYBOARD_HOTKEY_CODES } from '../../lib/windowHotkeyFallback';
import type {
  DictationInputSource,
  HotkeyBinding,
  MicrophoneDevice,
  PasteShortcut,
} from '../../lib/types';
import { useHotkeySettings } from '../../state/HotkeySettingsContext';
import { SelectLite } from '../../components/ui/SelectLite';
import { Card, Collapsible } from '../_atoms';
import { SettingRow, Toggle, inputStyle } from './shared';

// ─── autostart helpers（OS 持有状态，不存 prefs）──────────────────────

async function autostartIsEnabled(): Promise<boolean> {
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<boolean>('plugin:autostart|is_enabled');
}
async function autostartEnable(): Promise<void> {
  const { invoke } = await import('@tauri-apps/api/core');
  await invoke('plugin:autostart|enable');
}
async function autostartDisable(): Promise<void> {
  const { invoke } = await import('@tauri-apps/api/core');
  await invoke('plugin:autostart|disable');
}

// ─── hotkey utilities ────────────────────────────────────────────────

function normalizeKeyboardHotkeyCode(event: KeyboardEvent): string | null {
  if (event.key === 'Fn' || event.code === 'Fn') return 'Fn';
  if (event.key === 'FnLock' || event.code === 'FnLock') return 'FnLock';
  const code = event.code === 'OSLeft' ? 'MetaLeft' : event.code === 'OSRight' ? 'MetaRight' : event.code;
  if (SUPPORTED_KEYBOARD_HOTKEY_CODES.has(code)) return code;
  if (/^Key[A-Z]$/.test(code)) return code;
  if (/^Digit[0-9]$/.test(code)) return code;
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  if (/^Numpad[0-9]$/.test(code)) return code;
  return null;
}


// ─── shared styles ───────────────────────────────────────────────────

const recordingHotkeyControlWidth = 178;

const hotkeyRecorderButtonStyle: CSSProperties = {
  width: recordingHotkeyControlWidth,
  height: 32,
  padding: '0 8px 0 11px',
  border: '0.5px solid var(--ol-line-strong)',
  borderRadius: 8,
  background: 'var(--ol-surface-2)',
  display: 'inline-flex',
  alignItems: 'center',
  justifyContent: 'space-between',
  gap: 8,
  fontFamily: 'var(--ol-font-mono)',
  fontSize: 12.5,
  cursor: 'default',
  transition: 'background 0.16s var(--ol-motion-quick), border-color 0.16s var(--ol-motion-quick), color 0.16s var(--ol-motion-quick)',
};

const hotkeyRecorderLabelStyle: CSSProperties = {
  minWidth: 0,
  overflow: 'hidden',
  textOverflow: 'ellipsis',
  whiteSpace: 'nowrap',
};

const hotkeyClearButtonStyle: CSSProperties = {
  width: 18,
  height: 18,
  borderRadius: 999,
  display: 'inline-flex',
  alignItems: 'center',
  justifyContent: 'center',
  flexShrink: 0,
  background: 'rgba(0,0,0,0.2)',
  color: '#fff',
};

// ─── WaylandHotkeyCallout ────────────────────────────────────────────

/// Wayland 引导 callout：告诉用户绑桌面环境快捷键到 `listener-type --toggle-dictation`。
/// 显示条件由父组件控制（mount 时 invoke `is_wayland_cli_mode` 拉状态后渲染）。
/// 文案逐桌面环境列出步骤，每段 ≤ 5 行，详细的命令在 README 里。
function WaylandHotkeyCallout() {
  const { t } = useTranslation();
  const [helpOpen, setHelpOpen] = useState(false);
  // 三条命令各自独立的"已复制"反馈。用 string 而非 boolean 数组，
  // 避免 stale state（重复点击不同按钮时旧 timer 把别人擦掉）。
  const [copiedCommand, setCopiedCommand] = useState<string | null>(null);
  const copiedResetTimerRef = useRef<number | null>(null);

  useEffect(() => () => {
    if (copiedResetTimerRef.current != null) {
      window.clearTimeout(copiedResetTimerRef.current);
    }
  }, []);

  const onCopy = useCallback(async (command: string) => {
    try {
      await navigator.clipboard.writeText(command);
      setCopiedCommand(command);
      // 1.5s 后还原按钮文案；同时校验仍是这条命令，避免被后点的覆盖。
      if (copiedResetTimerRef.current != null) {
        window.clearTimeout(copiedResetTimerRef.current);
      }
      copiedResetTimerRef.current = window.setTimeout(() => {
        copiedResetTimerRef.current = null;
        setCopiedCommand(prev => (prev === command ? null : prev));
      }, 1500);
    } catch (err) {
      console.warn('[wayland-callout] clipboard write failed', err);
    }
  }, []);

  // 三条 CLI 命令 + 用途短标签。aeoform 在 #420 反馈 1.3.1-19 没提
  // --toggle-qa / --cancel-dictation，本次补全。
  const commandRows: Array<readonly [string, string]> = [
    ['listener-type --toggle-dictation', t('settings.recording.wayland.commandToggleDictationLabel')],
    ['listener-type --toggle-qa', t('settings.recording.wayland.commandToggleQaLabel')],
    ['listener-type --cancel-dictation', t('settings.recording.wayland.commandCancelDictationLabel')],
  ];

  const helpEntries: Array<readonly [string, string]> = [
    [t('settings.recording.wayland.gnomeTitle'), t('settings.recording.wayland.gnomeSteps')],
    [t('settings.recording.wayland.kdeTitle'), t('settings.recording.wayland.kdeSteps')],
    [t('settings.recording.wayland.hyprlandTitle'), t('settings.recording.wayland.hyprlandSteps')],
    [t('settings.recording.wayland.swayTitle'), t('settings.recording.wayland.swaySteps')],
  ];

  return (
    <div
      style={{
        marginTop: 10,
        marginBottom: 8,
        padding: '12px 14px',
        borderRadius: 10,
        background: 'rgba(217,119,6,0.08)',
        border: '0.5px solid rgba(217,119,6,0.22)',
      }}
    >
      <div style={{ fontSize: 12.5, fontWeight: 600, color: '#b45309', marginBottom: 4 }}>
        {t('settings.recording.wayland.calloutTitle')}
      </div>
      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-3)', lineHeight: 1.55, marginBottom: 8 }}>
        {t('settings.recording.wayland.calloutBody')}
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6, marginBottom: 8 }}>
        {commandRows.map(([command, label]) => (
          <div
            key={command}
            style={{ display: 'flex', alignItems: 'center', gap: 10 }}
          >
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 8,
                padding: '8px 10px',
                borderRadius: 8,
                background: 'var(--ol-glass-bg)',
                border: '0.5px solid var(--ol-line)',
                flex: 1,
                minWidth: 0,
              }}
            >
              <code
                style={{
                  flex: 1,
                  minWidth: 0,
                  fontSize: 12,
                  fontFamily: 'ui-monospace, SF Mono, Menlo, Consolas, monospace',
                  color: 'var(--ol-ink)',
                  userSelect: 'all',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  whiteSpace: 'nowrap',
                }}
              >
                {command}
              </code>
              <button
                type="button"
                onClick={() => onCopy(command)}
                style={{
                  padding: '4px 10px',
                  fontSize: 11,
                  fontWeight: 500,
                  border: '0.5px solid var(--ol-line-strong)',
                  borderRadius: 6,
                  background: 'var(--ol-control-active)',
                  color: 'var(--ol-ink-2)',
                  cursor: 'pointer',
                  fontFamily: 'inherit',
                  flexShrink: 0,
                }}
              >
                {copiedCommand === command
                  ? t('settings.recording.wayland.copyButtonCopied')
                  : t('settings.recording.wayland.copyButton')}
              </button>
            </div>
            <span
              style={{
                fontSize: 11,
                color: 'var(--ol-ink-3)',
                whiteSpace: 'nowrap',
                flexShrink: 0,
              }}
            >
              {label}
            </span>
          </div>
        ))}
      </div>
      <button
        type="button"
        onClick={() => setHelpOpen(prev => !prev)}
        style={{
          padding: 0,
          background: 'transparent',
          border: 0,
          color: '#b45309',
          fontSize: 11.5,
          fontWeight: 500,
          cursor: 'pointer',
          fontFamily: 'inherit',
        }}
      >
        {helpOpen ? '▾ ' : '▸ '}
        {t('settings.recording.wayland.helpToggle')}
      </button>
      {helpOpen && (
        <div style={{ marginTop: 8, display: 'flex', flexDirection: 'column', gap: 8 }}>
          {helpEntries.map(([title, body]) => (
            <div key={title}>
              <div style={{ fontSize: 11.5, fontWeight: 600, color: 'var(--ol-ink-2)', marginBottom: 2 }}>
                {title}
              </div>
              <div
                style={{
                  fontSize: 11,
                  color: 'var(--ol-ink-3)',
                  lineHeight: 1.55,
                  whiteSpace: 'pre-line',
                }}
              >
                {body}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
// ─── HotkeyRecorder ──────────────────────────────────────────────────

function HotkeyRecorder({
  binding,
  onCommit,
}: {
  binding: HotkeyBinding;
  onCommit: (codes: string[]) => void;
}) {
  const { t } = useTranslation();
  const [recording, setRecording] = useState(false);
  const [draftCodes, setDraftCodes] = useState<string[]>([]);
  const recorderStateRef = useRef(createHotkeyRecorderState());
  const recordingRef = useRef(false);

  const resetRecording = () => {
    recordingRef.current = false;
    recorderStateRef.current = createHotkeyRecorderState();
    setDraftCodes([]);
    setRecording(false);
  };

  const commitCodes = (codes: string[]) => {
    const ordered = orderHotkeyCodes(codes);
    resetRecording();
    onCommit(ordered);
  };

  const startRecording = () => {
    recordingRef.current = true;
    recorderStateRef.current = createHotkeyRecorderState();
    setDraftCodes([]);
    setRecording(true);
  };

  useEffect(() => {
    if (!recording) return undefined;

    const stopEvent = (event: Event) => {
      event.preventDefault();
      event.stopPropagation();
    };

    const applyHotkeyCode = (code: string, pressed: boolean) => {
      if (!recordingRef.current) return;
      const next = updateHotkeyRecorderState(recorderStateRef.current, code, pressed);
      recorderStateRef.current = next.state;
      setDraftCodes(next.state.draftCodes);
      if (next.commitCodes) commitCodes(next.commitCodes);
    };

    const onKeyDown = (event: KeyboardEvent) => {
      stopEvent(event);
      if (event.key === 'Escape' || event.code === 'Escape') {
        resetRecording();
        return;
      }
      const code = normalizeKeyboardHotkeyCode(event);
      if (!code) return;
      applyHotkeyCode(code, true);
    };

    const onKeyUp = (event: KeyboardEvent) => {
      stopEvent(event);
      if (!recordingRef.current) return;
      if (event.key === 'Escape' || event.code === 'Escape') {
        resetRecording();
        return;
      }
      const code = normalizeKeyboardHotkeyCode(event);
      if (!code) return;
      applyHotkeyCode(code, false);
    };

    const onMouseDown = (event: MouseEvent) => {
      const code = windowMouseHotkeyCode(event.button);
      if (!code) return;
      stopEvent(event);
      applyHotkeyCode(code, true);
    };

    const onMouseUp = (event: MouseEvent) => {
      const code = windowMouseHotkeyCode(event.button);
      if (!code) return;
      stopEvent(event);
      applyHotkeyCode(code, false);
    };

    window.addEventListener('keydown', onKeyDown, true);
    window.addEventListener('keyup', onKeyUp, true);
    window.addEventListener('mousedown', onMouseDown, true);
    window.addEventListener('mouseup', onMouseUp, true);
    return () => {
      window.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('keyup', onKeyUp, true);
      window.removeEventListener('mousedown', onMouseDown, true);
      window.removeEventListener('mouseup', onMouseUp, true);
    };
  }, [recording]);

  const label = recording
    ? draftCodes.length > 0
      ? draftCodes.map(getHotkeyCodeLabel).join('+')
      : t('settings.recording.hotkeyRecording')
    : getHotkeyBindingLabel(binding);
  const hasKeys = getHotkeyBindingCodes(binding).length > 0;

  return (
    <div style={{ display: 'inline-flex', alignItems: 'center', gap: 8 }}>
      <button
        type="button"
        onClick={startRecording}
        style={{
          ...hotkeyRecorderButtonStyle,
          borderColor: recording ? 'var(--ol-blue)' : 'var(--ol-line-strong)',
          color: recording ? 'var(--ol-blue)' : 'var(--ol-ink)',
        }}
      >
        <span style={hotkeyRecorderLabelStyle}>{label}</span>
        {!recording && hasKeys && (
          <span
            role="button"
            tabIndex={0}
            aria-label={t('settings.recording.hotkeyClear')}
            onClick={event => {
              event.stopPropagation();
              onCommit([]);
            }}
            onKeyDown={event => {
              if (event.key === 'Enter' || event.key === ' ') {
                event.preventDefault();
                event.stopPropagation();
                onCommit([]);
              }
            }}
            style={hotkeyClearButtonStyle}
          >
            <Icon name="x" size={11} strokeWidth={2} />
          </span>
        )}
      </button>
    </div>
  );
}

// ─── MicrophonePickerDialog ──────────────────────────────────────────

function MicrophonePickerDialog({
  devices,
  selectedName,
  onClose,
  onRefresh,
  loading,
  onSelect,
}: {
  devices: MicrophoneDevice[];
  selectedName: string;
  onClose: () => void;
  onRefresh: () => void;
  loading: boolean;
  onSelect: (name: string) => void;
}) {
  const { t } = useTranslation();
  const [pickedName, setPickedName] = useState(selectedName);
  const [previewName, setPreviewName] = useState(selectedName);
  const [level, setLevel] = useState(0);
  const [hoveredName, setHoveredName] = useState<string | null>(null);
  const [pressedName, setPressedName] = useState<string | null>(null);
  const [monitorError, setMonitorError] = useState<string | null>(null);
  const monitorQueueRef = useRef<Promise<void>>(Promise.resolve());

  const enqueueMonitorTask = useCallback((task: () => Promise<void>) => {
    const next = monitorQueueRef.current.catch(() => undefined).then(task);
    monitorQueueRef.current = next.catch(() => undefined);
    return next;
  }, []);

  useEffect(() => {
    setPickedName(selectedName);
    setPreviewName(selectedName);
  }, [selectedName]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    let timer: number | undefined;
    setLevel(0);
    setMonitorError(null);

    async function start() {
      await enqueueMonitorTask(async () => {
        try {
          if (isTauri) {
            const { listen } = await import('@tauri-apps/api/event');
            if (cancelled) return;
            const stopListening = await listen<{ level: number }>('microphone:level', event => {
              setLevel(Math.max(0, Math.min(1, event.payload.level ?? 0)));
            });
            if (cancelled) {
              stopListening();
              return;
            }
            unlisten = stopListening;
            await startMicrophoneLevelMonitor(previewName);
            if (cancelled) {
              unlisten?.();
              unlisten = undefined;
              await stopMicrophoneLevelMonitor();
            }
          } else {
            const tick = window.setInterval(() => {
              setLevel(0.25 + Math.random() * 0.55);
            }, 120);
            if (cancelled) {
              window.clearInterval(tick);
              return;
            }
            unlisten = () => window.clearInterval(tick);
          }
        } catch (err) {
          console.warn('[settings] microphone level monitor failed', err);
          if (!cancelled) {
            setMonitorError(err instanceof Error ? err.message : String(err));
          }
        }
      });
    }

    timer = window.setTimeout(() => {
      void start();
    }, 140);
    return () => {
      cancelled = true;
      if (timer !== undefined) {
        window.clearTimeout(timer);
      }
      void enqueueMonitorTask(async () => {
        unlisten?.();
        unlisten = undefined;
        await stopMicrophoneLevelMonitor();
      });
    };
  }, [enqueueMonitorTask, previewName]);

  const rows = [
    {
      id: 'default',
      name: '',
      label: t('settings.recording.microphoneDefault'),
      desc: t('settings.recording.microphoneDefaultDesc'),
      isDefault: false,
    },
    ...devices.map((device, index) => ({
      id: `${device.name}-${index}`,
      name: device.name,
      label: device.name,
      desc: device.isDefault ? t('settings.recording.microphoneSystemDefault') : '',
      isDefault: device.isDefault,
    })),
  ];

  return (
    <div
      role="presentation"
      onClick={onClose}
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 40,
        display: 'grid',
        placeItems: 'center',
        background: 'rgba(0,0,0,0.32)',
        animation: 'olMicPickerFadeIn 120ms ease-out',
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        onClick={e => e.stopPropagation()}
        style={{
          width: 450,
          maxWidth: 'calc(100vw - 48px)',
          borderRadius: 16,
          background: 'var(--ol-glass-bg-strong)',
          border: '0.5px solid var(--ol-line)',
          boxShadow: '0 24px 70px rgba(0,0,0,0.28)',
          padding: 24,
          animation: 'olMicPickerPopIn 160ms cubic-bezier(.2,.8,.2,1)',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12, marginBottom: 10 }}>
          <div style={{ fontSize: 18, fontWeight: 650 }}>{t('settings.recording.microphoneDialogTitle')}</div>
          <div style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
            <button
              type="button"
              onClick={onRefresh}
              disabled={loading}
              style={{
                border: 0,
                borderRadius: 999,
                background: 'transparent',
                color: loading ? 'var(--ol-ink-4)' : 'var(--ol-ink-3)',
                cursor: 'default',
                display: 'inline-flex',
                alignItems: 'center',
                justifyContent: 'center',
                width: 28,
                height: 28,
                opacity: loading ? 0.65 : 1,
                transition: 'background 0.16s var(--ol-motion-quick), opacity 0.16s var(--ol-motion-quick)',
              }}
              onMouseEnter={e => {
                if (!loading) e.currentTarget.style.background = 'var(--ol-hover-bg)';
              }}
              onMouseLeave={e => (e.currentTarget.style.background = 'transparent')}
              title={t('common.refresh')}
            >
              <Icon
                name="refresh"
                size={14}
                style={{ animation: loading ? 'olMicPickerSpin 800ms linear infinite' : undefined }}
              />
            </button>
            <button
              type="button"
              onClick={onClose}
              style={{
                border: 0,
                borderRadius: 999,
                background: 'transparent',
                color: 'var(--ol-ink-3)',
                cursor: 'default',
                display: 'inline-flex',
                alignItems: 'center',
                justifyContent: 'center',
                width: 28,
                height: 28,
                transition: 'background 0.16s var(--ol-motion-quick)',
              }}
              onMouseEnter={e => (e.currentTarget.style.background = 'var(--ol-hover-bg)')}
              onMouseLeave={e => (e.currentTarget.style.background = 'transparent')}
              title={t('common.close')}
            >
              <Icon name="close" size={14} />
            </button>
          </div>
        </div>
        <div style={{ fontSize: 12.5, color: 'var(--ol-ink-3)', lineHeight: 1.55, marginBottom: 18 }}>
          {t('settings.recording.microphoneDialogDesc')}
        </div>
        {monitorError && (
          <div style={{ fontSize: 11.5, color: 'var(--ol-err)', lineHeight: 1.45, marginBottom: 12 }}>
            {t('settings.recording.microphoneMonitorError', { message: monitorError })}
          </div>
        )}
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          {rows.map(row => {
            const active = pickedName === row.name;
            const previewing = previewName === row.name;
            const hovered = hoveredName === row.name;
            const pressed = pressedName === row.name;
            return (
              <button
                key={row.id}
                type="button"
                onMouseEnter={() => {
                  setHoveredName(row.name);
                }}
                onMouseLeave={() => {
                  setHoveredName(null);
                  setPressedName(null);
                }}
                onMouseDown={() => setPressedName(row.name)}
                onMouseUp={() => setPressedName(null)}
                onFocus={() => {
                  setHoveredName(row.name);
                }}
                onBlur={() => setHoveredName(null)}
                onClick={() => {
                  setPickedName(row.name);
                  setPreviewName(row.name);
                  onSelect(row.name);
                }}
                style={{
                  display: 'grid',
                  gridTemplateColumns: '1fr auto',
                  gap: 14,
                  alignItems: 'center',
                  width: '100%',
                  padding: '14px 16px',
                  borderRadius: 10,
                  border: active ? '1px solid rgba(101,123,112,0.7)' : '0.5px solid var(--ol-line-strong)',
                  background: active
                    ? 'rgba(101,123,112,0.08)'
                    : hovered
                      ? 'var(--ol-hover-bg)'
                      : 'var(--ol-control-active)',
                  boxShadow: active
                    ? '0 0 0 3px rgba(101,123,112,0.08)'
                    : hovered
                      ? 'var(--ol-control-active-shadow)'
                      : 'none',
                  color: 'var(--ol-ink)',
                  cursor: 'default',
                  textAlign: 'left',
                  transform: pressed ? 'scale(0.992)' : hovered ? 'translateY(-1px)' : 'translateY(0)',
                  transition: 'background 140ms ease, border-color 140ms ease, box-shadow 160ms ease, transform 120ms ease',
                }}
              >
                <span style={{ minWidth: 0 }}>
                  <span style={{ display: 'block', fontSize: 13, fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                    {row.label}
                  </span>
                  {row.desc && (
                    <span style={{ display: 'block', fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 3 }}>
                      {row.desc}
                    </span>
                  )}
                </span>
                <LevelMeter level={previewing ? level : 0} />
              </button>
            );
          })}
        </div>
        <style>
          {`
            @keyframes olMicPickerFadeIn {
              from { opacity: 0; }
              to { opacity: 1; }
            }
            @keyframes olMicPickerPopIn {
              from { opacity: 0; transform: translateY(8px) scale(.985); }
              to { opacity: 1; transform: translateY(0) scale(1); }
            }
            @keyframes olMicPickerSpin {
              from { transform: rotate(0deg); }
              to { transform: rotate(360deg); }
            }
          `}
        </style>
      </div>
    </div>
  );
}

// ─── LevelMeter ──────────────────────────────────────────────────────

function LevelMeter({ level }: { level: number }) {
  const amplified = Math.min(1, Math.max(0, level * 4.5));
  const bars = [0.25, 0.5, 0.75, 1, 0.75, 0.5];
  return (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4, height: 32 }}>
      {bars.map((weight, index) => {
        const intensity = Math.min(1, amplified * (0.85 + weight * 0.35));
        const height = 6 + intensity * (20 * weight);
        return (
          <span
            key={`${weight}-${index}`}
            style={{
              width: 5,
              height,
              borderRadius: 999,
              background: intensity > 0.08 ? 'var(--ol-blue)' : 'rgba(0,0,0,0.10)',
              opacity: 0.35 + intensity * 0.65,
              transition: 'height 70ms linear, opacity 90ms ease, background 120ms ease',
            }}
          />
        );
      })}
    </span>
  );
}

// ─── AutostartRow ────────────────────────────────────────────────────

// 不存进 prefs：autostart 状态由 OS 持有（mac LaunchAgent plist / linux .desktop /
// windows HKCU\Run），prefs 缓存反而会与 OS 真相不一致。issue #194。
function AutostartRow() {
  const { t } = useTranslation();
  const [enabled, setEnabled] = useState(false);
  const [loaded, setLoaded] = useState(false);
  // 切 plist / 注册表失败时给用户看的错误。null = 没有失败/上次操作已成功。
  // 不渲染等于把失败吞掉 —— Windows 写 HKCU\Run 被组策略拦、macOS 写
  // LaunchAgent plist 权限不够 都是真实可能。issue #194。
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isTauri) {
      setLoaded(true);
      return;
    }
    let cancelled = false;
    autostartIsEnabled()
      .then((v: boolean) => {
        if (!cancelled) {
          setEnabled(v);
          setLoaded(true);
        }
      })
      .catch((err: unknown) => {
        console.error('[autostart] isEnabled failed', err);
        if (!cancelled) setLoaded(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const onToggle = async (next: boolean) => {
    setEnabled(next);
    setError(null);
    try {
      if (!isTauri) return;
      if (next) await autostartEnable();
      else await autostartDisable();
    } catch (err) {
      console.error('[autostart] toggle failed', err);
      setEnabled(!next);
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <SettingRow
      label={t('settings.recording.startupAtBoot')}
      desc={t('settings.recording.startupAtBootDesc')}
    >
      <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
        {loaded ? <Toggle on={enabled} onToggle={onToggle} /> : null}
        {error && (
          <div style={{ fontSize: 11, color: 'var(--ol-err)', marginTop: 4, lineHeight: 1.5 }}>
            {t('settings.recording.startupAtBootError', { message: error })}
          </div>
        )}
      </div>
    </SettingRow>
  );
}

// ─── RecordingSection ────────────────────────────────────────────────

export function RecordingSection() {
  const { t } = useTranslation();
  const { prefs, capability, updatePrefs: savePrefs } = useHotkeySettings();
  const [microphoneDevices, setMicrophoneDevices] = useState<MicrophoneDevice[]>([]);
  const [microphoneDevicesLoaded, setMicrophoneDevicesLoaded] = useState(false);
  const [microphoneDevicesError, setMicrophoneDevicesError] = useState<string | null>(null);
  const [microphonePickerOpen, setMicrophonePickerOpen] = useState(false);
  // Wayland 下 rdev 监听不可用（issue #420）。改用 pull 模型：mount 时 invoke 拉状态。
  // 不能依赖一次性 event — Settings 模态是按需 mount，emit 早在 setup 阶段发完了。
  // XDG_SESSION_TYPE 在进程生命周期内不会变，拉一次即可，无需 polling 或 listener。
  const [waylandCliMode, setWaylandCliMode] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void isWaylandCliMode()
      .then(value => {
        if (!cancelled) setWaylandCliMode(value);
      })
      .catch((err: unknown) => {
        console.warn('[settings] is_wayland_cli_mode query failed', err);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const loadMicrophoneDevices = useCallback(async (
    signal?: { cancelled: boolean },
    options: { showLoading?: boolean } = {},
  ) => {
    if (options.showLoading ?? true) {
      setMicrophoneDevicesLoaded(false);
    }
    setMicrophoneDevicesError(null);
    try {
      const devices = await listMicrophoneDevices();
      if (signal?.cancelled) return;
      setMicrophoneDevices(devices);
      setMicrophoneDevicesLoaded(true);
    } catch (err) {
      console.error('[settings] list microphone devices failed', err);
      if (signal?.cancelled) return;
      setMicrophoneDevices([]);
      setMicrophoneDevicesError(err instanceof Error ? err.message : String(err));
      setMicrophoneDevicesLoaded(true);
    }
  }, []);

  useEffect(() => {
    const signal = { cancelled: false };
    void loadMicrophoneDevices(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [loadMicrophoneDevices]);

  useEffect(() => {
    if (!isTauri) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    async function listenForDeviceChanges() {
      const { listen } = await import('@tauri-apps/api/event');
      if (cancelled) return;
      const stopListening = await listen('microphone:devices-changed', () => {
        void loadMicrophoneDevices(undefined, { showLoading: false });
      });
      if (cancelled) {
        stopListening();
        return;
      }
      unlisten = stopListening;
    }
    void listenForDeviceChanges();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [loadMicrophoneDevices]);

  useEffect(() => {
    if (microphonePickerOpen) {
      void loadMicrophoneDevices(undefined, { showLoading: false });
    }
  }, [loadMicrophoneDevices, microphonePickerOpen]);

  if (!prefs || !capability) {
    return (
      <Card>
        <div style={{ fontSize: 12, color: 'var(--ol-ink-4)' }}>{t('common.loading')}</div>
      </Card>
    );
  }

  const onShowCapsuleChange = (showCapsule: boolean) =>
    savePrefs({ ...prefs, showCapsule });
  const onMuteDuringRecordingChange = (muteDuringRecording: boolean) =>
    savePrefs({ ...prefs, muteDuringRecording });
  const onMicrophoneDeviceChange = (microphoneDeviceName: string) =>
    savePrefs({ ...prefs, microphoneDeviceName });
  const onDictationInputSourceChange = (dictationInputSource: DictationInputSource) =>
    savePrefs({ ...prefs, dictationInputSource, dictationInputSourceUserOverridden: true });
  const onRestoreClipboardChange = (restoreClipboardAfterPaste: boolean) =>
    savePrefs({ ...prefs, restoreClipboardAfterPaste });
  const onPasteShortcutChange = (pasteShortcut: PasteShortcut) =>
    savePrefs({ ...prefs, pasteShortcut });
  const onAllowNonTsfFallbackChange = (allowNonTsfInsertionFallback: boolean) =>
    savePrefs({ ...prefs, allowNonTsfInsertionFallback });
  // 历史保留 / 对话感知 polish 上下文窗口都用裸 number input；空字符串时回滚到默认值。
  // 范围限制：retention 0-365 天，context window 0-60 分钟（再大的值对实际对话场景没意义且白烧 token）。
  const clamp = (n: number, min: number, max: number) => Math.max(min, Math.min(max, n));
  const onHistoryRetentionChange = (raw: string) => {
    const parsed = raw === '' ? 0 : Number.parseInt(raw, 10);
    if (Number.isNaN(parsed)) return;
    void savePrefs({ ...prefs, historyRetentionDays: clamp(parsed, 0, 365) });
  };
  const onPolishContextWindowChange = (raw: string) => {
    const parsed = raw === '' ? 0 : Number.parseInt(raw, 10);
    if (Number.isNaN(parsed)) return;
    void savePrefs({ ...prefs, polishContextWindowMinutes: clamp(parsed, 0, 60) });
  };
  const onStartMinimizedChange = (startMinimized: boolean) =>
    savePrefs({ ...prefs, startMinimized });
  const onAutoUpdateCheckChange = (autoUpdateCheck: boolean) =>
    savePrefs({ ...prefs, autoUpdateCheck });
  const onMarketplaceDevLoginChange = (marketplaceDevLogin: string) =>
    savePrefs({ ...prefs, marketplaceDevLogin });
  const onRecordAudioForDebugChange = (recordAudioForDebug: boolean) =>
    savePrefs({ ...prefs, recordAudioForDebug });
  // 历史条数 200 是当前 HISTORY_CAP（persistence.rs:32），下限 5 是避免用户填 0 导致
  // 写一条就立刻被清光；空字符串视为不限制，落回 null → 后端走 200 默认。
  const onHistoryMaxEntriesChange = (raw: string) => {
    const trimmed = raw.trim();
    if (trimmed === '') {
      void savePrefs({ ...prefs, historyMaxEntries: null });
      return;
    }
    const parsed = Number.parseInt(trimmed, 10);
    if (Number.isNaN(parsed)) return;
    void savePrefs({ ...prefs, historyMaxEntries: clamp(parsed, 5, 200) });
  };
  const onAudioRecordingMaxEntriesChange = (raw: string) => {
    const trimmed = raw.trim();
    if (trimmed === '') {
      void savePrefs({ ...prefs, audioRecordingMaxEntries: null });
      return;
    }
    const parsed = Number.parseInt(trimmed, 10);
    if (Number.isNaN(parsed)) return;
    void savePrefs({ ...prefs, audioRecordingMaxEntries: clamp(parsed, 1, 200) });
  };

  const hotkeyDesc = capability.requiresAccessibilityPermission
    ? t('settings.recording.hotkeyDescAcc')
    : t('settings.recording.hotkeyDescNoAcc');
  const preferredMicrophoneAvailable = Boolean(
    prefs.microphoneDeviceName
    && microphoneDevices.some(device => device.name === prefs.microphoneDeviceName),
  );
  const effectiveMicrophoneDeviceName = prefs.microphoneDeviceName
    && (!microphoneDevicesLoaded || preferredMicrophoneAvailable)
    ? prefs.microphoneDeviceName
    : '';
  const selectedMicrophoneLabel = effectiveMicrophoneDeviceName
    ? effectiveMicrophoneDeviceName
    : t('settings.recording.microphoneDefault');
  const selectedInputSource = prefs.dictationInputSource ?? 'embeddedBle';

  return (
    <>
    <Card>
      <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 4 }}>{t('settings.recording.title')}</div>
      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginBottom: 6 }}>{t('settings.recording.desc')}</div>
      {waylandCliMode && <WaylandHotkeyCallout />}
      <SettingRow label={t('settings.recording.hotkeyLabel')} desc={hotkeyDesc}>
        <ShortcutRecorder
          value={prefs.dictationHotkey}
          onSave={async binding => {
            await setDictationHotkey(binding);
            await savePrefs({ ...prefs, dictationHotkey: binding });
          }}
        />
      </SettingRow>
      <SettingRow label={t('settings.recording.inputSourceLabel')} desc={t('settings.recording.inputSourceDesc')}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8, width: '100%', maxWidth: 430 }}>
          <div style={{ display: 'inline-flex', alignSelf: 'flex-start', padding: 2, borderRadius: 8, background: 'var(--ol-control-track)' }}>
            {([
              ['microphone', t('settings.recording.inputSourceMicrophone'), 'mic'],
              ['embeddedBle', t('settings.recording.inputSourceEmbeddedBle'), 'bolt'],
            ] as const).map(([value, label, icon]) => {
              const active = selectedInputSource === value;
              return (
                <button
                  key={value}
                  onClick={() => onDictationInputSourceChange(value)}
                  style={{
                    minWidth: 112,
                    height: 28,
                    padding: '0 10px',
                    fontSize: 12,
                    fontWeight: 500,
                    border: 0,
                    borderRadius: 6,
                    fontFamily: 'inherit',
                    background: active ? 'var(--ol-control-active)' : 'transparent',
                    color: active ? 'var(--ol-ink)' : 'var(--ol-ink-3)',
                    boxShadow: active ? 'var(--ol-control-active-shadow)' : 'none',
                    cursor: 'default',
                    display: 'inline-flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    gap: 6,
                    transition: 'background 0.16s var(--ol-motion-quick), color 0.16s var(--ol-motion-quick), box-shadow 0.18s var(--ol-motion-soft)',
                  }}
                >
                  <Icon name={icon} size={13} />
                  <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{label}</span>
                </button>
              );
            })}
          </div>
        </div>
      </SettingRow>
      <SettingRow label={t('settings.recording.microphoneLabel')} desc={t('settings.recording.microphoneDesc')}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
          <button
            type="button"
            aria-label={t('settings.recording.microphoneLabel')}
            onClick={() => {
              setMicrophonePickerOpen(true);
            }}
            onKeyDown={e => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                setMicrophonePickerOpen(true);
              }
            }}
            onChange={() => {}}
            style={{
              ...inputStyle,
              flex: '0 0 auto',
              width: 200,
              maxWidth: 200,
              height: 32,
              minWidth: 0,
              alignSelf: 'flex-start',
              padding: '0 9px 0 10px',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              gap: 8,
              textAlign: 'left',
              color: 'var(--ol-ink)',
            }}
          >
            <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
              {selectedMicrophoneLabel}
            </span>
            <Icon name="chevRight" size={13} />
          </button>
          {!microphoneDevicesLoaded && (
            <div style={{ fontSize: 11, color: 'var(--ol-ink-4)' }}>{t('common.loading')}</div>
          )}
          {microphoneDevicesError && (
            <div style={{ fontSize: 11, color: 'var(--ol-err)', lineHeight: 1.5 }}>
              {t('settings.recording.microphoneLoadError', { message: microphoneDevicesError })}
            </div>
          )}
        </div>
      </SettingRow>
      {microphonePickerOpen && (
        <MicrophonePickerDialog
          devices={microphoneDevices}
          selectedName={effectiveMicrophoneDeviceName}
          onClose={() => setMicrophonePickerOpen(false)}
          onRefresh={() => {
            void loadMicrophoneDevices();
          }}
          loading={!microphoneDevicesLoaded}
          onSelect={(name) => {
            onMicrophoneDeviceChange(name);
          }}
        />
      )}
      <SettingRow label={t('settings.recording.capsuleLabel')} desc={t('settings.recording.capsuleDesc')}>
        <Toggle on={prefs.showCapsule} onToggle={onShowCapsuleChange} />
      </SettingRow>
      <SettingRow
        label={t('settings.recording.muteDuringRecordingLabel')}
        desc={t('settings.recording.muteDuringRecordingDesc')}
      >
        <Toggle on={prefs.muteDuringRecording} onToggle={onMuteDuringRecordingChange} />
      </SettingRow>
    </Card>

    {/* ─── 插入与剪贴板（折叠） ──────────────────────────────────── */}
    <Collapsible title={t('settings.recording.insertGroupTitle')}>
      <SettingRow
        label={t('settings.recording.restoreClipboardLabel')}
        desc={t('settings.recording.restoreClipboardDesc')}
      >
        <Toggle on={prefs.restoreClipboardAfterPaste} onToggle={onRestoreClipboardChange} />
      </SettingRow>
      {capability.adapter !== 'macEventTap' && (
        <SettingRow
          label={t('settings.recording.pasteShortcutLabel')}
          desc={t('settings.recording.pasteShortcutDesc')}
        >
          <SelectLite
            value={prefs.pasteShortcut}
            onChange={next => onPasteShortcutChange(next as PasteShortcut)}
            options={[
              { value: 'ctrlV', label: t('settings.recording.pasteShortcutCtrlV') },
              { value: 'ctrlShiftV', label: t('settings.recording.pasteShortcutCtrlShiftV') },
              { value: 'shiftInsert', label: t('settings.recording.pasteShortcutShiftInsert') },
            ]}
            ariaLabel={t('settings.recording.pasteShortcutLabel')}
            style={{ ...inputStyle, maxWidth: 220 }}
          />
        </SettingRow>
      )}
      {capability.adapter === 'windowsLowLevel' && (
        <SettingRow
          label={t('settings.recording.allowNonTsfFallbackLabel')}
          desc={t('settings.recording.allowNonTsfFallbackDesc')}
        >
          <Toggle
            on={prefs.allowNonTsfInsertionFallback}
            onToggle={onAllowNonTsfFallbackChange}
          />
        </SettingRow>
      )}
    </Collapsible>

    {/* ─── 历史与上下文（折叠） ────────────────────────────────── */}
    <Collapsible title={t('settings.recording.historyGroupTitle')}>
      <SettingRow
        label={t('settings.recording.historyRetentionLabel')}
        desc={t('settings.recording.historyRetentionDesc')}
      >
        <input
          type="number"
          min={0}
          max={365}
          value={prefs.historyRetentionDays}
          onChange={e => onHistoryRetentionChange(e.target.value)}
          style={{ ...inputStyle, width: 80, textAlign: 'right' }}
        />
      </SettingRow>
      <SettingRow
        label={t('settings.recording.historyMaxEntriesLabel')}
        desc={t('settings.recording.historyMaxEntriesDesc')}
      >
        <input
          type="number"
          min={5}
          max={200}
          placeholder="200"
          value={prefs.historyMaxEntries ?? ''}
          onChange={e => onHistoryMaxEntriesChange(e.target.value)}
          style={{ ...inputStyle, width: 80, textAlign: 'right' }}
        />
      </SettingRow>
      <SettingRow
        label={t('settings.recording.polishContextWindowLabel')}
        desc={t('settings.recording.polishContextWindowDesc')}
      >
        <input
          type="number"
          min={0}
          max={60}
          value={prefs.polishContextWindowMinutes}
          onChange={e => onPolishContextWindowChange(e.target.value)}
          style={{ ...inputStyle, width: 80, textAlign: 'right' }}
        />
      </SettingRow>
      <SettingRow
        label={t('settings.recording.recordAudioForDebugLabel')}
        desc={t('settings.recording.recordAudioForDebugDesc')}
      >
        <Toggle on={prefs.recordAudioForDebug} onToggle={onRecordAudioForDebugChange} />
      </SettingRow>
      <SettingRow
        label={t('settings.recording.audioRecordingMaxEntriesLabel')}
        desc={t('settings.recording.audioRecordingMaxEntriesDesc')}
      >
        <input
          type="number"
          min={1}
          max={200}
          placeholder="200"
          value={prefs.audioRecordingMaxEntries ?? ''}
          onChange={e => onAudioRecordingMaxEntriesChange(e.target.value)}
          style={{ ...inputStyle, width: 80, textAlign: 'right' }}
          disabled={!prefs.recordAudioForDebug}
        />
      </SettingRow>
    </Collapsible>

    {/* ─── 启动（折叠） ──────────────────────────────────────────── */}
    <Collapsible title={t('settings.recording.startupGroupTitle')}>
      <AutostartRow />
      <SettingRow
        label={t('settings.recording.startMinimizedLabel')}
        desc={t('settings.recording.startMinimizedDesc')}
      >
        <Toggle on={prefs.startMinimized} onToggle={onStartMinimizedChange} />
      </SettingRow>
      <SettingRow
        label={t('settings.recording.autoUpdateCheckLabel')}
        desc={t('settings.recording.autoUpdateCheckDesc')}
      >
        <Toggle on={prefs.autoUpdateCheck} onToggle={onAutoUpdateCheckChange} />
      </SettingRow>
      {capability.statusHint && (
        <div style={{ marginTop: 6, fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5 }}>
          {capability.statusHint}
        </div>
      )}
    </Collapsible>

    {/* ─── 风格市场（折叠） ────────────────────────────────────────── */}
    {/* Listener Type 后端未上线前，远端市场默认禁用；本地风格包导入/导出继续可用。 */}
    <Collapsible title={t('settings.recording.marketplaceGroupTitle')}>
      <SettingRow
        label={t('settings.recording.marketplaceDevLoginLabel')}
        desc={t('settings.recording.marketplaceDevLoginDesc')}
      >
        <input
          type="text"
          placeholder="your-github-login"
          value={prefs.marketplaceDevLogin}
          onChange={e => onMarketplaceDevLoginChange(e.target.value)}
          style={{ ...inputStyle, width: 180 }}
        />
      </SettingRow>
    </Collapsible>
    </>
  );
}
