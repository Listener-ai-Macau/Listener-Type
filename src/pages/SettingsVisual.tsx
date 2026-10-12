import { Settings } from './Settings';
import { HotkeySettingsProvider } from '../state/HotkeySettingsContext';

export function SettingsVisual({ section }: { section: 'advanced' | 'shortcuts' | 'device' }) {
  return (
    <HotkeySettingsProvider>
      <div className="ol-settings-visual-root" style={{ width: '100%', height: '100%', padding: 24, overflow: 'auto', background: 'var(--ol-window-bg)' }}>
        <Settings embedded initialSection={section} />
      </div>
    </HotkeySettingsProvider>
  );
}
