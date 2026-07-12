import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { Glance } from "./Glance";
import "./styles.css";

// The desktop tray popover loads `index.html?view=glance` — a compact
// usage+insight surface. Everything else renders the full app.
const view = new URLSearchParams(window.location.search).get("view");

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>{view === "glance" ? <Glance /> : <App />}</React.StrictMode>,
);
