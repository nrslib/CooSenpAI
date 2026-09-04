import react from "@vitejs/plugin-react";
import { fileURLToPath, URL } from "node:url";

export default {
  plugins: [react()],
  clearScreen: false,
  server: { host: "127.0.0.1", port: 1420, strictPort: true },
  build: {
    target: "safari13",
    rollupOptions: {
      input: {
        index: fileURLToPath(new URL("./index.html", import.meta.url)),
        bubble: fileURLToPath(new URL("./bubble.html", import.meta.url)),
        capturePopup: fileURLToPath(new URL("./capture-popup.html", import.meta.url)),
        speechPopup: fileURLToPath(new URL("./speech-popup.html", import.meta.url)),
        modelPopup: fileURLToPath(new URL("./model-popup.html", import.meta.url)),
      },
    },
  },
};
