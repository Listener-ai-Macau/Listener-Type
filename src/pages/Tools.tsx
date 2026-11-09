import { useTranslation } from 'react-i18next';
import { useState } from 'react';
import { Icon } from '../components/Icon';
import { DeviceLayoutsDialog } from '../components/DeviceLayoutsDialog';
import { writingDeviceKeyProfile } from '../lib/deviceKeyLayouts';
import type { AppTab } from '../state/useAppState';
import { Btn, PageHeader } from './_atoms';

type ToolEntry = { id: string; icon: string; title: string; hint: string; open: () => void };

export function Tools({ onOpen, onTryText, onOpenAudioFile, onOpenSavedDrafts }: {
  onOpen: (tab: AppTab) => void; onTryText: () => void;
  onOpenAudioFile: () => void; onOpenSavedDrafts: () => void;
}) {
  const { t } = useTranslation();
  const [layoutsOpen, setLayoutsOpen] = useState<'configure' | 'practice' | 'phrases' | null>(null);
  const visibleTools: ToolEntry[] = [
    { id: 'text-workspace', icon: 'style', title: 'simpleTextWorkflow.workspace', hint: 'simpleTextWorkflow.workspaceHint', open: onTryText },
    { id: 'saved-text', icon: 'history', title: 'textDraft.title', hint: 'simpleTextWorkflow.savedHint', open: onOpenSavedDrafts },
    { id: 'audio-file', icon: 'mic', title: 'audioFile.title', hint: 'audioFile.entryHint', open: onOpenAudioFile },
    { id: 'selectionAsk', icon: 'selectionAsk', title: 'nav.selectionAsk', hint: 'tools.selectionAsk', open: () => onOpen('selectionAsk') },
  ];
  const setup: ToolEntry[] = [
    { id: 'style', icon: 'style', title: 'simpleTextWorkflow.dictationStyles', hint: 'tools.style', open: () => onOpen('style') },
    { id: 'vocab', icon: 'vocab', title: 'nav.vocab', hint: 'tools.vocab', open: () => onOpen('vocab') },
    { id: 'translation', icon: 'translate', title: 'simpleTextWorkflow.translationSettings', hint: 'tools.translation', open: () => onOpen('translation') },
  ];
  return <>
    <PageHeader title={t('nav.tools')} desc={t('simpleTextWorkflow.toolsHint')} />
    <ToolGrid entries={visibleTools} />
    <h3 style={{ margin: '24px 0 10px', fontSize: 13, color: 'var(--ol-ink-3)' }}>{t('simpleTextWorkflow.setup')}</h3>
    <ToolGrid entries={setup} />
    <details className="ol-tools-more" style={{ marginTop: 18, color: 'var(--ol-ink-3)', fontSize: 12 }}>
      <summary style={{ display: 'flex', alignItems: 'center', gap: 9, cursor: 'pointer',
        padding: '11px 14px', border: '0.5px solid var(--ol-line)', borderRadius: 'var(--ol-r-md)',
        background: 'var(--ol-surface)', color: 'var(--ol-ink-2)', fontSize: 13, fontWeight: 600,
        listStyle: 'none' }}>
        <span className="ol-tools-more-chevron" aria-hidden="true" style={{ display: 'inline-flex' }}>
          <Icon name="chevRight" size={15} />
        </span>
        {t('simpleTextWorkflow.deviceKeys')}
      </summary>
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8, paddingTop: 10 }}>
        <Btn size="sm" onClick={() => setLayoutsOpen('configure')}>{t('deviceLayouts.open')}</Btn>
        <Btn size="sm" onClick={() => setLayoutsOpen('practice')}>{t('devicePractice.open')}</Btn>
        <Btn size="sm" onClick={() => setLayoutsOpen('phrases')}>{t('savedPhrases.title')}</Btn>
      </div>
    </details>
    {layoutsOpen && <DeviceLayoutsDialog rememberSelection initialPractice={layoutsOpen === 'practice'} initialPhrases={layoutsOpen === 'phrases'} initialProfile={layoutsOpen === 'practice' ? writingDeviceKeyProfile() : undefined} onClose={() => setLayoutsOpen(null)} />}
    <style>{`
      .ol-tool-card:hover { border-color: var(--ol-ink-4) !important; }
      .ol-tool-card:focus-visible { outline: 2px solid var(--ol-ink-3); outline-offset: 3px; }
      .ol-tools-more > summary::-webkit-details-marker { display: none; }
      .ol-tools-more > summary:hover { border-color: var(--ol-ink-4) !important; }
      .ol-tools-more > summary:focus-visible { outline: 2px solid var(--ol-ink-3); outline-offset: 3px; }
      .ol-tools-more-chevron { transition: transform .16s ease; }
      .ol-tools-more[open] > summary .ol-tools-more-chevron { transform: rotate(90deg); }
      @media (max-width: 760px) { .ol-tools-grid { grid-template-columns: minmax(0, 1fr) !important; } }
    `}</style>
  </>;
}

function ToolGrid({ entries }: { entries: ToolEntry[] }) {
  const { t } = useTranslation();
  return <div className="ol-tools-grid" style={{ display: 'grid', gridTemplateColumns: 'repeat(2, minmax(0, 1fr))', gap: 12 }}>
    {entries.map(tool => <button key={tool.id} type="button" onClick={tool.open}
      className="ol-tool-card" data-tool={tool.id} style={{
        display: 'flex', alignItems: 'center', gap: 14, textAlign: 'left', padding: 20,
        background: 'var(--ol-surface)', border: '0.5px solid var(--ol-line)',
        borderRadius: 'var(--ol-r-lg)', color: 'var(--ol-ink)', cursor: 'pointer',
        boxShadow: 'var(--ol-shadow-sm)', font: 'inherit', minWidth: 0,
      }}>
      <Icon name={tool.icon} size={21} style={{ color: 'var(--ol-ink-3)', flexShrink: 0 }} />
      <span style={{ flex: 1, minWidth: 0 }}>
        <span style={{ display: 'block', fontWeight: 600, marginBottom: 5 }}>{t(tool.title)}</span>
        <span style={{ display: 'block', fontSize: 12, lineHeight: 1.6, color: 'var(--ol-ink-3)' }}>{t(tool.hint)}</span>
      </span>
      <Icon name="chevRight" size={15} style={{ color: 'var(--ol-ink-4)', flexShrink: 0 }} />
    </button>)}
  </div>;
}
