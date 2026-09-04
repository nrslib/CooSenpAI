import type { BubbleInteraction, IpcResult } from "../types.js";

export interface BubbleInteractionRequest {
  readonly action: string;
  readonly value?: string;
}

export function selectedValue(interaction: BubbleInteraction | undefined): string {
  return interaction?.select?.selected ?? "";
}

export function selectRequest(
  interaction: BubbleInteraction,
  value: string,
): BubbleInteractionRequest | undefined {
  if (interaction.select === undefined) return undefined;
  if (!interaction.select.options.some((option) => option.value === value)) return undefined;
  return { action: interaction.select.action, value };
}

export function actionRequest(
  interaction: BubbleInteraction,
  action: string,
): BubbleInteractionRequest | undefined {
  return interaction.actions.some((item) => item.id === action) ? { action } : undefined;
}

export function interactionFailure(result: IpcResult<null>): string | undefined {
  return result.ok ? undefined : result.error.message;
}
