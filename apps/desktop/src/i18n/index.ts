import { createContext, createElement, useContext, type ReactNode } from "react";

import { en } from "./en.js";
import { ja, type JapaneseDictionary } from "./ja.js";

export type Locale = "ja" | "en";
export type TranslationKey = LeafPath<JapaneseDictionary>;
export type TranslationParams = Readonly<Record<string, string | number>>;

type LeafPath<T> = {
  [K in keyof T & string]: T[K] extends string
    ? K
    : T[K] extends Record<string, unknown>
      ? `${K}.${LeafPath<T[K]>}`
      : never;
}[keyof T & string];

const dictionaries = { ja, en } as const;

function dictionaryValue(locale: Locale, key: TranslationKey): string {
  let value: unknown = dictionaries[locale];
  for (const part of key.split(".")) {
    if (typeof value !== "object" || value === null || !(part in value)) {
      throw new Error(`Missing translation key: ${key}`);
    }
    value = (value as Record<string, unknown>)[part];
  }
  if (typeof value !== "string") throw new Error(`Translation key is not a string: ${key}`);
  return value;
}

export function t(locale: Locale, key: TranslationKey, params: TranslationParams = {}): string {
  return dictionaryValue(locale, key).replace(/\{([A-Za-z][A-Za-z0-9_]*)\}/gu, (_placeholder, name: string) => {
    const value = params[name];
    if (value === undefined) throw new Error(`Missing translation parameter: ${name}`);
    return String(value);
  });
}

interface I18nContextValue {
  readonly locale: Locale;
  readonly t: (key: TranslationKey, params?: TranslationParams) => string;
}

const I18nContext = createContext<I18nContextValue>({ locale: "ja", t: (key, params) => t("ja", key, params) });

export function I18nProvider({ locale, children }: { readonly locale: Locale; readonly children: ReactNode }): ReactNode {
  const value: I18nContextValue = { locale, t: (key, params) => t(locale, key, params) };
  return createElement(I18nContext.Provider, { value }, children);
}

export function useI18n(): I18nContextValue {
  return useContext(I18nContext);
}
