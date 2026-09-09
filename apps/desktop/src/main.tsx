import { createRoot } from "react-dom/client";

import { App } from "./App.js";
import "./components/CompanionModelControls.css";
import "./styles.css";

const root = document.getElementById("root");
if (root === null) throw new Error("Renderer root element was not found.");
createRoot(root).render(<App />);
