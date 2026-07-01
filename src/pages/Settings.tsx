// Settings.tsx — section dispatcher + shared exports.
// 各 section 组件已拆分到 settings/ 子目录，本文件只保留导航 dispatcher 和
// 跨 section 共享的常量 / re-export。

import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { PageHeader } from './_atoms';
import { RecordingSection } from './settings/RecordingSection';
import { DeviceSection } from './settings/DeviceSection';
import { ProvidersSection } from './settings/ProvidersSection';
import { AdvancedSection } from './settings/AdvancedSection';
import { ShortcutsSection } from './settings/ShortcutsSection';
import { PermissionsSection } from './settings/PermissionsSection';
import { LanguageSection } from './settings/LanguageSection';

export { Toggle } from './settings/shared';
export { AboutUpdateControl } from './settings/AboutUpdateControl';

interface SettingsProps {
  embedded?: boolean;
  initialSection?: SettingsSectionId;
}
// "关于" tab 已移除（内容并入外层 SettingsModal 的 About 页，避免设置内外重复入口）。
export type SettingsSectionId = 'recording' | 'device' | 'providers' | 'shortcuts' | 'permissions' | 'language' | 'advanced';

// 「高级」放最末——本地推理 / 实验性开关都集中到这一栏，避免新手用户在主流程
// 里误开 CPU 推理（之前提案：把 local-qwen3 / foundry-local-whisper 从主 ASR
// 下拉藏进高级）。位置末尾也是「实验性」语义在 macOS 系统偏好里的惯用位置。
const SECTION_ORDER: SettingsSectionId[] = ['device', 'recording', 'providers', 'shortcuts', 'permissions', 'language', 'advanced'];

export function Settings({ embedded = false, initialSection = 'device' }: SettingsProps) {
  const { t } = useTranslation();
  const [section, setSection] = useState<SettingsSectionId>(initialSection);

  useEffect(() => {
    setSection(initialSection);
  }, [initialSection]);

  // 跟 sidebar / SettingsModal 同款滑动 pill：测当前 active section 的 offsetTop/height
  // → 用 absolute pill 平滑滑过去；--ol-motion-spring 是项目里的 Apple 风格 ease-out-quint。
  const sectionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const [pillRect, setPillRect] = useState<{ top: number; height: number } | null>(null);
  useLayoutEffect(() => {
    const idx = SECTION_ORDER.indexOf(section);
    const el = sectionRefs.current[idx];
    if (!el) return;
    setPillRect({ top: el.offsetTop, height: el.offsetHeight });
  }, [section]);

  useEffect(() => {
    if (!import.meta.env.DEV) return;
    const rawScroll = new URLSearchParams(window.location.search).get('settingsScroll');
    if (!rawScroll) return;
    const top = Number(rawScroll);
    if (!Number.isFinite(top)) return;
    const id = window.setTimeout(() => {
      contentRef.current?.scrollTo({ top, behavior: 'auto' });
    }, 250);
    return () => window.clearTimeout(id);
  }, [section]);

  return (
    <>
      {!embedded && (
        <PageHeader
          kicker={t('settings.kicker')}
          title={t('settings.title')}
          desc={t('settings.desc')}
        />
      )}
      {/* embedded（在 SettingsModal 里）模式下：mini-sidebar 固定，仅右栏 scroll。
          外层 flex:1 minHeight:0 让 grid 拿到确定高度；gridTemplateRows: minmax(0, 1fr)
          强制行高等于容器高度，否则 grid 默认 auto rows 会跟内容长，右栏 overflow:auto
          就退化成"没东西需要 scroll"，于是大家照旧一起飘。 */}
      <div
        className="ol-settings-layout"
        style={{
          display: 'grid',
          gridTemplateColumns: embedded ? '120px 1fr' : '160px 1fr',
          gap: 18,
          ...(embedded ? { flex: 1, minHeight: 0, gridTemplateRows: 'minmax(0, 1fr)' } : {}),
        }}
      >
        <div className="ol-settings-nav" style={{ position: 'relative', display: 'flex', flexDirection: 'column', gap: 2 }}>
          {pillRect && (
            <div
              aria-hidden
              style={{
                position: 'absolute',
                left: 0,
                right: 0,
                top: pillRect.top,
                height: pillRect.height,
                background: 'var(--ol-control-track)',
                borderRadius: 8,
                transition: 'top 0.36s var(--ol-motion-spring), height 0.36s var(--ol-motion-spring)',
                pointerEvents: 'none',
                zIndex: 0,
              }}
            />
          )}
          {SECTION_ORDER.map((s, i) => {
            const active = section === s;
            return (
              <button
                key={s}
                ref={el => { sectionRefs.current[i] = el; }}
                onClick={() => setSection(s)}
                className={active ? 'ol-nav-btn ol-nav-btn-active' : 'ol-nav-btn'}
                style={{
                  padding: '8px 12px', textAlign: 'left',
                  fontSize: 13,
                  background: 'transparent',
                  border: 0, borderRadius: 8, fontFamily: 'inherit',
                  cursor: 'default',
                  position: 'relative',
                  zIndex: 1,
                  transition: 'color 0.16s var(--ol-motion-quick), background 0.16s var(--ol-motion-quick)',
                }}
              >
                {t(`settings.sections.${s}`)}
              </button>
            );
          })}
        </div>
        <div
          ref={contentRef}
          className={embedded ? 'ol-thinscroll' : undefined}
          style={{
            display: 'flex',
            flexDirection: 'column',
            gap: 12,
            // paddingBottom: 滚到底时让最后一张 Card / Collapsible 的 border + box-shadow
            // 不被滚动容器底边吃掉。16 跟 var(--ol-shadow-sm) 的扩散距离 + Card chrome 留白
            // 匹配，视觉上跟顶部 toolbar 的呼吸感对齐。
            ...(embedded ? { minHeight: 0, overflow: 'auto', paddingRight: 4, paddingBottom: 16 } : {}),
          }}
        >
          {section === 'recording' && <RecordingSection />}
          {section === 'device' && <DeviceSection />}
          {section === 'providers' && <ProvidersSection />}
          {section === 'shortcuts' && <ShortcutsSection />}
          {section === 'permissions' && <PermissionsSection />}
          {section === 'language' && <LanguageSection />}
          {section === 'advanced' && <AdvancedSection />}
        </div>
      </div>
    </>
  );
}
