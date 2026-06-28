import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { WindowChrome, detectOS } from './components/WindowChrome';
import { getSettings, isTauri } from './lib/ipc';
import { PageHeader, Pill } from './pages/_atoms';
import { CompanionSection } from './pages/settings/CompanionSection';
import { HotkeySettingsProvider } from './state/HotkeySettingsContext';

function useCompanionDarkMode() {
  useEffect(() => {
    const applyTheme = (enabled: boolean) => {
      document.documentElement.setAttribute('data-theme', enabled ? 'dark' : 'light');
      localStorage.setItem('ol-dark-mode', String(enabled));
    };

    if (localStorage.getItem('ol-dark-mode') === 'true') {
      document.documentElement.setAttribute('data-theme', 'dark');
    }
    if (!isTauri) return;

    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const prefs = await getSettings();
        if (!cancelled) {
          applyTheme(prefs.darkMode);
        }
      } catch (error) {
        console.warn('[companion] initial dark mode sync failed', error);
      }

      try {
        const { listen } = await import('@tauri-apps/api/event');
        unlisten = await listen<boolean>('dark-mode-changed', event => {
          applyTheme(event.payload);
        });
      } catch {
        // Browser previews do not expose the Tauri event bus.
      }
    })();

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
}

export function CompanionApp() {
  const { t } = useTranslation();
  useCompanionDarkMode();

  return (
    <HotkeySettingsProvider>
      <WindowChrome os={detectOS()} title="Companion Type" height="100%">
        <main
          className="ol-thinscroll"
          style={{
            height: '100%',
            overflow: 'auto',
            background: 'var(--ol-window-bg)',
            padding: 24,
          }}
        >
          <div style={{ maxWidth: 1180, margin: '0 auto', display: 'flex', flexDirection: 'column', gap: 14 }}>
            <PageHeader
              kicker={t('companionApp.kicker', 'Companion')}
              title={t('companionApp.title', 'Companion Type')}
              desc={t('companionApp.desc', '独立的 Companion 调试、录音链路、蓝牙升级和有线刷机工作台。')}
              titleRight={<Pill tone="blue" size="sm">{t('companionApp.badge', 'WB55 bring-up')}</Pill>}
            />
            <CompanionSection />
          </div>
        </main>
      </WindowChrome>
    </HotkeySettingsProvider>
  );
}
