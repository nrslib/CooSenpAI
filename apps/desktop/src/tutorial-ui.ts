import type { AppSnapshot } from "./types.js";
export function tutorialSettingsHighlight(onboarding: AppSnapshot["onboarding"], section: "persona" | "provider" | "watch"): boolean {
  return onboarding.highlightedSettings?.includes(section) === true;
}
