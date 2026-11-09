import { useTranslation } from 'react-i18next';
import { detectOS } from '../../components/WindowChrome';
import { useHotkeySettings } from '../../state/HotkeySettingsContext';
import type { PostDictationKey } from '../../lib/types';
import { SelectLite } from '../../components/ui/SelectLite';
import { SettingRow, Toggle, inputStyle } from './shared';

/** One location for host-wide dictation output preferences, independent of hardware. */
export function DictationOutputSettings() {
  const { t } = useTranslation();
  const { prefs, updatePrefs: savePrefs } = useHotkeySettings();
  const os = detectOS();
  const isMac = os === 'mac';
  const isWin = os === 'win';
  if (!prefs) return null;
  return <>
        <SettingRow
          label={t('settings.advanced.streamingInsertLabel')}
          desc={t(
            isMac
              ? 'settings.advanced.streamingInsertHintMac'
              : isWin
                ? 'settingsOrganization.streamingHintWindows'
                : 'settings.advanced.streamingInsertHintLinux'
          )}>
          <Toggle
            on={!!prefs?.streamingInsert}
            onToggle={(next) => {
              if (prefs) void savePrefs({ ...prefs, streamingInsert: next });
            }}
          />
        </SettingRow>
            <SettingRow
              label={t('settings.recording.removeFillerWordsLabel')}
              desc={t('settings.recording.removeFillerWordsDesc')}
            >
              <Toggle
                on={prefs.removeFillerWords}
                onToggle={removeFillerWords =>
                  savePrefs(current => ({ ...current, removeFillerWords }))}
              />
            </SettingRow>
            <SettingRow
              label={t('settings.recording.copyDictationToClipboardLabel')}
              desc={t('settings.recording.copyDictationToClipboardDesc')}
            >
              <Toggle
                on={prefs.copyDictationToClipboard}
                onToggle={copyDictationToClipboard =>
                  savePrefs(current => ({ ...current, copyDictationToClipboard }))}
              />
            </SettingRow>
            {detectOS() === 'win' && prefs.copyDictationToClipboard && (
              <SettingRow
                label={t('settings.recording.copyFinalImmediatelyLabel')}
                desc={t('settings.recording.copyFinalImmediatelyDesc')}
              >
                <Toggle
                  on={prefs.copyFinalTextWithoutPasteConfirmation}
                  onToggle={copyFinalTextWithoutPasteConfirmation =>
                    savePrefs(current => ({ ...current, copyFinalTextWithoutPasteConfirmation }))}
                />
              </SettingRow>
            )}
            <SettingRow
              label={t('settings.recording.sendKeyAfterDictationLabel')}
              desc={t('settings.recording.sendKeyAfterDictationDesc')}
            >
              <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                <Toggle
                  on={prefs.sendKeyAfterDictation}
                  onToggle={sendKeyAfterDictation =>
                    savePrefs(current => ({ ...current, sendKeyAfterDictation }))}
                />
                <SelectLite
                  value={prefs.postDictationKey}
                  onChange={postDictationKey =>
                    savePrefs(current => ({
                      ...current,
                      postDictationKey: postDictationKey as PostDictationKey,
                    }))}
                  options={[
                    { value: 'enter', label: t('settings.recording.postDictationKeyEnter') },
                    { value: 'ctrlEnter', label: t('settings.recording.postDictationKeyCtrlEnter') },
                  ]}
                  ariaLabel={t('settings.recording.postDictationKeyLabel')}
                  style={{ ...inputStyle, width: 132 }}
                  disabled={!prefs.sendKeyAfterDictation}
                />
              </div>
            </SettingRow>
  </>;
}
