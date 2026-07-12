import { readFile } from 'node:fs/promises';

const source = await readFile(new URL('./windows-installed-device-settings-ui-e2e.py', import.meta.url), 'utf8');

function expect(pattern, message) {
  if (!pattern.test(source)) throw new Error(message);
}

expect(/Input\.dispatchMouseEvent/, 'installed settings E2E must drive visible controls with mouse input');
expect(/Input\.insertText/, 'installed settings E2E must type into the visible number fields');
expect(/ensure_device_settings_card/, 'installed settings E2E must reach the settings card through the normal UI');
expect(/input\.scrollIntoView/, 'installed settings E2E must scroll each form field into view before editing');
expect(/button_center\(client, "写入"\)/, 'installed settings E2E must submit through the visible Write button');
expect(/button_center\(client, "读取"\)/, 'installed settings E2E must read back through the visible Read button');
expect(/--restore-from-json/, 'installed settings E2E must support restoring the original personal values');

for (const forbidden of ['set_device_settings', 'get_device_settings', 'invoke_retry']) {
  if (source.includes(forbidden)) {
    throw new Error(`installed settings E2E must not bypass the UI with ${forbidden}`);
  }
}

const replaceFocusedText = source.match(/def replace_focused_text[\s\S]*?(?=\n    def screenshot)/)?.[0] ?? '';
if (replaceFocusedText.includes('"Enter"')) {
  throw new Error('typing a field must not press Enter and accidentally write a partial form');
}

console.log('PASS: installed device-settings UI E2E keeps one visible write/read path without direct settings invokes');
