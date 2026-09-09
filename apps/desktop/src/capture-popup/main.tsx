import React from "react";
import ReactDOM from "react-dom/client";

import { CapturePopupView } from "./CapturePopup.js";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode><CapturePopupView /></React.StrictMode>,
);
