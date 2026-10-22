import { useTranslation } from 'react-i18next';
import type { SettingsSectionId } from '../pages/Settings';
import { openExternal } from '../lib/ipc';
import { Btn } from '../pages/_atoms';

const GUIDE_URL = 'https://github.com/Listener-ai-Macau/Listener-Type#readme';

export function HelpCenter({ onOpenSettings }: { onOpenSettings: (section: SettingsSectionId) => void }) {
  const { t } = useTranslation();
  return (
    <article data-help-center style={{ display: 'flex', flexDirection: 'column', gap: 18, fontSize: 13, lineHeight: 1.7, color: 'var(--ol-ink-2)' }}>
      <div>
        <p style={{ margin: '0 0 8px' }}>{t('offlineHelp.intro')}</p>
        <p style={{ margin: 0, color: 'var(--ol-ink-3)' }}>{t('offlineHelp.offline')}</p>
      </div>
      {(['speak', 'services', 'text', 'finish', 'costs', 'limits', 'data'] as const).map(id => (
        <section key={id}>
          <h3 style={{ margin: '0 0 5px', fontSize: 14, fontWeight: 600, color: 'var(--ol-ink)' }}>{t(`offlineHelp.${id}`)}</h3>
          <p style={{ margin: 0 }}>{t(`offlineHelp.${id}Body`)}</p>
          {id === 'speak' && <p style={{ margin: '6px 0 0', color: 'var(--ol-ink-3)' }}>{t('offlineHelp.wake')}</p>}
          {(id === 'speak' || id === 'services') && <div style={{ marginTop: 8 }}><Btn size="sm" variant="ghost" onClick={() => onOpenSettings(id === 'speak' ? 'recording' : 'providers')}>
            {t(`offlineHelp.${id === 'speak' ? 'recording' : 'providers'}`)}
          </Btn></div>}
        </section>
      ))}
      <a href={GUIDE_URL} target="_blank" rel="noopener noreferrer" onClick={event => {
        if ('__TAURI_INTERNALS__' in window) { event.preventDefault(); void openExternal(GUIDE_URL); }
      }}>{t('offlineHelp.online')} ↗</a>
    </article>
  );
}
