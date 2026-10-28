// Translation.tsx — 独立的"翻译"页，从 Settings → 录音 中拆出来。
// 用户在这里：
//   - 勾选自己的工作语言（多选，用作 LLM polish/translate prompt 的前提）
//   - 选一个翻译目标语言（单选；选"不启用"则 Shift 不触发翻译）
//   - 看完整使用说明（怎么触发、按钮位置、胶囊显示）

import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Card, PageHeader } from './_atoms';
import { SavedToast } from '../components/SavedToast';
import { StyleTextTrial } from '../components/StyleTextTrial';
import { LocalTranslationSetup } from '../components/LocalTranslationSetup';
import { SelectLite } from '../components/ui/SelectLite';
import { SUPPORTED_LANGUAGES } from '../lib/types';
import { useHotkeySettings } from '../state/HotkeySettingsContext';
import { formatComboLabel } from '../lib/hotkey';
import { translationTrialPack } from '../lib/translationTrial';
import type { UserPreferences } from '../lib/types';

type SaveState = 'idle' | 'saving' | 'saved' | 'failed';

export function Translation() {
  const { t } = useTranslation();
  const { prefs, loading, error, refresh, updatePrefs: savePrefs } = useHotkeySettings();
  const [saveState, setSaveState] = useState<SaveState>('idle');
  const [saveMessage, setSaveMessage] = useState('');
  const [trialOpen, setTrialOpen] = useState(false);
  const [trialDraft, setTrialDraft] = useState('');
  const [preparingLocal, setPreparingLocal] = useState(false);
  const statusTimer = useRef<number | null>(null);

  useEffect(() => () => {
    if (statusTimer.current !== null) window.clearTimeout(statusTimer.current);
  }, []);

  const showSaveStatus = (state: SaveState, message: string, temporary = false) => {
    if (statusTimer.current !== null) {
      window.clearTimeout(statusTimer.current);
      statusTimer.current = null;
    }
    setSaveState(state);
    setSaveMessage(message);
    if (temporary) {
      statusTimer.current = window.setTimeout(() => {
        setSaveState('idle');
        setSaveMessage('');
        statusTimer.current = null;
      }, 1600);
    }
  };

  const persistPrefs = async (
    resolveNext: (current: UserPreferences) => UserPreferences,
    failureMessage: string,
  ) => {
    showSaveStatus('saving', t('common.saving'));
    try {
      await savePrefs(resolveNext);
      showSaveStatus('saved', t('common.saved'), true);
    } catch (error) {
      console.error('[translation] failed to save preferences', error);
      showSaveStatus('failed', failureMessage);
      await refresh().catch(refreshError => {
        console.warn('[translation] failed to refresh preferences after save error', refreshError);
      });
    }
  };

  if (!prefs) {
    return (
      <>
        <PageHeader title={t('translation.title')} />
        <Card>
          {error ? (
            <div role="alert" style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
              <div style={{ fontSize: 12, color: 'var(--ol-red, #ef4444)', lineHeight: 1.5 }}>
                {t('common.settingsLoadFailed')}：{error}
              </div>
              <button
                type="button"
                onClick={() => { void refresh(); }}
                disabled={loading}
                style={{
                  alignSelf: 'flex-start',
                  padding: '6px 12px',
                  borderRadius: 999,
                  border: 0,
                  background: 'var(--ol-blue)',
                  color: '#fff',
                  fontSize: 12,
                  fontWeight: 600,
                  cursor: loading ? 'not-allowed' : 'default',
                  opacity: loading ? 0.64 : 1,
                }}
              >
                {loading ? t('common.loading') : t('common.retry')}
              </button>
            </div>
          ) : (
            <div style={{ fontSize: 12, color: 'var(--ol-ink-4)' }}>{t('common.loading')}</div>
          )}
        </Card>
      </>
    );
  }

  const onWorkingLanguagesChange = (workingLanguages: string[]) => {
    void persistPrefs(
      current => ({ ...current, workingLanguages }),
      t('translation.save.workingFailed'),
    );
  };
  const toggleWorkingLanguage = (lang: string) => {
    const next = prefs.workingLanguages.includes(lang)
      ? prefs.workingLanguages.filter(l => l !== lang)
      : [...prefs.workingLanguages, lang];
    onWorkingLanguagesChange(next);
  };
  const onTargetChange = (translationTargetLanguage: string) => {
    void persistPrefs(
      current => ({ ...current, translationTargetLanguage }),
      t('translation.save.targetFailed'),
    );
  };

  const triggerLabel = formatComboLabel(prefs.dictationHotkey);
  const translationHotkeyLabel = formatComboLabel(prefs.translationHotkey);
  const enabled = prefs.translationTargetLanguage.trim() !== '';

  const targetOptions = [
    { value: '', label: t('translation.target.disabled') },
    ...SUPPORTED_LANGUAGES.map(lang => ({ value: lang, label: lang })),
  ];

  return (
    <>
      <PageHeader title={t('translation.title')} />

      <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
        {error && (
          <div
            role="alert"
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              gap: 10,
              padding: '8px 12px',
              borderRadius: 10,
              border: '0.5px solid rgba(239,68,68,0.22)',
              background: 'rgba(239,68,68,0.07)',
              color: 'var(--ol-red, #ef4444)',
              fontSize: 11.5,
              lineHeight: 1.5,
            }}
          >
            <span>{t('common.settingsLoadFailed')}：{error}</span>
            <button
              type="button"
              onClick={() => { void refresh(); }}
              disabled={loading}
              style={{
                flex: '0 0 auto',
                border: 0,
                borderRadius: 999,
                background: 'rgba(239,68,68,0.12)',
                color: 'inherit',
                padding: '4px 10px',
                fontSize: 11,
                fontWeight: 600,
                cursor: loading ? 'not-allowed' : 'default',
                opacity: loading ? 0.64 : 1,
              }}
            >
              {loading ? t('common.loading') : t('common.retry')}
            </button>
          </div>
        )}

        <SavedToast saveState={saveState} message={saveMessage} />

        {/* The output choice comes first; working languages do not choose it. */}
        <Card>
          <label style={{ display: 'flex', flexDirection: 'column', gap: 8, marginBottom: 16 }}>
            <span style={{ fontSize: 13, fontWeight: 600 }}>{t('offlineTranslation.engine')}</span>
            <SelectLite ariaLabel={t('offlineTranslation.engine')} value={prefs.translationEngine ?? 'cloud'} disabled={preparingLocal}
              onChange={value => { const translationEngine = value as 'local' | 'cloud';
                void persistPrefs(current => ({ ...current, translationEngine }), t('translation.save.targetFailed')); }}
              options={[{ value: 'local', label: t('offlineTranslation.local') }, { value: 'cloud', label: t('offlineTranslation.cloud') }]}
              style={{ width: '100%', maxWidth: 440, fontSize: 13 }} />
          </label>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 12 }}>
            <div style={{ fontSize: 13, fontWeight: 600 }}>{t('translation.target.title')}</div>
            <span
              style={{
                padding: '2px 8px',
                fontSize: 10.5,
                fontWeight: 600,
                letterSpacing: 0,
                borderRadius: 999,
                background: enabled ? 'rgba(101,123,112,0.10)' : 'rgba(0,0,0,0.05)',
                color: enabled ? 'var(--ol-blue)' : 'var(--ol-ink-4)',
                textTransform: 'uppercase',
              }}
            >
              {enabled ? t('translation.statusEnabled') : t('translation.statusDisabled')}
            </span>
          </div>
          <div style={{ fontSize: 12, color: 'var(--ol-ink-3)', lineHeight: 1.6, marginBottom: 10 }}>
            {prefs.translationHotkey.primary.trim() ? t('translation.target.desc', { shortcut: translationHotkeyLabel }) : t('shortcutDefaults.translationUnbound')}
          </div>
          <SelectLite
            value={prefs.translationTargetLanguage}
            onChange={onTargetChange}
            options={targetOptions}
            placeholder={t('translation.target.disabled')}
            ariaLabel={t('translation.target.title')}
            style={{ width: '100%', maxWidth: 360, fontSize: 13, background: 'var(--ol-white)' }}
            disabled={preparingLocal}
          />
          {prefs.translationEngine === 'local' && <LocalTranslationSetup
            key={prefs.localTranslationQuality ?? 'standard'} quality={prefs.localTranslationQuality ?? 'standard'}
            onQualityChange={localTranslationQuality => { void persistPrefs(current => ({ ...current, localTranslationQuality }), t('translation.save.targetFailed')); }}
            source={prefs.translationSourceLanguage ?? ''} target={prefs.translationTargetLanguage}
            onSourceChange={translationSourceLanguage => { void persistPrefs(current => ({ ...current, translationSourceLanguage }), t('translation.save.targetFailed')); }}
            onBusy={setPreparingLocal} />}
          <p style={{ marginTop: 12, fontSize: 12, color: 'var(--ol-ink-3)', lineHeight: 1.6 }}>{t('offlineTranslation.voiceHint')}</p>
          <div style={{ marginTop: 12 }}>
            <button type="button" className="ol-btn ol-btn-secondary" disabled={!enabled}
              data-translation-try-text onClick={() => setTrialOpen(true)}>
              {t('translation.tryText')}
            </button>
            <div style={{ marginTop: 8, fontSize: 12, color: 'var(--ol-ink-3)', lineHeight: 1.6 }}>
              {t('translation.tryTextHint')}
            </div>
          </div>
        </Card>

        <Card>
          <div data-translation-howto>
            <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 10 }}>{t('translation.howto.title')}</div>
            <ol style={{ margin: 0, paddingLeft: 18, fontSize: 12.5, color: 'var(--ol-ink-2)', lineHeight: 1.7 }}>
              <li>{t('translation.howto.step1')}</li>
              <li>{prefs.dictationHotkey.primary.trim() ? t('translation.howto.step2', { trigger: triggerLabel }) : t('shortcutDefaults.usageUnbound')}</li>
              <li>{prefs.translationHotkey.primary.trim() ? t('translation.howto.step3', { shortcut: translationHotkeyLabel }) : t('shortcutDefaults.translationUnbound')}</li>
              <li>{t('translation.howto.step4')}</li>
              <li>{t('translation.howto.step5')}</li>
            </ol>
          </div>
          <div data-translation-device-howto style={{ marginTop: 14, fontSize: 12.5, color: 'var(--ol-ink-2)', lineHeight: 1.7 }}>
            <div style={{ fontWeight: 600 }}>{t('translation.howto.deviceTitle')}</div>
            <div>{t('translation.howto.deviceDesc')}</div>
          </div>
          <div data-translation-mode-indicator style={{ marginTop: 12, fontSize: 12, color: 'var(--ol-ink-3)', lineHeight: 1.6 }}>
            {t('translation.howto.indicatorDesc')}
          </div>
        </Card>

        <Card>
          <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 8 }}>{t('translation.working.title')}</div>
          <div style={{ fontSize: 12, color: 'var(--ol-ink-3)', lineHeight: 1.6, marginBottom: 12 }}>
            {t('translation.working.desc')}
          </div>
          <div role="group" aria-label={t('translation.working.title')} style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
            {SUPPORTED_LANGUAGES.map(lang => {
              const checked = prefs.workingLanguages.includes(lang);
              return (
                <button
                  key={lang}
                  aria-pressed={checked}
                  onClick={() => toggleWorkingLanguage(lang)}
                  style={{
                    padding: '6px 12px',
                    fontSize: 12.5,
                    fontWeight: checked ? 600 : 500,
                    border: 0,
                    borderRadius: 999,
                    background: checked ? 'var(--ol-blue)' : 'rgba(0,0,0,0.05)',
                    color: checked ? '#fff' : 'var(--ol-ink-2)',
                    cursor: 'default',
                    fontFamily: 'inherit',
                    transition: 'background 0.12s ease-out, color 0.12s ease-out',
                  }}
                >
                  {lang}
                </button>
              );
            })}
          </div>
        </Card>

      </div>
      {trialOpen && enabled && (
        <StyleTextTrial
          pack={translationTrialPack(prefs.translationTargetLanguage,
            t('translation.trialName', { language: prefs.translationTargetLanguage }), t('translation.tryTextHint'))}
          translationTarget={prefs.translationTargetLanguage}
          initialTranslationEngine={prefs.translationEngine ?? 'cloud'}
          initialTranslationSource={prefs.translationSourceLanguage}
          initialTranslationQuality={prefs.localTranslationQuality}
          draft={trialDraft} onDraftChange={setTrialDraft} onClose={() => setTrialOpen(false)}
        />
      )}
    </>
  );
}
