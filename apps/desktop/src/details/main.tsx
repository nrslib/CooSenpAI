import React from "react";
import ReactDOM from "react-dom/client";

import { Details } from "./Details.js";
import { DetailsErrorBoundary } from "./DetailsErrorBoundary.js";
import { hasStartupFailed, markRendererStarted } from "./renderer-error-listeners.js";
import "../styles.css";
import "./styles.css";

if (!hasStartupFailed()) {
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode><DetailsErrorBoundary><Details /></DetailsErrorBoundary></React.StrictMode>,
  );
  markRendererStarted();
}
