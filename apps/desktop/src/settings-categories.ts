import type { AppSnapshot } from "./types.js";

export type SettingsCategory =
  | "general"
  | "vision"
  | "hearing"
  | "speech"
  | "notifications"
  | "providers"
  | "shortcuts"
  | "setup";

export interface SettingsCategoryDefinition {
  readonly id: SettingsCategory;
  readonly label: string;
}

export const SETTINGS_CATEGORIES: readonly SettingsCategoryDefinition[] = [
  { id: "general", label: "一般" },
  { id: "vision", label: "Vision AI" },
  { id: "hearing", label: "Hearing AI" },
  { id: "speech", label: "音声入力" },
  { id: "notifications", label: "通知と吹き出し" },
  { id: "providers", label: "プロバイダとモデル" },
  { id: "shortcuts", label: "ショートカット" },
  { id: "setup", label: "セットアップ" },
];

export function settingsCategoryForFocus(
  focusSection: "watch" | undefined,
  highlight: "persona" | "watch" | undefined,
): SettingsCategory | undefined {
  if (focusSection === "watch" || highlight === "watch") return "vision";
  if (highlight === "persona") return "general";
  return undefined;
}

export function isTutorialPersonaSettings(
  onboarding: Pick<AppSnapshot["onboarding"], "tutorialActive" | "currentStep" | "settingsHighlight">,
): boolean {
  return onboarding.tutorialActive
    && onboarding.currentStep === "persona"
    && onboarding.settingsHighlight === "persona";
}

export function settingsCategoryForIssue(path: string): SettingsCategory {
  if (inPathFamily(path, "watch") || inPathFamily(path, "retention")) return "vision";
  if (inPathFamily(path, "audio")) return "hearing";
  if (path === "speech.locale") return "general";
  if (inPathFamily(path, "speech")) return "speech";
  if (inPathFamily(path, "notification") || inPathFamily(path, "bubble") || inPathFamily(path, "popup")) {
    return "notifications";
  }
  if (inPathFamily(path, "keymap")) return "shortcuts";
  if (inPathFamily(path, "observer")) {
    if (path === "observer") return "providers";
    if (inPathFamily(path, "observer.provider") || inPathFamily(path, "observer.model")
      || inPathFamily(path, "observer.effort") || inPathFamily(path, "observer.executable")
      || inPathFamily(path, "observer.timeoutMs") || inPathFamily(path, "observer.dailyCallLimit")) {
      return "providers";
    }
    return "vision";
  }
  if (inPathFamily(path, "companion")) {
    if (inPathFamily(path, "companion.provider") || inPathFamily(path, "companion.model")
      || inPathFamily(path, "companion.effort") || inPathFamily(path, "companion.executable")
      || inPathFamily(path, "companion.timeoutMs") || inPathFamily(path, "companion.dailyProactiveLimit")) {
      return "providers";
    }
    if (inPathFamily(path, "companion.wakeCoalesceMax") || inPathFamily(path, "companion.sessionMaxCalls")
      || inPathFamily(path, "companion.stuckAfterMs") || inPathFamily(path, "companion.pendingDeliveryLimit")
      || inPathFamily(path, "companion.pendingDeliveryMaxBytes") || inPathFamily(path, "companion.proactiveQuietMinutes")) {
      return "vision";
    }
    return "general";
  }
  if (inPathFamily(path, "ui.thoughtBubble")) return "notifications";
  if (inPathFamily(path, "memory") || inPathFamily(path, "ui") || inPathFamily(path, "debug")
    || inPathFamily(path, "app") || inPathFamily(path, "chat")) {
    return "general";
  }
  return "general";
}

function inPathFamily(path: string, family: string): boolean {
  return path === family || path.startsWith(`${family}.`) || path.startsWith(`${family}[`);
}
