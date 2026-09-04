import type { AppSnapshot } from "./types.js";

export function tutorialChatInputReady(
  snapshot: Pick<AppSnapshot, "onboarding">,
): boolean {
  return !snapshot.onboarding.setupRequired
    && (!snapshot.onboarding.tutorialActive || snapshot.onboarding.chatInputEnabled);
}

export function tutorialResponseFailed(
  snapshot: Pick<AppSnapshot, "onboarding" | "conversation" | "lastError" | "activeUserMessageId">,
): boolean {
  if (!snapshot.onboarding.tutorialActive
    || !["chat", "text", "image", "voice"].includes(snapshot.onboarding.currentStep ?? "")
    || snapshot.lastError === undefined
    || snapshot.activeUserMessageId !== undefined) return false;
  const answered = new Set(snapshot.conversation.flatMap((entry) => entry.causedByIds ?? []));
  return snapshot.conversation.some((entry) => entry.role === "user" && !answered.has(entry.id));
}

export function tutorialStepCanBeSkipped(
  step: AppSnapshot["onboarding"]["currentStep"],
): boolean {
  return ["chat", "text", "image", "voice", "watch"].includes(step ?? "");
}

export function tutorialSettingsAreAvailable(step: AppSnapshot["onboarding"]["currentStep"]): boolean {
  return step === "persona" || step === "watch";
}

export function tutorialSettingsHighlight(
  onboarding: AppSnapshot["onboarding"],
  section: "persona" | "provider" | "watch",
): boolean {
  if (!onboarding.tutorialActive) return false;
  if (onboarding.settingsHighlight === "persona" && onboarding.currentStep === "persona") {
    return section === "persona" || section === "provider";
  }
  return onboarding.settingsHighlight === "watch"
    && onboarding.currentStep === "watch"
    && section === "watch";
}

export function tutorialSettingsPresentationPending(
  onboarding: AppSnapshot["onboarding"],
  settingsOpen: boolean,
): boolean {
  return settingsOpen
    && onboarding.tutorialActive
    && ((onboarding.settingsHighlight === "persona" && onboarding.currentStep === "persona")
      || (onboarding.settingsHighlight === "watch" && onboarding.currentStep === "watch"));
}
