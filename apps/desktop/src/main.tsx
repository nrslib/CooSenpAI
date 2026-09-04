import { createRoot } from "react-dom/client";

import { App } from "./App.js";
import "./components/CompanionModelControls.css";
import "./styles.css";

const root = document.getElementById("root");
if (root === null) throw new Error("rendererのroot要素が見つかりません。");
createRoot(root).render(<App />);
