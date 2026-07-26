// FloatingShell.tsx — frosted outer frame + raised inner console.
// Sidebar lives INSIDE the console card. Footer icons sit on the frosted outer.
// Settings is no longer a sidebar tab — it opens as a centered modal sheet.
//
// Ported verbatim from design_handoff_listener_type/variants.jsx::FloatingShell.

import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ComponentType, type CSSProperties } from 'react';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { listen } from '@tauri-apps/api/event';
import { Icon } from './Icon';
import { WindowChrome, detectOS, type OS } from './WindowChrome';
import { SettingsModal } from './SettingsModal';
import { Overview } from '../pages/Overview';
import { History } from '../pages/History';
import { Vocab } from '../pages/Vocab';
import { Style } from '../pages/Style';
import { Translation } from '../pages/Translation';
import { SelectionAsk } from '../pages/SelectionAsk';
// 风格市场不再作为独立 nav tab —— 已整合为 Style 页面内 modal（入口在「风格包」标题右侧）。
// LocalAsr 不再作为主 nav tab——本地 ASR 模型管理已合并到 Settings → Advanced 中
// 通过 Settings -> Advanced 内的 <LocalAsr /> 渲染。这里之前的 import 与 NAV_BASE 条目都已移除。
import { APP_VERSION_LABEL } from '../lib/appVersion';
import { createDemoModeSession, type DemoModeSession } from '../lib/demoMode';
import { embeddedBleProbeErrorMessage, runEmbeddedBleProbeWithTimeout, type EmbeddedBleProbeStatus } from '../lib/embeddedBleProbe';
import { buildFirstRunPairingWizard, type FirstRunPairingAction, type FirstRunPairingStageStatus } from '../lib/firstRunPairingWizard';
import { applyFontScale, readFontScale } from '../lib/fontScale';
import {
  getCredentials,
  getEmbeddedBleRuntimeStatus,
  isMainWindowStartHidden,
  openSystemSettings,
  recoverEmbeddedBleDevice,
} from '../lib/ipc';
import type { DeviceCustomKeyAppPage, EmbeddedBleRepairResult, EmbeddedBleRuntimeStatus } from '../lib/types';
import {
  PROVIDER_SETUP_PROMPT_DEFERRED_KEY,
  shouldShowProviderSetupPrompt,
} from '../lib/providerSetup';
import { type SettingsSectionId } from '../pages/Settings';
import { useHotkeySettings } from '../state/HotkeySettingsContext';
import { useAppState, type AppTab } from '../state/useAppState';

interface NavItem {
  id: AppTab;
  name: string;
  icon: string;
  cmp: ComponentType;
}

const NAV_BASE: Array<Omit<NavItem, 'name'>> = [
  { id: 'overview', icon: 'overview', cmp: Overview },
  { id: 'history', icon: 'history', cmp: History },
  { id: 'vocab', icon: 'vocab', cmp: Vocab },
  { id: 'style', icon: 'style', cmp: Style },
  { id: 'translation', icon: 'translate', cmp: Translation },
  { id: 'selectionAsk', icon: 'selectionAsk', cmp: SelectionAsk },
];

const BLE_PAIRING_PROMPT_ACK_KEY = 'ol.blePairingPromptAck';
const BLE_PAIRING_PROMPT_DEFERRED_KEY = 'ol.blePairingPromptDeferredThisSession';
const DEV_SETTINGS_SECTIONS: SettingsSectionId[] = ['recording', 'device', 'providers', 'shortcuts', 'permissions', 'language', 'advanced'];

interface FloatingShellProps {
  os?: OS;
  initialTab?: AppTab;
  initialSettings?: boolean;
}

export function FloatingShell({ os: osProp, initialTab = 'overview', initialSettings = false }: FloatingShellProps) {
  const os = osProp ?? detectOS();
  return (
    <WindowChrome os={os} title="Listener Type" height="100%">
      <FloatingShellBody os={os} initialTab={initialTab} initialSettings={initialSettings} />
    </WindowChrome>
  );
}

function FloatingShellBody({ os, initialTab, initialSettings }: { os: OS; initialTab: AppTab; initialSettings: boolean }) {
  const { t } = useTranslation();
  const { prefs, updatePrefs } = useHotkeySettings();
  const { currentTab, setCurrentTab, settingsOpen, setSettingsOpen } = useAppState(initialTab, initialSettings);
  const [settingsInitialSection, setSettingsInitialSection] = useState<SettingsSectionId | undefined>();
  const [providerPromptOpen, setProviderPromptOpen] = useState(false);
  const [blePairingPromptOpen, setBlePairingPromptOpen] = useState(false);
  const [demoModeSession, setDemoModeSession] = useState<DemoModeSession | null>(null);

  // displayTab 是实际渲染的 tab，currentTab 是用户点中的目标 tab。
  const [displayTab, setDisplayTab] = useState<AppTab>(initialTab);
  const [tabPhase, setTabPhase] = useState<'idle' | 'exiting'>('idle');
  useEffect(() => {
    if (currentTab === displayTab) return;
    setTabPhase('exiting');
    const id = window.setTimeout(() => {
      setDisplayTab(currentTab);
      setTabPhase('idle');
    }, 180);
    return () => window.clearTimeout(id);
  }, [currentTab, displayTab]);

  // 字体档位 — 启动时按 localStorage 应用一次；之后改动来自 Settings 的"个性化"section。
  useEffect(() => {
    applyFontScale(readFontScale());
  }, []);

  const NAV = useMemo<NavItem[]>(
    () => NAV_BASE.map(b => ({ ...b, name: t(`nav.${b.id}`) })),
    [t],
  );
  const Page = (NAV.find((n) => n.id === displayTab) ?? NAV[0]).cmp;

  // sidebar nav 滑动指示器：测量当前 active button 的 offsetTop / height，
  // 用一个 absolute pill 平滑滑过去，而不是每个按钮各自瞬切背景色。
  const navItemRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const [pillRect, setPillRect] = useState<{ top: number; height: number } | null>(null);
  useLayoutEffect(() => {
    const idx = NAV.findIndex(n => n.id === currentTab);
    const el = navItemRefs.current[idx];
    if (!el) return;
    setPillRect({ top: el.offsetTop, height: el.offsetHeight });
  }, [currentTab, NAV]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      if (await isMainWindowStartHidden()) {
        return;
      }
      const credentials = await getCredentials();
      const promptDeferredValue = window.sessionStorage.getItem(PROVIDER_SETUP_PROMPT_DEFERRED_KEY);
      if (!cancelled && shouldShowProviderSetupPrompt(credentials, promptDeferredValue)) {
        setProviderPromptOpen(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (os !== 'win' || !prefs) return;
    let cancelled = false;
    (async () => {
      if (await isMainWindowStartHidden()) {
        return;
      }
      const acknowledgedValue = window.localStorage.getItem(BLE_PAIRING_PROMPT_ACK_KEY);
      const deferredValue = window.sessionStorage.getItem(BLE_PAIRING_PROMPT_DEFERRED_KEY);
      if (
        !cancelled &&
        acknowledgedValue !== '1' &&
        deferredValue !== '1' &&
        (prefs.dictationInputSource ?? 'embeddedBle') !== 'embeddedBle'
      ) {
        setBlePairingPromptOpen(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [os, prefs?.dictationInputSource]);

  // 之前监听的模型设置跳转事件已无意义——「模型设置」独立 tab 已下线，
  // 模型管理 UI 现在通过 Settings → Advanced 的 <LocalAsr /> 渲染，
  // 用户在 Settings 内即可一站式管理，无需跨页跳转。

  const rememberProviderPrompt = () => {
    window.sessionStorage.setItem(PROVIDER_SETUP_PROMPT_DEFERRED_KEY, '1');
    setProviderPromptOpen(false);
  };


  const deferBlePairingPrompt = () => {
    window.sessionStorage.setItem(BLE_PAIRING_PROMPT_DEFERRED_KEY, '1');
    setBlePairingPromptOpen(false);
  };

  const keepMicrophoneFromBlePrompt = () => {
    window.localStorage.setItem(BLE_PAIRING_PROMPT_ACK_KEY, '1');
    setBlePairingPromptOpen(false);
  };

  const openSettings = (section?: SettingsSectionId) => {
    setSettingsInitialSection(section);
    setSettingsOpen(true);
  };

  useEffect(() => {
    if (!import.meta.env.DEV) return;
    const section = new URLSearchParams(window.location.search).get('openSettings');
    if (section && DEV_SETTINGS_SECTIONS.includes(section as SettingsSectionId)) {
      openSettings(section as SettingsSectionId);
    }
  }, []);

  const openDeviceKeyAppPage = (page: DeviceCustomKeyAppPage) => {
    const settingsPages: Partial<Record<DeviceCustomKeyAppPage, SettingsSectionId>> = {
      settingsDevice: 'device',
      settingsRecording: 'recording',
      settingsProviders: 'providers',
      settingsShortcuts: 'shortcuts',
      settingsPermissions: 'permissions',
      settingsLanguage: 'language',
      settingsAdvanced: 'advanced',
    };
    const settingsSection = settingsPages[page];
    if (settingsSection) {
      openSettings(settingsSection);
      return;
    }
    const appTabs: Partial<Record<DeviceCustomKeyAppPage, AppTab>> = {
      overview: 'overview',
      history: 'history',
      vocab: 'vocab',
      style: 'style',
      translation: 'translation',
      selectionAsk: 'selectionAsk',
    };
    const tab = appTabs[page] ?? 'overview';
    setSettingsOpen(false);
    setCurrentTab(tab);
  };

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    listen<DeviceCustomKeyAppPage>('device-key:open-app-page', event => {
      openDeviceKeyAppPage(event.payload);
    }).then(fn => {
      // effect 已卸载但 listen promise 才 resolve：立即销毁句柄，避免订阅泄漏。
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    }).catch(error => {
      console.warn('[device-key] open app page listener setup failed', error);
    });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
    // openSettings only wraps stable React setters.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ⌘, 打开设置页面
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.metaKey && e.key === ',') {
        e.preventDefault();
        openSettings();
      }
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, []);

  const openProviderSettings = () => {
    rememberProviderPrompt();
    openSettings('providers');
  };

  const openRecordingSettingsFromProviderPrompt = () => {
    rememberProviderPrompt();
    openSettings('recording');
  };

  const startDemoMode = (source: DemoModeSession['source']) => {
    setProviderPromptOpen(false);
    setBlePairingPromptOpen(false);
    setDemoModeSession(createDemoModeSession(source));
    setCurrentTab('overview');
  };

  const startProviderDemoMode = () => {
    startDemoMode('provider');
  };

  const startDeviceDemoMode = () => {
    startDemoMode('device');
  };

  const openDemoProviderSettings = () => {
    setDemoModeSession(null);
    openSettings('providers');
  };

  const openDemoRecordingSettings = () => {
    setDemoModeSession(null);
    openSettings('recording');
  };


  const enableEmbeddedBleInputFromPrompt = async () => {
    if (prefs && (prefs.dictationInputSource ?? 'embeddedBle') !== 'embeddedBle') {
      await updatePrefs({
        ...prefs,
        dictationInputSource: 'embeddedBle',
        dictationInputSourceUserOverridden: true,
      }).catch(error => {
        console.warn('[ble-pairing] failed to switch input source', error);
      });
    }
  };

  const completeBlePairingPrompt = () => {
    window.localStorage.setItem(BLE_PAIRING_PROMPT_ACK_KEY, '1');
    setBlePairingPromptOpen(false);
  };

  return (
    <div style={{ flex: 1, position: 'relative', display: 'flex', flexDirection: 'column', minHeight: 0, paddingTop: os === 'mac' ? 28 : 0 }}>
      <div
        style={{
          position: 'absolute',
          top: os === 'mac' ? 34 : 8,
          right: 16,
          zIndex: 5,
        }}
      >
        <FooterIcon name="settings" tip={t('shell.footer.settings')} active={settingsOpen} onClick={() => openSettings()} />
      </div>

      {/* Main shell — flush with the frosted backplate (no separate float). */}
      <div
        style={{
          flex: 1, minHeight: 0,
          display: 'flex',
          background: 'transparent',
          overflow: 'hidden',
          position: 'relative',
          zIndex: 1,
        }}>

        {/* Sidebar — 透明地坐在外层磨砂底板上，让 LOGO/导航/快捷键/BETA/footer 共用同一片磨砂玻璃 */}
        <aside
          style={{
            width: 188,
            flexShrink: 0,
            display: 'flex', flexDirection: 'column',
            background: 'transparent',
            padding: '10px 10px 12px',
          }}>

          {/* brand */}
          <div style={{ display: 'flex', alignItems: 'center', gap: 9, padding: '2px 8px 12px' }}>
            <img
              src="AppIcon.png"
              alt="Listener Type"
              style={{ width: 22, height: 22, borderRadius: 5, boxShadow: '0 1px 2px rgba(0,0,0,.1), 0 0 0 0.5px rgba(0,0,0,.06)' }} />

            <div style={{ fontSize: 13.5, fontWeight: 600, letterSpacing: 0, color: 'var(--ol-ink)' }}>Listener Type</div>
          </div>

          {/* nav — 滑动指示器：active pill 是 absolute 元素，currentTab 改变时 top/height
              过渡到目标按钮的位置，而非各按钮自己瞬切背景色。hover 灰底通过 .ol-nav-btn 的
              CSS :hover 规则实现，仅对非 active 项生效。 */}
          <nav style={{ position: 'relative', display: 'flex', flexDirection: 'column', gap: 1 }}>
            {pillRect && (
              <div
                aria-hidden
                style={{
                  position: 'absolute',
                  left: 0,
                  right: 0,
                  top: pillRect.top,
                  height: pillRect.height,
                  background: 'var(--ol-surface)',
                  borderRadius: 8,
                  boxShadow: '0 1px 2px rgba(0,0,0,.05), 0 0 0 0.5px rgba(0,0,0,.06)',
                  transition: 'top 0.36s var(--ol-motion-spring), height 0.36s var(--ol-motion-spring)',
                  pointerEvents: 'none',
                  zIndex: 0,
                }}
              />
            )}
            {NAV.map((n, i) => {
              const active = currentTab === n.id;
              return (
                <button
                  key={n.id}
                  ref={el => { navItemRefs.current[i] = el; }}
                  onClick={() => setCurrentTab(n.id)}
                  className={active ? 'ol-nav-btn ol-nav-btn-active' : 'ol-nav-btn'}
                  style={{
                    display: 'flex', alignItems: 'center', gap: 10,
                    padding: '7px 10px',
                    borderRadius: 8, border: 0,
                    background: 'transparent',
                    fontFamily: 'inherit', fontSize: 13,
                    cursor: 'default',
                    transition: 'color 0.16s var(--ol-motion-quick), background 0.16s var(--ol-motion-quick)',
                    textAlign: 'left',
                    position: 'relative',
                    zIndex: 1,
                  }}>

                  <Icon name={n.icon} size={14} />
                  <span style={{ flex: 1 }}>{n.name}</span>
                </button>
              );
            })}
          </nav>

          <div style={{ flex: 1 }} />
        </aside>

        {/* Main content — v1.3.1-8 用户希望"sidebar / main panel / caption 三处玻璃统一"。
            从 var(--ol-surface) 不透明白底 改成半透明白 + backdrop-filter，跟 sidebar
            一起坐在 WindowChrome 的磨砂底板上，整体一块连续玻璃。 */}
        <div style={{ flex: 1, minWidth: 0, padding: '4px 8px 6px 0', display: 'flex' }}>
          <main
            style={{
              flex: 1, minWidth: 0,
              overflow: 'hidden',
              background: 'var(--ol-glass-bg)',
              backdropFilter: 'blur(18px) saturate(170%)',
              WebkitBackdropFilter: 'blur(18px) saturate(170%)',
              borderRadius: 'var(--ol-window-console-radius)',
              border: '0.5px solid var(--ol-line)',
              boxShadow: '0 1px 0 var(--ol-line-soft) inset, 0 8px 24px -12px rgba(15,17,22,0.10), 0 2px 6px -2px rgba(15,17,22,0.06)',
              display: 'flex',
              flexDirection: 'column',
            }}
          >
            {/* key={displayTab} 让每次切换重挂这棵子树 → ol-page-slide keyframe 重新触发。
                旧 tab 退出时不立刻 unmount，而是先播 ol-page-fadeout（blur+淡出），
                180ms 后再切到新 tab 并播入场动画。详见 displayTab/tabPhase 的 effect。
                padding + overflow:auto 直接挂在这棵 wrapper 上：
                  - 自然高度的页（Overview / Vocab / Style）—— 整页内容超出时 wrapper 出现滚动条
                  - 用 height:100% 撑满的页（History 左右双列）—— 100% 能解析到 wrapper 的固定高度，
                    两列内部各自的 overflow:auto 才能独立滚动 */}
            <div
              key={displayTab}
              // issue #243：所有 tab 都允许 overflow:auto，让窗口被压缩 / 文案
              //   变长时仍可触达底部内容（Codex P1：之前 overview 用 hidden
              //   会让缩窗后 Recent 卡彻底不可见）。
              //   - Overview 借 Overview.tsx 内部 flex 把底部行 grow 到撑满，
              //     正常尺寸下内容刚好占满 → 浏览器自动不显示 scrollbar；
              //     真挤不下了才 fallback 出细滚动条。
              //   - 其他 tab 同样走细滚动条。
              className="ol-thinscroll"
              style={{
                flex: 1, minHeight: 0,
                overflow: 'auto',
                padding: '24px 28px 32px',
                // position:relative 让页面里的"已保存"toast 用 absolute top:16 right:16
                // 锚到这块控制台卡的右上角，而不是横在页头变成长横幅。
                position: 'relative',
                // 苹果"spring out"风格的曲线：开始快、收尾顺滑，符合人体直觉
                animation: tabPhase === 'exiting'
                  ? 'ol-page-fadeout 0.18s var(--ol-motion-soft) forwards'
                  : 'ol-page-slide 0.34s var(--ol-motion-spring) both',
                willChange: 'opacity, transform, filter',
                display: 'flex',
                flexDirection: 'column',
              }}
            >
              {demoModeSession ? (
                <DemoModeBanner
                  source={demoModeSession.source}
                  onClose={() => setDemoModeSession(null)}
                  onOpenProviders={openDemoProviderSettings}
                  onOpenRecording={openDemoRecordingSettings}
                />
              ) : null}
              {displayTab === 'overview' ? (
                <Overview
                  onOpenProvidersSettings={() => openSettings('providers')}
                  onOpenRecordingSettings={() => openSettings('recording')}
                  onStartDemoMode={startProviderDemoMode}
                />
              ) : (
                <Page />
              )}
            </div>
          </main>
        </div>
      </div>

      {/* Footer — 透明地坐在外层磨砂底板上，跟 sidebar 同一片磨砂玻璃 */}
      <div
        style={{
          flexShrink: 0,
          height: 44,
          display: 'flex', alignItems: 'center',
          padding: '0 24px',
          gap: 4,
          fontSize: 11,
          color: 'var(--ol-ink-4)',
          position: 'relative',
          zIndex: 2,
        }}>

        <div style={{ flex: 1 }} />

        <span style={{ fontFamily: 'var(--ol-font-sans)', marginRight: 12 }}>{t('shell.footer.version', { version: APP_VERSION_LABEL })}</span>
      </div>

      {/* Settings modal — rendered inside this window */}
      {settingsOpen &&
        <SettingsModal
          key={settingsInitialSection ?? 'default'}
          os={os}
          initialSettingsSection={settingsInitialSection}
          onClose={() => setSettingsOpen(false)}
        />
      }

      {providerPromptOpen ? (
        <ProviderSetupPrompt
          onLater={rememberProviderPrompt}
          onOpenSettings={openProviderSettings}
          onOpenRecording={openRecordingSettingsFromProviderPrompt}
          onStartDemo={startProviderDemoMode}
        />
      ) : blePairingPromptOpen ? (
        <BlePairingPrompt
          onLater={deferBlePairingPrompt}
          onUseMicrophone={keepMicrophoneFromBlePrompt}
          onEnableEmbeddedBle={enableEmbeddedBleInputFromPrompt}
          onComplete={completeBlePairingPrompt}
          onStartDemo={startDeviceDemoMode}
        />
      ) : null}

      {/* tab 切换 + provider prompt + footer popover 公用的入场关键帧 */}
      <style>{`
        /* nav 三段视觉层次：
             基础态  → ink-3（中灰文字 + 透明底）
             hover  → ink（深色文字 + 浅灰底）  ← 让"翻译"等字词在悬停时高亮，跟基础/选中都拉开差距
             选中  → ink（深色文字 + 白色 pill 底，由 absolute pill 提供）
           inline color/fontWeight 留给 active 项写最高优先级；非 active 走 class，
           这样 :hover 能正确覆盖（CSS 不能盖 inline style）。 */
        .ol-nav-btn {
          color: var(--ol-ink-3);
          font-weight: 500;
        }
        .ol-nav-btn.ol-nav-btn-active {
          color: var(--ol-ink);
          font-weight: 600;
        }
        .ol-nav-btn:not(.ol-nav-btn-active):hover {
          background: var(--ol-surface-2);
          color: var(--ol-ink);
        }
        @keyframes ol-page-slide {
          from { opacity: 0; transform: translate3d(10px, 0, 0) scale(.996); filter: blur(6px); }
          to   { opacity: 1; transform: translate3d(0, 0, 0) scale(1); filter: blur(0); }
        }
        @keyframes ol-page-fadeout {
          from { opacity: 1; filter: blur(0); }
          to   { opacity: 0; filter: blur(8px); }
        }
        @keyframes ol-prompt-fade {
          from { opacity: 0; backdrop-filter: blur(0); -webkit-backdrop-filter: blur(0); }
          to   { opacity: 1; backdrop-filter: blur(6px); -webkit-backdrop-filter: blur(6px); }
        }
        @keyframes ol-prompt-pop {
          from { opacity: 0; transform: translateY(6px) scale(.97); filter: blur(6px); }
          to   { opacity: 1; transform: translateY(0) scale(1); filter: blur(0); }
        }
      `}</style>
    </div>
  );
}

function DemoModeBanner({
  source,
  onClose,
  onOpenProviders,
  onOpenRecording,
}: {
  source: DemoModeSession['source'];
  onClose: () => void;
  onOpenProviders: () => void;
  onOpenRecording: () => void;
}) {
  const { t } = useTranslation();
  const providerDemo = source === 'provider';
  return (
    <div
      role="status"
      aria-live="polite"
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        flexWrap: 'wrap',
        gap: 12,
        padding: '10px 12px',
        marginBottom: 12,
        borderRadius: 8,
        border: '0.5px solid rgba(101,123,112,0.22)',
        background: 'rgba(101,123,112,0.08)',
        color: 'var(--ol-ink)',
        flexShrink: 0,
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 10, minWidth: 0, flex: '1 1 260px' }}>
        <div
          style={{
            width: 28,
            height: 28,
            borderRadius: 7,
            background: 'var(--ol-surface)',
            color: 'var(--ol-blue)',
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            flexShrink: 0,
          }}
        >
          <Icon name={providerDemo ? 'sparkle' : 'mic'} size={14} />
        </div>
        <div style={{ minWidth: 0 }}>
          <div style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--ol-ink)', lineHeight: 1.35 }}>
            {t(providerDemo ? 'shell.demoMode.providerTitle' : 'shell.demoMode.deviceTitle')}
          </div>
          <div style={{ fontSize: 11.5, color: 'var(--ol-ink-3)', lineHeight: 1.45 }}>
            {t(providerDemo ? 'shell.demoMode.providerBody' : 'shell.demoMode.deviceBody')}
          </div>
        </div>
      </div>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'flex-end', gap: 8, flexWrap: 'wrap', flex: '0 1 auto' }}>
        {providerDemo ? (
          <button onClick={onOpenProviders} style={promptPrimaryButtonStyle}>
            {t('shell.demoMode.configureProvider')}
          </button>
        ) : (
          <button onClick={onOpenRecording} style={promptPrimaryButtonStyle}>
            {t('shell.demoMode.testAudio')}
          </button>
        )}
        <button onClick={onOpenRecording} style={promptSoftButtonStyle}>
          {t(providerDemo ? 'shell.demoMode.testAudio' : 'shell.demoMode.pairDevice')}
        </button>
        <button
          onClick={onClose}
          aria-label={t('shell.demoMode.close')}
          title={t('shell.demoMode.close')}
          style={{
            width: 28,
            height: 28,
            borderRadius: 7,
            border: '0.5px solid var(--ol-line-strong)',
            background: 'var(--ol-surface)',
            color: 'var(--ol-ink-3)',
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            cursor: 'default',
          }}
        >
          <Icon name="close" size={13} />
        </button>
      </div>
    </div>
  );
}

function BlePairingPrompt({
  onLater,
  onUseMicrophone,
  onEnableEmbeddedBle,
  onComplete,
  onStartDemo,
}: {
  onLater: () => void;
  onUseMicrophone: () => void;
  onEnableEmbeddedBle: () => Promise<void>;
  onComplete: () => void;
  onStartDemo: () => void;
}) {
  const { t } = useTranslation();
  const [probeStatus, setProbeStatus] = useState<EmbeddedBleProbeStatus>('idle');
  const [probeMessage, setProbeMessage] = useState<string | null>(null);
  const [runtime, setRuntime] = useState<EmbeddedBleRuntimeStatus | null>(null);
  const [lastRepairResult, setLastRepairResult] = useState<EmbeddedBleRepairResult | null>(null);
  const [busyAction, setBusyAction] = useState<FirstRunPairingAction | null>(null);

  useEffect(() => {
    let cancelled = false;
    getEmbeddedBleRuntimeStatus()
      .then(value => {
        if (!cancelled) setRuntime(value);
      })
      .catch(error => {
        console.warn('[ble-pairing] failed to load runtime status', error);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const wizard = useMemo(
    () => buildFirstRunPairingWizard({
      supported: true,
      probeStatus,
      probeMessage,
      runtime,
      lastRepairResult,
    }, t),
    [lastRepairResult, probeMessage, probeStatus, runtime, t],
  );

  const refreshRuntime = async () => {
    try {
      const value = await getEmbeddedBleRuntimeStatus();
      setRuntime(value);
    } catch (error) {
      console.warn('[ble-pairing] failed to refresh runtime status', error);
    }
  };

  const runPairingCheck = async () => {
    setBusyAction('startCheck');
    setProbeStatus('checking');
    setProbeMessage(null);
    setLastRepairResult(null);
    try {
      await onEnableEmbeddedBle();
      await refreshRuntime();
      await runEmbeddedBleProbeWithTimeout();
      setProbeStatus('ok');
      setProbeMessage(t('shell.blePairingPrompt.readyBody'));
      await refreshRuntime();
    } catch (error) {
      setProbeStatus('error');
      setProbeMessage(embeddedBleProbeErrorMessage(error, t));
      await refreshRuntime();
    } finally {
      setBusyAction(null);
    }
  };

  const runPairingRepair = async () => {
    setBusyAction('repair');
    setProbeStatus('checking');
    setProbeMessage(null);
    try {
      await onEnableEmbeddedBle();
      const result = await recoverEmbeddedBleDevice(15_000);
      setLastRepairResult(result);
      if (result.runtime) {
        setRuntime(result.runtime);
      } else {
        await refreshRuntime();
      }
      if (result.recovered) {
        setProbeStatus('ok');
        setProbeMessage(t('shell.blePairingPrompt.readyBody'));
      } else {
        setProbeStatus('error');
        setProbeMessage(result.message ?? t('shell.blePairingPrompt.needsRepairBody'));
      }
      if (result.openBluetoothSettings) {
        await openSystemSettings('bluetooth');
      }
    } catch (error) {
      setProbeStatus('error');
      setProbeMessage(embeddedBleProbeErrorMessage(error, t));
      await refreshRuntime();
    } finally {
      setBusyAction(null);
    }
  };

  const openBluetooth = async () => {
    setBusyAction('openBluetooth');
    try {
      await onEnableEmbeddedBle();
      await openSystemSettings('bluetooth');
      await refreshRuntime();
    } catch (error) {
      console.warn('[ble-pairing] failed to open bluetooth settings', error);
    } finally {
      setBusyAction(null);
    }
  };

  const handleAction = (action: FirstRunPairingAction) => {
    if (busyAction) return;
    if (action === 'startCheck' || action === 'retry') {
      void runPairingCheck();
      return;
    }
    if (action === 'repair') {
      void runPairingRepair();
      return;
    }
    if (action === 'openBluetooth') {
      void openBluetooth();
      return;
    }
    onComplete();
  };

  const primaryDisabled = busyAction !== null;
  const secondaryAction = wizard.secondaryAction && wizard.secondaryAction !== wizard.primaryAction
    ? wizard.secondaryAction
    : null;

  return (
    <div
      style={{
        position: 'absolute',
        inset: 0,
        zIndex: 70,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 28,
        background: 'rgba(15,17,22,0.28)',
        backdropFilter: 'blur(6px) saturate(140%)',
        WebkitBackdropFilter: 'blur(6px) saturate(140%)',
        animation: 'ol-prompt-fade 0.2s var(--ol-motion-soft)',
      }}
    >
      <div
        style={{
          width: 500,
          maxWidth: 'calc(100vw - 56px)',
          borderRadius: 12,
          background: 'var(--ol-surface)',
          border: '0.5px solid rgba(0,0,0,.08)',
          boxShadow: '0 24px 70px -24px rgba(15,17,22,.38), 0 0 0 0.5px rgba(0,0,0,.06)',
          padding: 20,
          animation: 'ol-prompt-pop 0.26s var(--ol-motion-spring)',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 12 }}>
          <div
            style={{
              width: 34,
              height: 34,
              borderRadius: 8,
              background: 'rgba(101,123,112,0.10)',
              color: 'var(--ol-blue)',
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              flexShrink: 0,
            }}
          >
            <Icon name="bolt" size={17} />
          </div>
          <div style={{ minWidth: 0 }}>
            <div style={{ fontSize: 14, fontWeight: 600, color: 'var(--ol-ink)' }}>{wizard.title}</div>
            <div style={{ marginTop: 2, fontSize: 11.5, color: toneColor(wizard.tone) }}>
              {t(`shell.blePairingPrompt.tone.${wizard.tone}`)}
            </div>
          </div>
        </div>
        <div style={{ fontSize: 12.5, color: 'var(--ol-ink-3)', lineHeight: 1.55 }}>
          {wizard.message}
        </div>
        <div
          style={{
            marginTop: 16,
            display: 'grid',
            gap: 7,
            padding: 10,
            borderRadius: 8,
            border: '0.5px solid var(--ol-line)',
            background: 'rgba(0,0,0,0.018)',
          }}
        >
          {wizard.stages.map(stage => (
            <div
              key={stage.id}
              style={{
                display: 'grid',
                gridTemplateColumns: '20px minmax(0, 1fr) auto',
                alignItems: 'center',
                gap: 8,
                minHeight: 30,
              }}
            >
              <div
                aria-hidden
                style={{
                  width: 18,
                  height: 18,
                  borderRadius: 999,
                  display: 'inline-flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  border: stage.status === 'pending' ? '0.5px solid var(--ol-line-strong)' : '0',
                  background: stageDotBackground(stage.status),
                  color: stage.status === 'pending' ? 'var(--ol-ink-4)' : '#fff',
                  fontSize: 10,
                  fontWeight: 700,
                }}
              >
                {stageDotText(stage.status)}
              </div>
              <div style={{ minWidth: 0 }}>
                <div style={{ fontSize: 12.5, color: 'var(--ol-ink)', fontWeight: 600, lineHeight: 1.25 }}>
                  {stage.label}
                </div>
                <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.25 }}>
                  {stage.description}
                </div>
              </div>
              <div style={{ fontSize: 11.5, color: stageTextColor(stage.status), whiteSpace: 'nowrap' }}>
                {t(`shell.blePairingPrompt.stageStatus.${stage.status}`)}
              </div>
            </div>
          ))}
        </div>
        <div style={{ fontSize: 12, color: 'var(--ol-ink-4)', lineHeight: 1.5, marginTop: 10 }}>
          {t('shell.blePairingPrompt.demoHint')}
        </div>
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 18, flexWrap: 'wrap' }}>
          <button
            onClick={onLater}
            disabled={primaryDisabled}
            style={promptSecondaryButtonStyle}
          >
            {t('shell.blePairingPrompt.later')}
          </button>
          {wizard.showUseMicrophone && (
            <button
              onClick={onUseMicrophone}
              disabled={primaryDisabled}
              style={promptSoftButtonStyle}
            >
              {t('shell.blePairingPrompt.useMicrophone')}
            </button>
          )}
          {secondaryAction && (
            <button
              onClick={() => handleAction(secondaryAction)}
              disabled={primaryDisabled}
              style={promptSoftButtonStyle}
            >
              {actionLabel(secondaryAction, t)}
            </button>
          )}
          <button
            onClick={onStartDemo}
            disabled={primaryDisabled}
            style={promptSoftButtonStyle}
          >
            {t('shell.blePairingPrompt.demoMode')}
          </button>
          <button
            onClick={() => handleAction(wizard.primaryAction)}
            disabled={primaryDisabled}
            style={promptPrimaryButtonStyle}
          >
            {busyAction ? t('shell.blePairingPrompt.working') : actionLabel(wizard.primaryAction, t)}
          </button>
        </div>
      </div>
    </div>
  );
}

function actionLabel(action: FirstRunPairingAction, t: TFunction): string {
  switch (action) {
    case 'openBluetooth':
      return t('shell.blePairingPrompt.openBluetooth');
    case 'retry':
      return t('shell.blePairingPrompt.retryCheck');
    case 'repair':
      return t('shell.blePairingPrompt.repair');
    case 'done':
      return t('shell.blePairingPrompt.done');
    case 'startCheck':
    default:
      return t('shell.blePairingPrompt.startCheck');
  }
}

function stageDotText(status: FirstRunPairingStageStatus): string {
  if (status === 'done') return '✓';
  if (status === 'error') return '!';
  if (status === 'current') return '•';
  return '';
}

function stageDotBackground(status: FirstRunPairingStageStatus): string {
  if (status === 'done') return 'var(--ol-blue)';
  if (status === 'error') return '#b42318';
  if (status === 'current') return 'var(--ol-ink)';
  return 'transparent';
}

function stageTextColor(status: FirstRunPairingStageStatus): string {
  if (status === 'done') return 'var(--ol-blue)';
  if (status === 'error') return '#b42318';
  if (status === 'current') return 'var(--ol-ink)';
  return 'var(--ol-ink-4)';
}

function toneColor(tone: 'outline' | 'blue' | 'ok' | 'err'): string {
  if (tone === 'ok') return 'var(--ol-blue)';
  if (tone === 'err') return '#b42318';
  if (tone === 'blue') return 'var(--ol-ink)';
  return 'var(--ol-ink-4)';
}

function ProviderSetupPrompt({
  onLater,
  onOpenSettings,
  onOpenRecording,
  onStartDemo,
}: {
  onLater: () => void;
  onOpenSettings: () => void;
  onOpenRecording: () => void;
  onStartDemo: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div
      style={{
        position: 'absolute',
        inset: 0,
        zIndex: 70,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 28,
        background: 'rgba(15,17,22,0.28)',
        backdropFilter: 'blur(6px) saturate(140%)',
        WebkitBackdropFilter: 'blur(6px) saturate(140%)',
        animation: 'ol-prompt-fade 0.2s var(--ol-motion-soft)',
      }}
    >
      <div
        style={{
          width: 430,
          borderRadius: 12,
          background: 'var(--ol-surface)',
          border: '0.5px solid var(--ol-line)',
          boxShadow: '0 24px 70px -24px rgba(15,17,22,.38), 0 0 0 0.5px rgba(0,0,0,.06)',
          padding: 20,
          animation: 'ol-prompt-pop 0.26s var(--ol-motion-spring)',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 12 }}>
          <div
            style={{
              width: 34,
              height: 34,
              borderRadius: 8,
              background: 'rgba(101,123,112,0.10)',
              color: 'var(--ol-blue)',
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              flexShrink: 0,
            }}
          >
            <Icon name="settings" size={17} />
          </div>
          <div style={{ fontSize: 14, fontWeight: 600, color: 'var(--ol-ink)' }}>{t('shell.providerPrompt.title')}</div>
        </div>
        <div style={{ fontSize: 12.5, color: 'var(--ol-ink-3)', lineHeight: 1.55 }}>
          {t('shell.providerPrompt.body')}
        </div>
        <div style={{ fontSize: 12, color: 'var(--ol-ink-4)', lineHeight: 1.5, marginTop: 8 }}>
          {t('shell.providerPrompt.demoHint')}
        </div>
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 18, flexWrap: 'wrap' }}>
          <button
            onClick={onLater}
            style={promptSecondaryButtonStyle}
          >
            {t('shell.providerPrompt.later')}
          </button>
          <button
            onClick={onStartDemo}
            style={promptSoftButtonStyle}
          >
            {t('shell.providerPrompt.demoMode')}
          </button>
          <button
            onClick={onOpenRecording}
            style={promptSoftButtonStyle}
          >
            {t('shell.providerPrompt.testAudio')}
          </button>
          <button
            onClick={onOpenSettings}
            style={promptPrimaryButtonStyle}
          >
            {t('shell.providerPrompt.openSettings')}
          </button>
        </div>
      </div>
    </div>
  );
}

const promptSecondaryButtonStyle: CSSProperties = {
  height: 32,
  padding: '0 13px',
  borderRadius: 8,
  border: '0.5px solid var(--ol-line-strong)',
  background: 'var(--ol-surface)',
  color: 'var(--ol-ink-3)',
  fontFamily: 'inherit',
  fontSize: 12.5,
  fontWeight: 500,
  cursor: 'default',
  transition: 'background 0.16s var(--ol-motion-quick), border-color 0.16s var(--ol-motion-quick)',
};

const promptSoftButtonStyle: CSSProperties = {
  height: 32,
  padding: '0 14px',
  borderRadius: 8,
  border: '0.5px solid var(--ol-line-strong)',
  background: 'rgba(101,123,112,0.08)',
  color: 'var(--ol-ink)',
  fontFamily: 'inherit',
  fontSize: 12.5,
  fontWeight: 500,
  cursor: 'default',
  transition: 'background 0.16s var(--ol-motion-quick), transform 0.12s var(--ol-motion-quick)',
};

const promptPrimaryButtonStyle: CSSProperties = {
  height: 32,
  padding: '0 14px',
  borderRadius: 8,
  border: 0,
  background: 'var(--ol-ink)',
  color: '#fff',
  fontFamily: 'inherit',
  fontSize: 12.5,
  fontWeight: 500,
  cursor: 'default',
  transition: 'background 0.16s var(--ol-motion-quick), transform 0.12s var(--ol-motion-quick)',
};

interface FooterIconProps {
  name: string;
  tip: string;
  active?: boolean;
  onClick?: () => void;
}

function FooterIcon({ name, tip, active, onClick }: FooterIconProps) {
  const [hover, setHover] = useState(false);
  // 选中（active）= popover 打开，深灰；hover = 浅灰；其它 = 透明
  const background = active
    ? 'var(--ol-surface-2)'
    : hover
      ? 'var(--ol-line-soft)'
      : 'transparent';
  return (
    <button
      onClick={onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      title={tip}
      style={{
        width: 30, height: 30, borderRadius: 7, border: 0,
        background,
        color: active ? 'var(--ol-ink)' : hover ? 'var(--ol-ink-2)' : 'var(--ol-ink-4)',
        display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
        cursor: 'default',
        transition: 'background 0.16s var(--ol-motion-quick), color 0.16s var(--ol-motion-quick)',
      }}>
      <Icon name={name} size={15} />
    </button>
  );
}
