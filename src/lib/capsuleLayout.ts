import type { OS } from '../components/WindowChrome';

export type CapsuleMessageKind = 'default' | 'processing' | 'error';

export interface CapsulePillMetrics {
  width: number;
  height: number;
  textWidth: number;
  boxSizing: 'border-box' | 'content-box';
}

export interface CapsuleHostMetrics {
  width: number;
  height: number;
  horizontalInset: number;
  bottomInset: number;
  badgeGap: number;
  boxSizing: 'border-box' | 'content-box';
}

export interface CapsuleMessageLayout {
  allowWrap: boolean;
  lineClamp: number;
}

const PROCESSING_ICON_WIDTH = 17;
// Both status badges fit a fixed 13px line, 6px padding and rounded borders.
// Reserve their full painted box in the native host, including top shadow room.
export const CAPSULE_BADGE_HEIGHT = 21;
export const CAPSULE_BADGE_STACK_STEP = 26;

export function getCapsulePillMetrics(os: OS): CapsulePillMetrics {
  if (os === 'win') {
    // Windows metrics describe the visible outer footprint of the pill.
    // Runtime window bounds reserve 12px side insets around the pill, so
    // frontend layout must match the native capsule sizing contract.
    return { width: 280, height: 52, textWidth: 192, boxSizing: 'border-box' };
  }

  return { width: 176, height: 42, textWidth: 84, boxSizing: 'border-box' };
}

export function getCapsuleProcessingTextMaxWidth(os: OS): number {
  const metrics = getCapsulePillMetrics(os);
  return Math.max(0, metrics.textWidth - PROCESSING_ICON_WIDTH);
}

// macOS 走 1.2.11 calc 布局，不依赖 host metrics；Windows 端要更大的 host
// 装下阴影 inset，仍用这一份。
export function getCapsuleHostMetrics(
  os: OS,
  translationActive: boolean,
): CapsuleHostMetrics {
  if (os === 'win') {
    const horizontalInset = 12;
    const pill = getCapsulePillMetrics(os);
    const bottomInset = 12;
    const badgeGap = 8;
    const topInset = 12;
    return {
      width: pill.width + horizontalInset * 2,
      height: pill.height + bottomInset + badgeGap + CAPSULE_BADGE_HEIGHT + topInset
        + (translationActive ? CAPSULE_BADGE_STACK_STEP : 0),
      horizontalInset,
      bottomInset,
      badgeGap,
      boxSizing: 'border-box',
    };
  }
  return {
    width: 176,
    height: 42,
    horizontalInset: 0,
    bottomInset: 0,
    badgeGap: 8,
    boxSizing: 'border-box',
  };
}

export function getCapsuleMessageLayout(
  os: OS,
  kind: CapsuleMessageKind,
): CapsuleMessageLayout {
  if (os === 'win' && (kind === 'error' || kind === 'processing')) {
    return { allowWrap: true, lineClamp: 2 };
  }

  return { allowWrap: false, lineClamp: 1 };
}
