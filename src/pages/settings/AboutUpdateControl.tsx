// 关于面板里嵌入的"检查更新"控件。
// 注：原 Settings 内的 About tab 已并入 SettingsModal 的 AboutMini；这里只是版本号 + 检查按钮。

import { useTranslation } from 'react-i18next';
import { isDialogStatus, UpdateDialog, useAutoUpdate } from '../../components/AutoUpdate';
import { APP_SOURCE_REVISION, APP_VERSION_LABEL, APP_DISPLAY_VERSION_LABEL } from '../../lib/appVersion';
import { Btn } from '../_atoms';
import { openExternal } from '../../lib/ipc';

export function AboutUpdateControl({ tagline }: { tagline: string }) {
  const { t } = useTranslation();
  const u = useAutoUpdate();
  const previewRelease = APP_VERSION_LABEL.includes('-');
  const openDownloads = () => void openExternal(`https://github.com/Listener-ai-Macau/Listener-Type/releases/${previewRelease ? `tag/${APP_VERSION_LABEL}` : 'latest'}`);
  return (
    <>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 2 }}>
        <span style={{ fontSize: 12, color: 'var(--ol-ink-3)' }}>{tagline} · {APP_DISPLAY_VERSION_LABEL}</span>
        <Btn variant="ghost" size="sm" onClick={previewRelease ? openDownloads : u.checkForUpdates} disabled={u.checking || u.busy}>
          {previewRelease ? t('settings.about.openDownloads') : u.checking ? t('settings.about.checkingUpdate') : t('settings.about.checkUpdateBtn')}
        </Btn>
      </div>
      <div style={{ fontSize: 11, color: 'var(--ol-ink-4)', marginTop: 4 }}>
        {t('settings.about.sourceRevision', { revision: APP_SOURCE_REVISION })}
      </div>
      {previewRelease && <div style={{ fontSize: 11, color: 'var(--ol-ink-4)', marginTop: 4 }}>
        {t('settings.about.previewUpdatesManual')}
      </div>}
      {(u.status === 'none' || u.status === 'error') && (
        <div style={{ fontSize: 11, color: u.status === 'error' ? 'var(--ol-err)' : 'var(--ol-ink-4)', marginTop: 4 }}>
          {u.status === 'none' ? t('settings.about.upToDate') : t(u.errorStage === 'check' ? 'settings.about.updateCheckError' : 'settings.about.updateError')}
          {u.status === 'error' && (
            <Btn size="sm" variant="ghost" onClick={openDownloads}>
              {t('settings.about.openDownloads')}
            </Btn>
          )}
        </div>
      )}
      {isDialogStatus(u.status) && (
        <UpdateDialog
          status={u.status}
          version={u.version}
          progress={u.progress}
          downloaded={u.downloaded}
          contentLength={u.contentLength}
          onInstall={u.installUpdate}
          onClose={u.dismissDialog}
        />
      )}
    </>
  );
}
