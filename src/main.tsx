import React from "react";
import ReactDOM from "react-dom/client";
import "react-grid-layout/css/styles.css";
import "react-resizable/css/styles.css";
import App from "./App";
import { GateProvider } from "./state";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <GateProvider>
      <App />
    </GateProvider>
  </React.StrictMode>,
);

// PWA: register the service worker when served over http(s) by the backend —
// not inside the Tauri webview and not on the vite dev server.
if (
  "serviceWorker" in navigator &&
  !("__TAURI_INTERNALS__" in window) &&
  !import.meta.env.DEV
) {
  navigator.serviceWorker.register("/sw.js").catch(() => {});
}
