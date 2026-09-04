import type { CooSenpaiConfig } from "./types.js";

export interface Appearance {
  readonly theme: CooSenpaiConfig["ui"]["theme"];
  readonly font: string;
}

interface AppearanceRoot {
  readonly dataset: DOMStringMap;
  readonly style: Pick<CSSStyleDeclaration, "setProperty">;
}

const SYSTEM_FONT = '-apple-system, BlinkMacSystemFont, "Hiragino Sans", sans-serif';

export function fontFamily(value: string): string {
  if (value === "system") return SYSTEM_FONT;
  if (value === "rounded") {
    return '"Hiragino Maru Gothic ProN", "Hiragino Sans", -apple-system, sans-serif';
  }
  if (value === "serif") {
    return '"Hiragino Mincho ProN", "YuMincho", serif';
  }
  if (value === "mono") return '"SF Mono", "Menlo", monospace';
  const escaped = value.replaceAll("\\", "\\\\").replaceAll('"', '\\"');
  return `"${escaped}", ${SYSTEM_FONT}`;
}

export function applyAppearance(
  appearance: Appearance,
  root: AppearanceRoot = document.documentElement,
): void {
  root.dataset.theme = appearance.theme;
  root.style.setProperty("--font-body", fontFamily(appearance.font));
}

export function appearanceFromConfig(config: CooSenpaiConfig): Appearance {
  return { theme: config.ui.theme, font: config.ui.font };
}
