import { useTranslation } from 'react-i18next';
import type { DeviceCustomKeyId, DeviceKeyProfile } from '../lib/types';
import { layoutKeyIds } from '../lib/deviceKeyLayouts';
import { DeviceKeyActionPreview } from './DeviceKeyActionPreview';
import hardwarePhoto from '../../docs/assets/readme/keyboard-front.jpg';

/** Positions refer to the existing product photograph, not a connected device. */
const locations: Record<DeviceCustomKeyId, { left: string; top: string }> = {
  key1: { left: '36.5%', top: '44%' }, key2: { left: '49%', top: '52%' },
  key3: { left: '61%', top: '60%' }, key4: { left: '74%', top: '70%' },
  knob: { left: '83.5%', top: '36.5%' },
};

export function DeviceLayoutMap({ profile, group, selected, onSelect }: {
  profile: DeviceKeyProfile;
  group: 'deviceCustomKeys' | 'deviceCustomKeyDoubleClicks' | 'deviceCustomKeyLongPresses';
  selected: DeviceCustomKeyId;
  onSelect: (id: DeviceCustomKeyId) => void;
}) {
  const { t } = useTranslation();
  return <section className="ol-device-layout-map" aria-label={t('hardwareLayout.title')}>
    <figure>
      <div className="ol-device-layout-photo">
        <img src={hardwarePhoto} alt={t('hardwareLayout.photo')} />
        {layoutKeyIds.map(id => <span key={id} aria-hidden="true" data-selected={selected === id} style={locations[id]}>
          {id === 'knob' ? '●' : id.slice(3)}
        </span>)}
      </div>
      <figcaption>{t('hardwareLayout.caption')}</figcaption>
    </figure>
    <div className="ol-device-layout-pad">
      {layoutKeyIds.map(id => <button type="button" key={id} aria-pressed={selected === id} onClick={() => onSelect(id)}>
        <strong>{id === 'knob' ? t('settings.deviceKeys.knob.title') : t('deviceLayouts.key', { number: id.slice(3) })}</strong>
        <span>{t(`settings.deviceKeys.actions.${profile[id === 'knob' ? 'deviceCustomKeys' : group][id].action}`)}</span>
        <DeviceKeyActionPreview mapping={profile[id === 'knob' ? 'deviceCustomKeys' : group][id]} />
      </button>)}
    </div>
  </section>;
}

export { hardwarePhoto };
