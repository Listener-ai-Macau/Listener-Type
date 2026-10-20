import packageJson from '../../package.json';

export const APP_VERSION = packageJson.version;
export const APP_VERSION_LABEL = `v${APP_VERSION}`;
export const APP_SOURCE_REVISION: string = import.meta.env.VITE_APP_SOURCE_REVISION ?? 'development';
