import { useTranslation } from 'react-i18next';
import { TEXT_PROVIDER_SETUP_LINKS } from '../lib/providerSetupLinks';

export function ProviderSetupGuide({ providerId, providerName, expanded }: {
  providerId: string;
  providerName: string;
  expanded: boolean;
}) {
  const { t } = useTranslation();
  // Codex OAuth already has its own local-login explanation, not API-key setup.
  if (providerId === 'codex_oauth') return null;
  const links = TEXT_PROVIDER_SETUP_LINKS[providerId];
  return (
    <details data-provider-setup-guide={providerId} open={expanded}
      style={{ margin: '10px 0 14px', padding: '10px 12px', borderRadius: 8, border: '1px solid var(--ol-line)', background: 'var(--ol-surface)' }}>
      <summary style={{ cursor: 'pointer', color: 'var(--ol-ink-2)', fontSize: 12, fontWeight: 600 }}>
        {t('settings.providers.setupGuide.title', { provider: providerName })}
      </summary>
      <div style={{ color: 'var(--ol-ink-3)', fontSize: 11.5, lineHeight: 1.65, paddingTop: 8 }}>
        <p style={{ margin: '0 0 6px' }}>{t('settings.providers.setupGuide.scope')}</p>
        <ol style={{ margin: '0 0 8px', paddingLeft: 20 }}>
          <li>{t('settings.providers.setupGuide.account')}</li>
          <li>{t('settings.providers.setupGuide.model')}</li>
          <li>{t('settings.providers.setupGuide.check')}</li>
        </ol>
        {links ? (
          <div style={{ display: 'flex', gap: '4px 14px', flexWrap: 'wrap' }}>
            <a href={links.setup} target="_blank" rel="noreferrer" style={{ color: 'var(--ol-ink-2)', textDecoration: 'underline' }}>
              {t('settings.providers.setupGuide.setupLink', { provider: providerName })}
            </a>
            <a href={links.pricing} target="_blank" rel="noreferrer" style={{ color: 'var(--ol-ink-2)', textDecoration: 'underline' }}>
              {t('settings.providers.setupGuide.pricingLink')}
            </a>
          </div>
        ) : <p style={{ margin: 0 }}>{t('settings.providers.setupGuide.other')}</p>}
        <p style={{ margin: '8px 0 0', color: 'var(--ol-ink-4)' }}>{t('settings.providers.setupGuide.cost')}</p>
      </div>
    </details>
  );
}
