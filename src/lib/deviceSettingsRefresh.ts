import type { DeviceSettingsSnapshot, DeviceSettingsUpdateRequest } from './types';

export type DeviceSettingsUiStatus = 'idle' | 'loading' | 'saving' | 'saved' | 'error';

export function shouldRefreshDeviceSettingsOnFocus(
  status: DeviceSettingsUiStatus,
  snapshot: DeviceSettingsSnapshot | null,
  form: DeviceSettingsUpdateRequest,
  snapshotForm: DeviceSettingsUpdateRequest | null,
): boolean {
  if (status === 'loading' || status === 'saving') return false;
  if (!snapshot || !snapshotForm) return true;
  return JSON.stringify(form) === JSON.stringify(snapshotForm);
}
