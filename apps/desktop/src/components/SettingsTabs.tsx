import type { ReactElement } from "react";

import { SETTINGS_CATEGORIES, type SettingsCategory } from "../settings-categories.js";

interface Props {
  readonly activeCategory: SettingsCategory;
  readonly onSelect: (category: SettingsCategory) => void;
}

export function SettingsTabs({ activeCategory, onSelect }: Props): ReactElement {
  return <nav className="settings-tabs" aria-label="設定カテゴリ" role="tablist" aria-orientation="vertical">
    {SETTINGS_CATEGORIES.map((category) => <button
      id={`settings-tab-${category.id}`}
      className="settings-tab"
      key={category.id}
      type="button"
      role="tab"
      aria-selected={activeCategory === category.id}
      aria-controls={`settings-category-${category.id}`}
      tabIndex={activeCategory === category.id ? 0 : -1}
      onClick={() => onSelect(category.id)}
    >{category.label}</button>)}
  </nav>;
}
