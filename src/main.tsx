import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { Settings } from "./pages/Settings";
import { HotkeySettingsProvider } from "./state/HotkeySettingsContext";
import i18n from "./i18n"; // 副作用：触发 i18next init
import "./styles/tokens.css";
import "./styles/global.css";

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
const isSettingsCompanionVisual =
  import.meta.env.DEV && params.get("visual") === "settings-companion";

if (import.meta.env.DEV && params.get("theme") === "dark") {
  document.documentElement.setAttribute("data-theme", "dark");
  window.localStorage.setItem("ol-dark-mode", "true");
}

const root = ReactDOM.createRoot(document.getElementById("root")!);

const renderApp = () => {
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
        ) : isSettingsDeviceVisual || isSettingsCompanionVisual ? (
          <HotkeySettingsProvider>
            <div className="ol-settings-visual-root" style={{ width: "100%", height: "100%", padding: 24, overflow: "auto", background: "var(--ol-window-bg)" }}>
              <Settings embedded initialSection={isSettingsCompanionVisual ? "companion" : "device"} />
            </div>
          </HotkeySettingsProvider>
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
  renderApp();
} else {
  i18n.on("initialized", renderApp);
}
