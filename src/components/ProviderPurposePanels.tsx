import { useEffect, useId, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import './ProviderPurposePanels.css';

/** Changing the visible purpose keeps both credential forms mounted. */
export function ProviderPurposePanels({ speech, text, prioritizeText, textOnly, speechOnly }: {
  speech: ReactNode;
  text: ReactNode;
  prioritizeText: boolean;
  textOnly: boolean;
  speechOnly: boolean;
}) {
  const { t } = useTranslation();
  const id = useId();
  const [purpose, setPurpose] = useState<'speech' | 'text'>(prioritizeText ? 'text' : 'speech');
  useEffect(() => { setPurpose(prioritizeText ? 'text' : 'speech'); }, [prioritizeText]);
  if (textOnly) return <>{text}</>;
  if (speechOnly) return <>{speech}</>;
  return <>
    <div role="group" aria-label={t('guidedSetup.servicePurpose')} className="ol-provider-purpose" data-provider-purpose>
      {(['speech', 'text'] as const).map(value => <button key={value} type="button"
        id={`${id}-${value}-button`} aria-controls={`${id}-${value}-panel`}
        aria-pressed={purpose === value} onClick={() => setPurpose(value)}>
        {t(`guidedSetup.${value}Purpose`)}
      </button>)}
    </div>
    <p className="ol-style-trial-hint">{t(`guidedSetup.${purpose}Hint`)}</p>
    <div id={`${id}-speech-panel`} role="region" aria-labelledby={`${id}-speech-button`}
      hidden={purpose !== 'speech'} data-provider-panel="speech">{speech}</div>
    <div id={`${id}-text-panel`} role="region" aria-labelledby={`${id}-text-button`}
      hidden={purpose !== 'text'} data-provider-panel="text">{text}</div>
  </>;
}
