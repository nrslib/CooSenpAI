export function focusFirstSettingsControl(
  root: Pick<HTMLElement, "querySelector">,
): boolean {
  const first = root.querySelector<HTMLElement>(
    "button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled])",
  );
  first?.focus();
  return first !== null;
}
