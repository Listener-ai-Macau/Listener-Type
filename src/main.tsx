import React from "react";
import ReactDOM from "react-dom/client";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { useDarkMode } from "./state/useDarkMode";
import i18n from "./i18n"; // 副作用：触发 i18next init
import "./styles/tokens.css";
import "./styles/global.css";

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

function reportStartup(event: string, detail?: Record<string, unknown>) {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
    return;
  }
  import("@tauri-apps/api/core")
    .then(({ invoke }) =>
      invoke("record_ui_timeline_event", {
        payload: {
          source: "frontend.startup",
          event,
          detail: detail ?? {},
        },
      }),
    )
    .catch(() => {});
}

const params = new URLSearchParams(window.location.search);
const windowKind = params.get("window");
const isCapsule = windowKind === "capsule";
const isQa = windowKind === "qa";
const isLocalAsrVisual =
  import.meta.env.DEV && params.get("visual") === "local-asr";
const isSettingsShortcutsVisual =
  import.meta.env.DEV && params.get("visual") === "settings-shortcuts";
const isSettingsDeviceVisual =
  import.meta.env.DEV && params.get("visual") === "settings-device";

const visualSection = isLocalAsrVisual ? 'advanced'
  : isSettingsShortcutsVisual ? 'shortcuts'
  : isSettingsDeviceVisual ? 'device' : null;

// Select before importing a window's module. A hidden overlay must not parse
// the main shell, history, settings and onboarding on every renderer startup.
const viewModule = isCapsule
  ? import('./components/Capsule').then(({ Capsule }) => ({ default: Capsule }))
  : isQa
    ? import('./pages/QaPanel').then(({ QaPanel }) => ({ default: QaPanel }))
    : visualSection
      ? import('./pages/SettingsVisual').then(({ SettingsVisual }) => ({
          default: () => <SettingsVisual section={visualSection} />,
        }))
      : import('./App').then(({ App }) => ({ default: App }));
// The import begins while i18n prepares. React's boundary handles rejection
// when rendering; attach a handler meanwhile to avoid an unhandled rejection.
void viewModule.catch(() => {});
const WindowView = React.lazy(() => viewModule);

function ThemedWindow() {
  useDarkMode();
  return <WindowView />;
}

if (import.meta.env.DEV && params.get("theme") === "dark") {
  document.documentElement.setAttribute("data-theme", "dark");
  window.localStorage.setItem("ol-dark-mode", "true");
}

const root = ReactDOM.createRoot(document.getElementById("root")!);

let rendered = false;

const renderApp = (reason: string) => {
  if (rendered) return;
  rendered = true;
  reportStartup("render", {
    reason,
    i18nInitialized: i18n.isInitialized,
    windowKind: windowKind ?? "main",
  });
  root.render(
    <React.StrictMode>
      <ErrorBoundary>
        <React.Suspense fallback={null}>
          {visualSection ? <WindowView /> : <ThemedWindow />}
        </React.Suspense>
      </ErrorBoundary>
    </React.StrictMode>,
  );
};

// i18n 必须就绪后才能渲染：否则首次渲染拿到的 t() 返回 key 字面量。
// react-i18next useSuspense=false 时不会自动等，只有事件触发后重渲染才能拿到译文。
if (i18n.isInitialized) {
  renderApp("i18n-ready");
} else {
  reportStartup("waiting-for-i18n", { windowKind: windowKind ?? "main" });
  i18n.on("initialized", () => renderApp("i18n-initialized-event"));
  window.setTimeout(() => {
    renderApp("i18n-timeout-fallback");
  }, 1500);
}
