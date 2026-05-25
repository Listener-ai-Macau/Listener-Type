export const HOTKEY_MODE_MIGRATION_NOTICE_VERSION = '1.2.7';
export const HOTKEY_MODE_MIGRATION_ACK_KEY = `ol.hotkeyModeMigrationAck:${HOTKEY_MODE_MIGRATION_NOTICE_VERSION}`;
export const HOTKEY_MODE_MIGRATION_DEFERRED_KEY = `ol.hotkeyModeMigrationDeferred:${HOTKEY_MODE_MIGRATION_NOTICE_VERSION}`;

export function isHotkeyModeMigrationNoticeActive(): boolean {
  return false;
}

export function shouldShowHotkeyModeMigrationPrompt(
  acknowledgedValue: string | null,
  deferredValue: string | null,
): boolean {
  return isHotkeyModeMigrationNoticeActive() && acknowledgedValue !== '1' && deferredValue !== '1';
}
