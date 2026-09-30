import type { ComposerVisualLine } from "./composer-view.js";

export const COMPOSER_MAX_HEIGHT = 144;

export function resizeComposerTextarea(
  textarea: Pick<HTMLTextAreaElement, "scrollHeight" | "style">,
): void {
  textarea.style.height = "auto";
  textarea.style.height = `${Math.min(textarea.scrollHeight, COMPOSER_MAX_HEIGHT)}px`;
  textarea.style.overflowY = textarea.scrollHeight > COMPOSER_MAX_HEIGHT ? "auto" : "hidden";
}

export function measureComposerVisualLine(textarea: HTMLTextAreaElement): ComposerVisualLine {
  const document = textarea.ownerDocument;
  const style = getComputedStyle(textarea);
  const mirror = document.createElement("div");
  for (const property of [
    "font-family", "font-size", "font-weight", "font-style", "font-stretch",
    "font-variant", "font-feature-settings", "font-variation-settings", "line-height",
    "letter-spacing", "word-spacing", "text-transform", "text-indent", "text-align",
    "tab-size", "direction", "word-break", "overflow-wrap", "white-space",
  ]) mirror.style.setProperty(property, style.getPropertyValue(property));
  Object.assign(mirror.style, {
    position: "fixed", visibility: "hidden", pointerEvents: "none",
    left: "0", top: "0", margin: "0", padding: "0", border: "0",
    boxSizing: "content-box",
    width: `${textarea.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight)}px`,
  });
  // 末尾の空行にもキャレットを測れる文字を置き、本文全体の折り返しは維持する。
  const text = document.createTextNode(`${textarea.value}\u200b`);
  mirror.append(text);
  mirror.setAttribute("aria-hidden", "true");
  document.body.append(mirror);
  try {
    const range = document.createRange();
    const topAt = (offset: number) => {
      range.setStart(text, offset);
      range.collapse(true);
      return range.getBoundingClientRect().top;
    };
    const first = topAt(0);
    const last = topAt(textarea.value.length);
    const caret = topAt(textarea.selectionStart);
    // 同じ行のサブピクセル丸めだけを許容する。
    return { firstVisualLine: Math.abs(caret - first) < 0.5, lastVisualLine: Math.abs(caret - last) < 0.5 };
  } finally {
    mirror.remove();
  }
}
