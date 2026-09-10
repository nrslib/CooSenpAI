interface ModifierLabel {
  readonly label: string;
  readonly order: number;
}

const modifierLabels: Readonly<Record<string, ModifierLabel>> = {
  Control: { label: "⌃", order: 0 },
  Alt: { label: "⌥", order: 1 },
  Shift: { label: "⇧", order: 2 },
  CommandOrControl: { label: "⌘", order: 3 },
  Command: { label: "⌘", order: 3 },
  Super: { label: "⌘", order: 3 },
};

const keyLabels: Readonly<Record<string, string>> = {
  Equal: "=",
  Minus: "-",
  BracketLeft: "[",
  BracketRight: "]",
  Quote: "'",
  Semicolon: ";",
  Backslash: "\\",
  Comma: ",",
  Slash: "/",
  Period: ".",
  Backquote: "`",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  Backspace: "⌫",
  Delete: "⌦",
  Space: "Space",
  Tab: "Tab",
};

export function formatShortcutLabel(shortcut: string): string {
  const modifiers: Array<ModifierLabel & { readonly index: number }> = [];
  const keys: string[] = [];

  for (const [index, part] of shortcut.split("+").entries()) {
    const modifier = modifierLabels[part];
    if (modifier === undefined) {
      keys.push(keyLabels[part] ?? part);
    } else {
      modifiers.push({ ...modifier, index });
    }
  }

  modifiers.sort((left, right) => left.order - right.order || left.index - right.index);
  return [...modifiers.map(({ label }) => label), ...keys].join("");
}
