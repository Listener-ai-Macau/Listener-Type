import { useTranslation } from 'react-i18next';
import { Icon } from '../components/Icon';
import type { AppTab } from '../state/useAppState';
import { PageHeader } from './_atoms';

const TOOLS: Array<{ id: AppTab; icon: string }> = [
  { id: 'style', icon: 'style' },
  { id: 'vocab', icon: 'vocab' },
  { id: 'translation', icon: 'translate' },
  { id: 'selectionAsk', icon: 'selectionAsk' },
];

export function Tools({ onOpen }: { onOpen: (tab: AppTab) => void }) {
  const { t } = useTranslation();
  return <>
    <PageHeader title={t('nav.tools')} desc={t('tools.description')} />
    <div className="ol-tools-grid" style={{ display: 'grid', gridTemplateColumns: 'repeat(2, minmax(0, 1fr))', gap: 12 }}>
      {TOOLS.map(tool => <button key={tool.id} type="button" onClick={() => onOpen(tool.id)}
        className="ol-tool-card" data-tool={tool.id} style={{
          display: 'flex', alignItems: 'center', gap: 14, textAlign: 'left', padding: 20,
          background: 'var(--ol-surface)', border: '0.5px solid var(--ol-line)',
          borderRadius: 'var(--ol-r-lg)', color: 'var(--ol-ink)', cursor: 'pointer',
          boxShadow: 'var(--ol-shadow-sm)', font: 'inherit', minWidth: 0,
        }}>
        <Icon name={tool.icon} size={21} style={{ color: 'var(--ol-ink-3)', flexShrink: 0 }} />
        <span style={{ flex: 1, minWidth: 0 }}>
          <span style={{ display: 'block', fontWeight: 600, marginBottom: 5 }}>{t(`nav.${tool.id}`)}</span>
          <span style={{ display: 'block', fontSize: 12, lineHeight: 1.6, color: 'var(--ol-ink-3)' }}>{t(`tools.${tool.id}`)}</span>
        </span>
        <Icon name="chevRight" size={15} style={{ color: 'var(--ol-ink-4)', flexShrink: 0 }} />
      </button>)}
    </div>
    <style>{`
      .ol-tool-card:hover { border-color: var(--ol-ink-4) !important; }
      .ol-tool-card:focus-visible { outline: 2px solid var(--ol-ink-3); outline-offset: 3px; }
      @media (max-width: 760px) { .ol-tools-grid { grid-template-columns: minmax(0, 1fr) !important; } }
    `}</style>
  </>;
}
