import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import SettingsWindow from "./settings/SettingsWindow";

const isSettings = new URLSearchParams(window.location.search).get("view") === "settings";
document.body.className = isSettings ? "settings-window" : "widget-window";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isSettings ? <SettingsWindow /> : <App />}
  </React.StrictMode>,
);
