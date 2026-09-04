import React from "react";
import ReactDOM from "react-dom/client";

import { CapturePopup } from "./CapturePopup.js";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode><CapturePopup /></React.StrictMode>,
);
