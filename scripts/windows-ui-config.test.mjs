import { readFile } from 'node:fs/promises';

function assertEqual(actual, expected, name) {
  if (actual !== expected) {
    throw new Error(`${name}: expected ${expected}, got ${actual}`);
  }
}

function assertMatch(source, pattern, name) {
  if (!pattern.test(source)) {
    throw new Error(`${name}: pattern ${pattern} not found`);
  }
}

const raw = await readFile(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf-8');
const config = JSON.parse(raw);
const capsuleWindow = config.app.windows.find((window) => window.label === 'capsule');
const mainWindow = config.app.windows.find((window) => window.label === 'main');
const libRs = await readFile(new URL('../src-tauri/src/lib.rs', import.meta.url), 'utf-8');
const coordinatorRs = await readFile(new URL('../src-tauri/src/coordinator.rs', import.meta.url), 'utf-8');
const mainTsx = await readFile(new URL('../src/main.tsx', import.meta.url), 'utf-8');
const errorBoundaryTsx = await readFile(new URL('../src/components/ErrorBoundary.tsx', import.meta.url), 'utf-8');
const capsuleTsx = await readFile(new URL('../src/components/Capsule.tsx', import.meta.url), 'utf-8');
const capsuleLayoutTs = await readFile(new URL('../src/lib/capsuleLayout.ts', import.meta.url), 'utf-8');
const windowChromeTsx = await readFile(new URL('../src/components/WindowChrome.tsx', import.meta.url), 'utf-8');
const floatingShellTsx = await readFile(new URL('../src/components/FloatingShell.tsx', import.meta.url), 'utf-8');
const deviceSectionTsx = await readFile(new URL('../src/pages/settings/DeviceSection.tsx', import.meta.url), 'utf-8');
const ipcTs = await readFile(new URL('../src/lib/ipc.ts', import.meta.url), 'utf-8');
const tokensCss = await readFile(new URL('../src/styles/tokens.css', import.meta.url), 'utf-8');

if (!capsuleWindow) {
  throw new Error('capsule window config missing');
}
if (!mainWindow) {
  throw new Error('main window config missing');
}
assertEqual(capsuleWindow.width, 220, 'windows capsule config keeps translation-capable width baseline');
assertEqual(capsuleWindow.height, 110, 'windows capsule config keeps translation-capable height baseline');
assertEqual(capsuleWindow.transparent, true, 'capsule window should keep transparent visuals');
assertEqual(capsuleWindow.alwaysOnTop, true, 'capsule window should stay above the focused app while recording');
assertEqual(mainWindow.url, 'index.html', 'main window should explicitly load the frontend entry instead of relying on platform defaults');
assertEqual(mainWindow.decorations, true, 'shared main window config should keep the native OS window chrome');
assertEqual(mainWindow.visible, false, 'windows main window should stay hidden until the intended first show point');

assertMatch(
  mainTsx,
  /import \{ ErrorBoundary \} from "\.\/components\/ErrorBoundary";/,
  'frontend entry should import the WebView blank-screen error boundary',
);
assertMatch(
  mainTsx,
  /<ErrorBoundary>[\s\S]*<App isCapsule=\{isCapsule\} isQa=\{isQa\} \/>[\s\S]*<\/ErrorBoundary>/,
  'frontend entry should render the app inside ErrorBoundary so React failures do not leave a blank WebView',
);
assertMatch(
  mainTsx,
  /record_ui_timeline_event[\s\S]*source:\s*"frontend\.startup"[\s\S]*event/,
  'frontend startup should keep writing render telemetry for blank-window triage',
);
assertMatch(
  mainTsx,
  /reportStartup\("waiting-for-i18n"[\s\S]*i18n\.on\("initialized", \(\) => renderApp\("i18n-initialized-event"\)\);[\s\S]*window\.setTimeout\(\(\) => \{[\s\S]*renderApp\("i18n-timeout-fallback"\);[\s\S]*\}, 1500\);/,
  'frontend startup should keep a bounded i18n fallback instead of waiting forever before first render',
);
assertMatch(
  errorBoundaryTsx,
  /static getDerivedStateFromError\(error: unknown\)/,
  'ErrorBoundary should catch React render failures',
);
assertMatch(
  errorBoundaryTsx,
  /listen<\{ message: string \}>\('panic:error'/,
  'ErrorBoundary should listen for Rust panic events',
);
assertMatch(
  errorBoundaryTsx,
  /内部错误[\s\S]*页面出错[\s\S]*重试/,
  'ErrorBoundary should show a visible recoverable error surface instead of an empty page',
);
assertMatch(
  libRs,
  /std::panic::set_hook[\s\S]*handle\.emit\("panic:error", payload\)/,
  'Rust panic hook should emit panic:error so the frontend can replace a blank WebView with an error surface',
);

assertMatch(
  libRs,
  /#\[cfg\(target_os = "windows"\)\]\s*\{[\s\S]*?apply_mica\(&main, None\)[\s\S]*?apply_windows_caption_color_for_theme\(&main,\s*coordinator\.prefs\(\)\.get\(\)\.dark_mode\);/,
  'windows runtime should keep native chrome while applying Mica and caption color',
);

assertMatch(
  coordinatorRs,
  /#\[cfg\(target_os = "macos"\)\][\s\S]*?orderFrontRegardless/,
  'macOS capsule should show without taking the key window',
);

assertMatch(
  windowChromeTsx,
  /const MAC_TITLEBAR_HEIGHT = 28;/,
  'macOS titlebar spacer should stay visually compact around the native traffic lights',
);
assertMatch(
  windowChromeTsx,
  /const titlebarHeight = os === 'mac' \? MAC_TITLEBAR_HEIGHT : 0;/,
  'windows native chrome should not reserve a custom React titlebar spacer',
);
assertMatch(
  windowChromeTsx,
  /boxShadow: os === 'win' \? 'none' : 'var\(--ol-shadow-xl\)'/,
  'windows native chrome should own the outer shell shadow',
);
assertMatch(
  windowChromeTsx,
  /border: os === 'win' \? 'none' : os === 'mac' \? 'none' : '0\.5px solid rgba\(0,0,0,\.10\)'/,
  'windows native chrome should own the outer shell border',
);
assertMatch(
  libRs,
  /show_main_window[\s\S]*?set_focus\(\)/,
  'macOS main window should rely on native traffic lights instead of manually moving standardWindowButton frames',
);
if (/standardWindowButton|setFrameOrigin: origin|tune_macos_main_window_controls/.test(libRs)) {
  throw new Error('macOS traffic lights should not be manually repositioned; keep native AppKit button frames visible');
}
assertMatch(
  tokensCss,
  /--ol-motion-spring:[\s\S]*?--ol-motion-soft:[\s\S]*?--ol-motion-quick:/,
  'shared motion tokens should drive shell animations and transitions',
);

if (/#\[cfg\(target_os = "windows"\)\][\s\S]*?main\.set_decorations\(false\)/.test(libRs)) {
  throw new Error('windows main window should not switch to a custom frameless chrome path');
}

if (!/borderRadius:\s*'var\(--ol-window-console-radius\)'/.test(floatingShellTsx)) {
  throw new Error('floating shell should consume the shared window-console radius');
}

assertMatch(
  deviceSectionTsx,
  /restoreDefaultDeviceKeyboardOutput[\s\S]*key1:\s*defaultDeviceKeyboardKey\('RightControl'\)[\s\S]*key2:\s*defaultDeviceKeyboardKey\('C',\s*\['ctrl'\]\)[\s\S]*key3:\s*defaultDeviceKeyboardKey\('V',\s*\['ctrl'\]\)[\s\S]*key4:\s*defaultDeviceKeyboardKey\('Z',\s*\['ctrl'\]\)[\s\S]*knob:\s*defaultDeviceDictationKey\(\)/,
  'device custom key defaults should remain KEY1 Ctrl, KEY2 Ctrl+C, KEY3 Ctrl+V, KEY4 Ctrl+Z, EC11 click dictation',
);
assertMatch(
  ipcTs,
  /deviceCustomKeys:[\s\S]*key1:\s*defaultDeviceKeyboardKey\('RightControl'\)[\s\S]*key2:\s*defaultDeviceKeyboardKey\('C',\s*\['ctrl'\]\)[\s\S]*key3:\s*defaultDeviceKeyboardKey\('V',\s*\['ctrl'\]\)[\s\S]*key4:\s*defaultDeviceKeyboardKey\('Z',\s*\['ctrl'\]\)[\s\S]*knob:\s*\{\s*action:\s*'dictation'/,
  'mock device custom key defaults should keep EC11 click as recording',
);
assertMatch(
  deviceSectionTsx,
  /mapping\.action === 'sendShortcut'[\s\S]*<ShortcutRecorder[\s\S]*value=\{shortcut\}/,
  'device send-key UI should use the recorder as its single key-entry control',
);
for (const forbidden of ['DEVICE_KEYBOARD_PRIMARY_OPTIONS', 'DEVICE_SHORTCUT_MODIFIERS', 'deviceShortcutWithPrimary', 'deviceShortcutWithModifier']) {
  if (deviceSectionTsx.includes(forbidden)) {
    throw new Error(`device send-key UI should not bring back the extra selector control: ${forbidden}`);
  }
}

assertMatch(
  coordinatorRs,
  /let visible = !matches!\(state,\s*CapsuleState::Idle\);/,
  'capsule should stay visible until the unified idle hide path runs',
);
assertMatch(
  coordinatorRs,
  /fn hide_capsule_window_if_present\(\)/,
  'windows capsule lifecycle should include an explicit native hide helper',
);
assertMatch(
  coordinatorRs,
  /ShowWindow\(hwnd, SW_HIDE\)/,
  'windows capsule hide helper should force the native window hidden',
);
assertMatch(
  coordinatorRs,
  /SetWindowPos\([\s\S]*?HWND_NOTOPMOST[\s\S]*?SWP_HIDEWINDOW/m,
  'windows capsule hide helper should drop topmost participation when inactive',
);

if (!/export function getCapsuleHostMetrics\(\s*os: OS,\s*translationActive: boolean,\s*\): CapsuleHostMetrics/.test(capsuleLayoutTs)) {
  throw new Error('capsule layout should define explicit host metrics separate from the visible pill metrics');
}

if (!/if \(os === 'win'\)\s*\{[\s\S]*?const horizontalInset = 12;[\s\S]*?const pill = getCapsulePillMetrics\(os\);[\s\S]*?width: pill\.width \+ horizontalInset \* 2,[\s\S]*?height: translationActive \? 118 : 84,[\s\S]*?horizontalInset,[\s\S]*?bottomInset: 12,[\s\S]*?badgeGap: 8,[\s\S]*?boxSizing: 'border-box',[\s\S]*?\}/.test(capsuleLayoutTs)) {
  throw new Error('windows capsule host metrics should leave room for shadow and badge geometry');
}

if (!/const hostMetrics = getCapsuleHostMetrics\(os,\s*translation\);/.test(capsuleTsx)) {
  throw new Error('capsule should derive host metrics from the shared layout contract');
}

if (!/return\s*\(\s*<div\s*style=\{\{[\s\S]*?width:\s*'100%',[\s\S]*?height:\s*'100%',[\s\S]*?position:\s*'relative',[\s\S]*?display:\s*'flex',[\s\S]*?alignItems:\s*'center',[\s\S]*?justifyContent:\s*'center',[\s\S]*?paddingLeft:\s*hostMetrics\.horizontalInset,[\s\S]*?paddingRight:\s*hostMetrics\.horizontalInset,[\s\S]*?\}\}/.test(capsuleTsx)) {
  throw new Error('capsule host should center the pill within the shared layout contract');
}

if (!/paddingLeft:\s*hostMetrics\.horizontalInset,/.test(capsuleTsx) || !/paddingRight:\s*hostMetrics\.horizontalInset,/.test(capsuleTsx)) {
  throw new Error('windows capsule host should reserve shared horizontal inset room for shadow geometry');
}

if (!/paddingBottom:\s*os === 'win' \? hostMetrics\.bottomInset : 0/.test(capsuleTsx)) {
  throw new Error('windows capsule host should respect the shared bottom inset');
}

if (!/hostMetrics\.bottomInset \+ metrics\.height \+ hostMetrics\.badgeGap/.test(capsuleTsx)) {
  throw new Error('windows translation badge should anchor from the shared host inset instead of a fixed center-based offset');
}

if (!/#\[cfg\(target_os = "windows"\)\][\s\S]*?const WINDOWS_CAPSULE_PILL_WIDTH: f64 = 280\.0;[\s\S]*?const WINDOWS_CAPSULE_SIDE_INSET: f64 = 12\.0;[\s\S]*?width: WINDOWS_CAPSULE_PILL_WIDTH \+ WINDOWS_CAPSULE_SIDE_INSET \* 2\.0,[\s\S]*?height: if translation_active \{ 118\.0 \} else \{ 84\.0 \},[\s\S]*?bottom_inset: 12\.0,/.test(libRs)) {
  throw new Error('windows runtime capsule bounds should leave room for the native shadow while keeping a fixed visual pill');
}

if (!/#\[cfg\(target_os = "windows"\)\]\s*\{\s*52\.0\s*\}/.test(libRs)) {
  throw new Error('windows capsule visual pill height should stay at 52px');
}

if (!/window\.set_size\(LogicalSize::new\(bounds\.width, bounds\.height\)\)\?/.test(libRs)) {
  throw new Error('capsule positioning should resync runtime size with the computed layout');
}

if (!/let _ = window\.hide\(\);/.test(coordinatorRs)) {
  throw new Error('capsule should be hidden once it leaves active states');
}
