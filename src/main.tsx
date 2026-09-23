import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { Settings } from "./pages/Settings";
import AcceptancePanel from "./pages/AcceptancePanel";
import { HotkeySettingsProvider } from "./state/HotkeySettingsContext";
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
const isAcceptance = windowKind === "acceptance";
const isLocalAsrVisual =
  import.meta.env.DEV && params.get("visual") === "local-asr";
const isSettingsShortcutsVisual =
  import.meta.env.DEV && params.get("visual") === "settings-shortcuts";
const isSettingsDeviceVisual =
  import.meta.env.DEV && params.get("visual") === "settings-device";

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
        {isLocalAsrVisual ? (
          <HotkeySettingsProvider>
            <div className="ol-settings-visual-root" style={{ width: "100%", height: "100%", padding: 24, overflow: "auto", background: "var(--ol-window-bg)" }}>
              <Settings embedded initialSection="advanced" />
            </div>
          </HotkeySettingsProvider>
        ) : isSettingsShortcutsVisual ? (
          <HotkeySettingsProvider>
            <div className="ol-settings-visual-root" style={{ width: "100%", height: "100%", padding: 24, overflow: "auto", background: "var(--ol-window-bg)" }}>
              <Settings embedded initialSection="shortcuts" />
            </div>
          </HotkeySettingsProvider>
        ) : isSettingsDeviceVisual ? (
          <HotkeySettingsProvider>
            <div className="ol-settings-visual-root" style={{ width: "100%", height: "100%", padding: 24, overflow: "auto", background: "var(--ol-window-bg)" }}>
              <Settings embedded initialSection="device" />
            </div>
          </HotkeySettingsProvider>
        ) : isAcceptance ? (
          <AcceptancePanel />
        ) : (
          <App isCapsule={isCapsule} isQa={isQa} />
        )}
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
