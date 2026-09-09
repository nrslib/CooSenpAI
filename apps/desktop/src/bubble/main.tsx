import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { BubbleApp } from "./BubbleApp.js";
import "./styles.css";

const root = document.getElementById("root");
if (root === null) throw new Error("Bubble root element was not found.");
createRoot(root).render(<StrictMode><BubbleApp /></StrictMode>);
