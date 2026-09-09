import type { AppSnapshot } from "./types.js";
import type { TranslationKey } from "./i18n/index.js";

export type SettingsCategory =
  | "work"
  | "general"
  | "vision"
  | "hearing"
  | "speech"
  | "notifications"
  | "providers"
  | "shortcuts"
  | "setup"
  | "beta";

export interface SettingsCategoryDefinition {
  readonly id: SettingsCategory;
  readonly labelKey: TranslationKey;
}

export const SETTINGS_CATEGORIES: readonly SettingsCategoryDefinition[] = [
  { id: "work", labelKey: "settings.categories.work" },
  { id: "general", labelKey: "settings.categories.general" },
  { id: "vision", labelKey: "settings.categories.vision" },
  { id: "hearing", labelKey: "settings.categories.hearing" },
  { id: "speech", labelKey: "settings.categories.speech" },
  { id: "notifications", labelKey: "settings.categories.notifications" },
  { id: "providers", labelKey: "settings.categories.providers" },
  { id: "shortcuts", labelKey: "settings.categories.shortcuts" },
  { id: "setup", labelKey: "settings.categories.setup" },
  { id: "beta", labelKey: "settings.categories.beta" },
];

export function personaSelectionRoute(
  onboarding: Pick<AppSnapshot["onboarding"], "setupRequired">,
): "setup" | "runtime" {
  return onboarding.setupRequired ? "setup" : "runtime";
}
