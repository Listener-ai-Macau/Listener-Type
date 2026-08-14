import { useEffect, type CSSProperties } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import type { VoiceprintStatus } from '../../lib/types';
import { Btn } from '../_atoms';

const STEP_TARGET_MS = [600, 600, 600] as const;

interface VoiceprintEnrollmentWizardProps {
  open: boolean;
  attempted: boolean;
  busy: boolean;
  phrase: string;
  status: VoiceprintStatus | null;
  onStart: () => void;
  onCancel: () => void;
  onClose: () => void;
}

export function VoiceprintEnrollmentWizard({
  open,
  attempted,
  busy,
  phrase,
  status,
  onStart,
  onCancel,
  onClose,
}: VoiceprintEnrollmentWizardProps) {
  const { t } = useTranslation();
  const captureActive = ['preparing', 'armed', 'capturing', 'processing']
    .includes(status?.state ?? '');
  const success = attempted && status?.state === 'complete' && status.enrolled;
  const failed = attempted && status?.state === 'error';
  const phase = success ? 'success' : failed ? 'error' : captureActive ? 'capture' : 'intro';

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      if (captureActive) onCancel();
      else onClose();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [captureActive, onCancel, onClose, open]);

  if (!open) return null;

  const currentStep = Math.max(1, Math.min(3, status?.captureStep ?? 1));
  const speechMs = status?.stepSpeechMs ?? [0, 0, 0];
  const currentSpeechProgress = Math.min(
    100,
    Math.round(((speechMs[currentStep - 1] ?? 0) / STEP_TARGET_MS[currentStep - 1]) * 100),
  );
  const feedbackKey = status?.captureFeedback ?? 'waiting';
  const signalLevel = Math.max(0, Math.min(100, status?.signalLevel ?? 0));
  const feedback = t(
    `settings.recording.voiceprintWizardFeedback.${feedbackKey}`,
    {
      defaultValue: feedbackKey === 'good'
        ? '很好，保持停顿，等待下一步'
        : feedbackKey === 'hearing'
          ? '正在听，请自然说完'
          : feedbackKey === 'too_quiet'
            ? '没有听清，请靠近设备再说一次'
            : feedbackKey === 'too_loud'
              ? '声音过大，请稍微离远一点'
              : '准备好后开始说话',
    },
  );
  const stepLabels = Array.from({ length: 3 }, (_, index) => t(
    'settings.recording.voiceprintWizardWakeStep',
    {
      current: index + 1,
      phrase,
      defaultValue: '第 {{current}} 次说“{{phrase}}”',
    },
  ));

  return createPortal(
    <div
      className="ol-voiceprint-wizard-backdrop"
      role="presentation"
      onMouseDown={event => {
        if (event.target === event.currentTarget && !captureActive) onClose();
      }}
    >
      <section
        className="ol-voiceprint-wizard"
        role="dialog"
        aria-modal="true"
        aria-labelledby="voiceprint-wizard-title"
      >
        {phase === 'intro' && (
          <>
            <div className="ol-voiceprint-wizard-hero" aria-hidden="true">
              <span className="ol-voiceprint-wizard-mic">●</span>
            </div>
            <h2 id="voiceprint-wizard-title">
              {t('settings.recording.voiceprintWizardTitle', '录入你的声音')}
            </h2>
            <p className="ol-voiceprint-wizard-lead">
              {t(
                'settings.recording.voiceprintWizardIntro',
                '录入后，只有你的声音能自动唤醒；旁人的说话也不会拖长录音。',
              )}
            </p>
            <div className="ol-voiceprint-wizard-preflight">
              <div><span>1</span>{t('settings.recording.voiceprintWizardQuiet', '尽量保持周围安静')}</div>
              <div><span>2</span>{t('settings.recording.voiceprintWizardDistance', '在平时使用的位置自然说话')}</div>
              <div><span>3</span>{t('settings.recording.voiceprintWizardPrivacy', '录音仅用于本机生成声纹，完成后立即丢弃')}</div>
            </div>
            <div className="ol-voiceprint-wizard-actions">
              <Btn onClick={onClose}>{t('common.cancel', '取消')}</Btn>
              <Btn variant="blue" onClick={onStart} disabled={busy || !status?.available}>
                {busy
                  ? t('settings.recording.voiceprintPreparing', '准备中')
                  : t('settings.recording.voiceprintWizardStart', '开始录入')}
              </Btn>
            </div>
          </>
        )}

        {phase === 'capture' && (
          <>
            <div className="ol-voiceprint-wizard-topline">
              <span>{t('settings.recording.voiceprintWizardProgress', { current: currentStep, total: 3, defaultValue: '步骤 {{current}} / {{total}}' })}</span>
              <button type="button" onClick={onCancel} disabled={busy}>
                {t('common.cancel', '取消')}
              </button>
            </div>
            <div
              className={`ol-voiceprint-wizard-orb is-${feedbackKey}`}
              style={{
                '--voice-scale': String(0.86 + signalLevel / 180),
                '--voice-opacity': String(0.2 + signalLevel / 180),
              } as CSSProperties}
              aria-hidden="true"
            >
              <span />
            </div>
            <h2 id="voiceprint-wizard-title">
              {status?.state === 'preparing' || status?.state === 'armed'
                ? t('settings.recording.voiceprintGuidePreparing', '正在准备设备，请稍候…')
                : status?.state === 'processing'
                  ? t('settings.recording.voiceprintGuideProcessing', '录制完成，正在本机生成声纹…')
                  : t('settings.recording.voiceprintWizardSayPhrase', { phrase, defaultValue: '请说“{{phrase}}”' })}
            </h2>
            <p className={`ol-voiceprint-wizard-feedback is-${feedbackKey}`} aria-live="polite">
              {status?.state === 'processing'
                ? t('settings.recording.voiceprintWizardDoNotClose', '马上就好，请不要关闭窗口')
                : feedback}
            </p>
            <div className="ol-voiceprint-wizard-steps">
              {stepLabels.map((label, index) => {
                const step = index + 1;
                const good = (speechMs[index] ?? 0) >= STEP_TARGET_MS[index];
                const active = status?.state === 'capturing' && step === currentStep;
                return (
                  <div key={step} className={`${good ? 'is-good' : ''}${active ? ' is-active' : ''}`}>
                    <span>{good ? '✓' : step}</span>
                    <small>{label}</small>
                  </div>
                );
              })}
            </div>
            {status?.state === 'capturing' && (
              <div className="ol-voiceprint-wizard-speech-progress" aria-label={`${currentSpeechProgress}%`}>
                <span style={{ width: `${Math.max(3, currentSpeechProgress)}%` }} />
              </div>
            )}
            <p className="ol-voiceprint-wizard-tip">
              {t('settings.recording.voiceprintWizardPauseTip', '说完后停一下，看到下一步再继续')}
            </p>
          </>
        )}

        {phase === 'success' && (
          <>
            <div className="ol-voiceprint-wizard-result is-success" aria-hidden="true">✓</div>
            <h2 id="voiceprint-wizard-title">
              {t('settings.recording.voiceprintWizardSuccessTitle', '声音录入完成')}
            </h2>
            <p className="ol-voiceprint-wizard-lead">
              {t('settings.recording.voiceprintWizardSuccessDesc', '主人声纹已经生效。现在会优先保留你的声音，并过滤旁人的干扰。')}
            </p>
            <div className="ol-voiceprint-wizard-actions is-single">
              <Btn variant="blue" onClick={onClose}>{t('common.done', '完成')}</Btn>
            </div>
          </>
        )}

        {phase === 'error' && (
          <>
            <div className="ol-voiceprint-wizard-result is-error" aria-hidden="true">!</div>
            <h2 id="voiceprint-wizard-title">
              {t('settings.recording.voiceprintWizardErrorTitle', '这次没有录好')}
            </h2>
            <p className="ol-voiceprint-wizard-lead is-error">{status?.error}</p>
            <p className="ol-voiceprint-wizard-tip">
              {t('settings.recording.voiceprintWizardErrorTip', '请靠近设备、降低周围噪声，然后重新录入。')}
            </p>
            <div className="ol-voiceprint-wizard-actions">
              <Btn onClick={onClose}>{t('common.cancel', '取消')}</Btn>
              <Btn variant="blue" onClick={onStart} disabled={busy}>
                {t('settings.recording.voiceprintRedo', '重新录制')}
              </Btn>
            </div>
          </>
        )}
      </section>
    </div>,
    document.body,
  );
}
