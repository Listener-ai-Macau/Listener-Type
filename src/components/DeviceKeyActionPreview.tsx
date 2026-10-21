import { useTranslation } from 'react-i18next';
import { formatComboLabel } from '../lib/hotkey';
import type { DeviceCustomKeyMapping } from '../lib/types';

/** Show the mapping's actual parameter, never a separately stored label. */
export function DeviceKeyActionPreview({ mapping }: { mapping: DeviceCustomKeyMapping }) {
  const { t } = useTranslation();
  const text = mapping.action === 'pasteTemplate' ? mapping.pasteTemplate
    : mapping.action === 'openApp' ? t(`settings.deviceKeys.appPages.${mapping.appPage}`)
    : mapping.action === 'sendShortcut' && mapping.shortcut ? formatComboLabel(mapping.shortcut)
    : mapping.action === 'openExternalApp' ? mapping.externalAppPath.split(/[\\/]/).pop() ?? '' : '';
  return text ? <span className="ol-device-key-phrase" title={mapping.action === 'openExternalApp' ? mapping.externalAppPath : text}>{text}</span> : null;
}
