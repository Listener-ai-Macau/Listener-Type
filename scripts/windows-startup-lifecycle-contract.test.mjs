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
const mainWindow = config.app.windows.find(window => window.label === 'main');
const appTsx = await readFile(new URL('../src/App.tsx', import.meta.url), 'utf-8');
const libRs = await readFile(new URL('../src-tauri/src/lib.rs', import.meta.url), 'utf-8');

if (!mainWindow) {
  throw new Error('main window config missing');
}

assertEqual(mainWindow.visible, false, 'main window should stay hidden until startup contract allows first show');
assertMatch(
  appTsx,
  /const \[gate, setGate\] = useState<Gate>\(isTauri \? 'checking' : 'ready'\);/,
  'desktop app should start in checking gate before claiming ready',
);
assertMatch(
  appTsx,
  /if \(os === 'win' && gate === 'checking'\) return;/,
  'windows should not show the main shell while startup gate is still checking',
);
assertMatch(
  appTsx,
  /const pollHotkeyStatus = async \(\) => \{[\s\S]*?if \(status\.state !== 'starting'\) \{[\s\S]*?setGate\('ready'\);/m,
  'windows startup should wait for hotkey status to leave the starting phase before entering ready',
);
assertMatch(
  libRs,
  /fn restore_main_window_layout_if_needed[\s\S]*set_size\(LogicalSize::new\(1240\.0,\s*800\.0\)\)/,
  'windows desktop-shortcut reopen should restore tiny/offscreen main windows to normal size',
);
assertMatch(
  libRs,
  /fn restore_main_window_native[\s\S]*EnumWindows[\s\S]*GetWindowTextLengthW\(candidate\) > 0[\s\S]*ShowWindow\(candidate,\s*SW_SHOW\)/,
  'windows desktop-shortcut reopen should reveal hidden WebView host windows without expanding titled QA/capsule windows',
);
