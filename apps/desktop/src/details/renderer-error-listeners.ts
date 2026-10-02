import { invoke } from "@tauri-apps/api/core";

export interface RendererErrorSummary {
  readonly name: string;
  readonly frames: readonly { readonly file: string; readonly line: number }[];
}

let mountGeneration: number | undefined;
const waiting: Array<(generation: number) => void> = [];
let rendererStarted = false;
let startupFailed = false;

window.addEventListener("coosenpai:details:mount-generation", (event) => {
  const generation = (event as CustomEvent<unknown>).detail;
  if (typeof generation !== "number" || !Number.isSafeInteger(generation) || generation < 0) return;
  mountGeneration = generation;
  for (const resolve of waiting.splice(0)) resolve(generation);
});

export function currentMountGeneration(): Promise<number> {
  if (mountGeneration !== undefined) return Promise.resolve(mountGeneration);
  return new Promise((resolve) => waiting.push(resolve));
}

export function markRendererStarted(): void {
  rendererStarted = true;
}

export function hasStartupFailed(): boolean {
  return startupFailed;
}

export function failureText(): { readonly heading: string; readonly reload: string } {
  return navigator.language.toLowerCase().startsWith("ja")
    ? { heading: "詳細画面の読み込みに失敗しました", reload: "再読み込み" }
    : { heading: "Failed to load the details window", reload: "Reload" };
}

export function showStartupError(error: unknown): void {
  if (startupFailed) return;
  startupFailed = true;
  reportRendererError(error);

  const { heading, reload } = failureText();
  const main = document.createElement("main");
  main.className = "details-shell";
  main.setAttribute("role", "alert");
  main.style.padding = "24px";
  const title = document.createElement("h1");
  title.textContent = heading;
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = reload;
  button.addEventListener("click", () => window.location.reload());
  main.append(title, button);
  (document.getElementById("root") ?? document.body).replaceChildren(main);

  void currentMountGeneration()
    .then((generation) => invoke("details_rendered", { payload: { generation, failed: true } }))
    .catch(() => console.error("Failed to report details renderer state"));
}

export function summarizeRendererError(error: unknown): RendererErrorSummary {
  const name = error instanceof Error && /^[A-Za-z0-9]{1,64}$/.test(error.name) ? error.name : "UnknownError";
  const stack = error instanceof Error ? error.stack : undefined;
  const frames: { file: string; line: number }[] = [];
  if (stack !== undefined) {
    for (const entry of stack.split("\n").slice(1)) {
      const match = /(?:^|[/(])([A-Za-z0-9._-]{1,128}):(\d+)(?::\d+)?(?:\)|$)/.exec(entry.trim());
      if (match === null) continue;
      const line = Number(match[2]);
      if (match[1] !== undefined && Number.isSafeInteger(line) && line > 0 && line <= 0xffffffff) frames.push({ file: match[1], line });
      if (frames.length === 8) break;
    }
  }
  return { name, frames };
}

export function reportRendererError(error: unknown): void {
  void invoke("details_renderer_error", { payload: summarizeRendererError(error) })
    .catch(() => console.error("Failed to report details renderer error"));
}

window.addEventListener("error", (event) => {
  event.preventDefault();
  if (rendererStarted) reportRendererError(event.error);
  else showStartupError(event.error);
});
window.addEventListener("unhandledrejection", (event) => {
  event.preventDefault();
  if (rendererStarted) reportRendererError(event.reason);
  else showStartupError(event.reason);
});
