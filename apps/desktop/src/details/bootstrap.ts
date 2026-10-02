import { showStartupError } from "./renderer-error-listeners.js";

void import("./main.js").catch(showStartupError);
