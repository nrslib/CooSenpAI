import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { ThoughtWindow } from "./ThoughtWindow.js";
import "./styles.css";

const root = document.getElementById("root");
if (root === null) throw new Error("Thought root element was not found.");
createRoot(root).render(<StrictMode><ThoughtWindow /></StrictMode>);
