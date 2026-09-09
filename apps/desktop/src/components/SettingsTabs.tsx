import type { ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import { SETTINGS_CATEGORIES, type SettingsCategory } from "../settings-categories.js";

interface Props {
  readonly activeCategory: SettingsCategory;
  readonly onSelect: (category: SettingsCategory) => void;
  readonly disabled?: boolean;
}

export function SettingsTabs({ activeCategory, onSelect, disabled = false }: Props): ReactElement {
  const { t } = useI18n();
  return <nav className="settings-tabs" aria-label={t("settings.categoriesLabel")} role="tablist" aria-orientation="vertical">
    {SETTINGS_CATEGORIES.map((category) => <button
      id={`settings-tab-${category.id}`}
      className="settings-tab"
      key={category.id}
      type="button"
      role="tab"
      aria-selected={activeCategory === category.id}
      aria-controls={`settings-category-${category.id}`}
      tabIndex={disabled ? -1 : activeCategory === category.id ? 0 : -1}
      disabled={disabled}
      onClick={() => onSelect(category.id)}
    >{t(category.labelKey)}</button>)}
  </nav>;
}
