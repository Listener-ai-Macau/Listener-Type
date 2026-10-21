import { createContext } from 'react';
import type { DeviceCustomKeyAppPage } from '../lib/types';

/** In-app navigation only; no OS input, device writes or external app launch. */
export const DevicePageNavigationContext = createContext<((page: DeviceCustomKeyAppPage) => void) | undefined>(undefined);
