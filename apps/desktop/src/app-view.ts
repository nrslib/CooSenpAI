import type { TranslationKey } from "./i18n/index.js";
export type UiText = { readonly kind: "literal"; readonly text: string } | { readonly kind: "message"; readonly key: TranslationKey; readonly args: Readonly<Record<string, UiText>> } | { readonly kind: "join"; readonly parts: readonly UiText[] };
export function renderUiText(text: UiText, t: (key: TranslationKey, params?: Readonly<Record<string, string | number>>) => string): string {
  switch (text.kind) {
    case "literal": return text.text;
    case "message": return t(text.key, Object.fromEntries(Object.entries(text.args).map(([key, value]) => [key, renderUiText(value, t)])));
    case "join": return text.parts.map((part) => renderUiText(part, t)).join("");
  }
}
export interface TutorialUi { readonly message: UiText; readonly next: "next" | "retry" | null; readonly finishLabel: UiText }
export interface AppView {
  readonly screen: "loading" | "startupError" | "finish" | "setup" | "chat"; readonly loading: boolean; readonly error: string | null; readonly finishBusy: boolean;
  readonly settings: "closed" | "open" | "locked"; readonly settingsFocus: "watch" | null; readonly settingsGeneration: number;
  readonly focusRequest: number; readonly historyOpen: boolean; readonly resetConfirmOpen: boolean; readonly menuOpen: boolean; readonly canReset: boolean;
  readonly watchChanging: boolean; readonly audioChanging: boolean; readonly tutorial: TutorialUi | null;
}
export type AppInput = { readonly type: "mounted" | "retry" | "startupSettings" | "openSettings" | "closeSettings" | "toggleHistory" | "resetOpen" | "resetCancel" | "resetConfirm" | "menuToggle" | "menuDismiss" | "menuModel" | "menuReset" | "toggleWatch" | "toggleAudio" | "tutorialNext" | "tutorialFinish" | "recover" }
  | { readonly type: "settingsPresented"; readonly generation: number }
  | { readonly type: "report"; readonly error: string | null };
export interface ThoughtView { readonly text: UiText; readonly leaving: boolean }
export interface BannerView { readonly tone: "error" | "warning" | "info"; readonly message: UiText; readonly action: "settings" | "screen-capture" | "microphone" | "recognition" | "relaunch" | null; readonly actionLabel: UiText | null }
export interface StatusView { readonly presence: { readonly mode: "resting" | "watching" | "thinking" | "attention" | "switching"; readonly text: UiText }; readonly thought: ThoughtView | null; readonly banner: BannerView | null }
