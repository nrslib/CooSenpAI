import { t, type Locale, type TranslationKey } from "../i18n/index.js";

export type VrmErrorKey = Extract<TranslationKey, `vrm.errors.${string}`>;

export class VrmError extends Error {
  constructor(readonly key: VrmErrorKey) {
    super(t("ja", key));
    this.name = "VrmError";
  }
}

export function vrmError(key: VrmErrorKey): VrmError {
  return new VrmError(key);
}

export function localizeVrmError(cause: unknown, locale: Locale): string {
  if (cause instanceof VrmError) return t(locale, cause.key);
  if (cause instanceof Error && cause.message.trim() !== "") return cause.message;
  return String(cause);
}
