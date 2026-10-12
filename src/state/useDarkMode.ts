import { useEffect } from 'react';
import { getSettings, isTauri } from '../lib/ipc';

/** Shared window theme; importing it does not initialize the main app. */
export function useDarkMode() {
  useEffect(() => {
    const applyTheme = (enabled: boolean) => {
      document.documentElement.setAttribute('data-theme', enabled ? 'dark' : 'light');
      localStorage.setItem('ol-dark-mode', String(enabled));
    };
    if (localStorage.getItem('ol-dark-mode') === 'true') {
      document.documentElement.setAttribute('data-theme', 'dark');
    }
    if (!isTauri) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      try {
        const prefs = await getSettings();
        if (!cancelled) applyTheme(prefs.darkMode);
      } catch (error) {
        console.warn('[theme] initial dark mode sync failed', error);
      }
      try {
        const { listen } = await import('@tauri-apps/api/event');
        const handle = await listen<boolean>('dark-mode-changed', event => {
          applyTheme(event.payload);
        });
        if (cancelled) handle();
        else unlisten = handle;
      } catch { /* non-tauri */ }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
}
