import { readFile } from 'node:fs/promises';

const source = await readFile(new URL('./windows-installed-device-settings-ui-e2e.py', import.meta.url), 'utf8');

function expect(pattern, message) {
  if (!pattern.test(source)) throw new Error(message);
}

expect(/Input\.dispatchMouseEvent/, 'installed settings E2E must drive visible controls with mouse input');
expect(/Input\.insertText/, 'installed settings E2E must type into the visible number fields');
expect(/ensure_device_settings_card/, 'installed settings E2E must reach the settings card through the normal UI');
expect(/return visible_button_center\(client, text="稍后"\) is not None/, 'installed settings E2E must dismiss the visible startup provider panel without relying on its CSS overlay layout');
expect(/later = visible_button_center\(client, text="稍后"\)/, 'installed settings E2E must click the visible Later action before navigating to Settings');
expect(/entry = wait_for\([\s\S]*main_entry,[\s\S]*20,[\s\S]*main-window UI did not become ready/, 'installed settings E2E must wait for the real main UI instead of racing the first WebView target');
expect(/main-window Settings button did not become visible/, 'installed settings E2E must wait for the visible Settings control after dismissing startup UI');
expect(/\.with_suffix\("\.error\.png"\)/, 'installed settings E2E must retain a screenshot when visible navigation fails');
expect(/input\.scrollIntoView/, 'installed settings E2E must scroll each form field into view before editing');
expect(/button_center\(client, "写入"\)/, 'installed settings E2E must submit through the visible Write button');
expect(/button_center\(client, "读取"\)/, 'installed settings E2E must read back through the visible Read button');
expect(/--restore-from-json/, 'installed settings E2E must support restoring the original personal values');
expect(/--same-name-write/, 'installed settings E2E must exercise a visible same-name write');
expect(/--set-ble-name/, 'installed settings E2E must support a visible exact-name recovery without bypassing Type UI');
expect(/ble_name_changed=false apply_needed=false/, 'same-name E2E must require a Type log proving the BLE-name recovery path stayed idle');
expect(/--different-random-name-roundtrip/, 'installed settings E2E must exercise a visible random BLE-name change and restore');
expect(/start_snapshot = read_device_settings\(client, args\.max_write_ms\)/, 'BLE-name E2E must refresh firmware values before choosing the original name');
expect(/string\.ascii_letters \+ string\.digits/, 'random BLE-name E2E must use the full visible ASCII alphabet');
expect(/secrets\.choice\(alphabet\) for _ in range\(12\)/, 'random BLE-name E2E must generate a 12-character random name');
expect(/output_json\.parent\.mkdir\(parents=True, exist_ok=True\)/, 'BLE-name E2E must create the evidence directory before writing log deltas');
expect(/allow_user_prompt=false/, 'random BLE-name E2E must require silent Type recovery');
expect(/open_settings=false/, 'random BLE-name E2E must reject Windows Settings escalation');
expect(/--max-rename-total-ms/, 'BLE-name E2E must enforce a bounded end-to-end rename recovery time');
expect(/ble_name_write_wait_ms = max\(args\.max_write_ms, args\.max_rename_total_ms\)/, 'BLE-name E2E must let the visible saved confirmation consume the same bounded rename budget instead of incorrectly applying the numeric-settings write limit');
expect(/background listener notify ready/, 'BLE-name E2E must require Type audio notifications to recover after each rename');
expect(/wait_for_listener_notify_ready/, 'BLE-name E2E must wait for the post-pairing notify subscription instead of accepting only UI confirmation');
expect(/pair_indices\[-1\] \+ 1/, 'BLE-name E2E must associate notify-ready evidence with the current rename PairAsync rather than an earlier operation');
expect(/restore_error/, 'random BLE-name E2E must report a failed restore instead of hiding it');
expect(/wait_for_device_settings_write_to_settle/, 'a timed-out rename probe must wait for the in-flight UI write to settle before restoring the user name');
expect(/recovery before restore failed/, 'a failed timeout cleanup must remain visible in the rename evidence');
expect(/numeric_form_values\(typed\)/, 'BLE-name E2E must keep the numeric form parseable while editing the BLE name');

const sameNameEvidence = source.match(/def same_name_log_evidence[\s\S]*?(?=\n\ndef changed_name_log_evidence)/)?.[0] ?? '';
if (!/output_json\.parent\.mkdir\(parents=True, exist_ok=True\)/.test(sameNameEvidence)) {
  throw new Error('same-name E2E must create its evidence directory before writing the log delta');
}

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
