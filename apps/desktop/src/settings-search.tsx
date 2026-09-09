import { createContext, useContext, type ReactElement, type ReactNode } from "react";

interface SettingsSearchContextValue {
  readonly query: string;
  readonly categoryLabel?: string;
}

export interface SettingsSearchFields {
  readonly label: string;
  readonly description?: string;
  readonly path?: string;
}

const defaultContext: SettingsSearchContextValue = { query: "" };
const SettingsSearchContext = createContext<SettingsSearchContextValue>(defaultContext);

export function normalizeSettingsSearch(value: string): string {
  return value.normalize("NFKC").toLocaleLowerCase("ja-JP").trim();
}

export function matchesSettingsSearch(query: string, fields: SettingsSearchFields): boolean {
  const normalizedQuery = normalizeSettingsSearch(query);
  if (normalizedQuery === "") return true;
  return [fields.label, fields.description, fields.path]
    .filter((value): value is string => value !== undefined)
    .some((value) => normalizeSettingsSearch(value).includes(normalizedQuery));
}

export function SettingsSearchProvider({ query, children }: { readonly query: string; readonly children: ReactNode }): ReactElement {
  return <SettingsSearchContext.Provider value={{ query }}>{children}</SettingsSearchContext.Provider>;
}

export function SettingsSearchCategory({ label, children }: { readonly label: string; readonly children: ReactNode }): ReactElement {
  const context = useContext(SettingsSearchContext);
  if (normalizeSettingsSearch(context.query) === "") return <>{children}</>;
  return <div className="settings-search-category" data-settings-search-category={label}>
    <SettingsSearchContext.Provider value={{ query: context.query, categoryLabel: label }}>
      {children}
    </SettingsSearchContext.Provider>
  </div>;
}

export function SettingsSearchItem({ label, description, path, children }: SettingsSearchFields & { readonly children: ReactNode }): ReactElement | null {
  const context = useContext(SettingsSearchContext);
  if (!matchesSettingsSearch(context.query, { label, description, path })) return null;
  if (normalizeSettingsSearch(context.query) === "") return <>{children}</>;
  return <div className="settings-search-item" data-settings-search-match="true" data-settings-search-key={path}>
    {context.categoryLabel === undefined ? null : <small className="settings-search-category-label">{context.categoryLabel}</small>}
    {children}
  </div>;
}
