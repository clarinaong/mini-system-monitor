import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import SettingsWindow from "./settings/SettingsWindow";
import HistoryWindow from "./history/HistoryWindow";

const view = new URLSearchParams(window.location.search).get("view");
const Window = view === "history" ? HistoryWindow : view === "settings" ? SettingsWindow : App;
document.body.className = view === "history" ? "history-window" : view === "settings" ? "settings-window" : "widget-window";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Window />
  </React.StrictMode>,
);
