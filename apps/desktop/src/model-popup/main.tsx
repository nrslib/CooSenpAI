import React from "react";
import ReactDOM from "react-dom/client";

import { ModelPopup } from "./ModelPopup.js";
import "../components/CompanionModelControls.css";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode><ModelPopup /></React.StrictMode>,
);
