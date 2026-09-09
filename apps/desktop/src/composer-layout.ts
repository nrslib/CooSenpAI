export const COMPOSER_MAX_HEIGHT = 144;

export function resizeComposerTextarea(
  textarea: Pick<HTMLTextAreaElement, "scrollHeight" | "style">,
): void {
  textarea.style.height = "auto";
  textarea.style.height = `${Math.min(textarea.scrollHeight, COMPOSER_MAX_HEIGHT)}px`;
  textarea.style.overflowY = textarea.scrollHeight > COMPOSER_MAX_HEIGHT ? "auto" : "hidden";
}
