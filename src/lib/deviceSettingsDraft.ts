import type { DeviceSettingsUpdateRequest } from './types';

/** One selected device's explicit edits. Defaults and cached names are never writes. */
export class DeviceSettingsDraft {
  private patch: Partial<DeviceSettingsUpdateRequest> = {};
  edit(patch: Partial<DeviceSettingsUpdateRequest>): void {
    this.patch = { ...this.patch, ...patch };
  }
  has(field: keyof DeviceSettingsUpdateRequest): boolean {
    return Object.prototype.hasOwnProperty.call(this.patch, field);
  }
  merge(freshDeviceForm: DeviceSettingsUpdateRequest): DeviceSettingsUpdateRequest {
    return { ...freshDeviceForm, ...this.patch };
  }
}
