import { t, type Locale, type TranslationKey } from "./i18n/index.js";
import type { CompanionEmotions } from "./types.js";

export const companionEmotionKeys: readonly (keyof CompanionEmotions)[] = [
  "joy",
  "embarrassment",
  "concern",
  "surprise",
  "curiosity",
  "frustration",
];

const companionEmotionLabelKeys: Readonly<Record<keyof CompanionEmotions, TranslationKey>> = {
  joy: "now.emotionJoy",
  embarrassment: "now.emotionEmbarrassment",
  concern: "now.emotionConcern",
  surprise: "now.emotionSurprise",
  curiosity: "now.emotionCuriosity",
  frustration: "now.emotionFrustration",
};

export function companionEmotionLabel(key: keyof CompanionEmotions, locale: Locale): string {
  return t(locale, companionEmotionLabelKeys[key]);
}
