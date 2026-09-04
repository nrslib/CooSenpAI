import type { CompanionModelCatalog, ConfigPatch, ModelCatalogProvider, ProviderName } from "./types.js";

export const PROVIDER_OPTIONS: readonly ProviderName[] = ["codex", "claude", "opencode"];
export const EFFORT_OPTIONS = ["default", "low", "medium", "high", "xhigh"] as const;

export const BUILTIN_MODEL_CANDIDATES: Readonly<Record<"codex" | "claude", readonly string[]>> = {
  codex: ["default", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"],
  claude: ["default", "opus", "sonnet", "haiku"],
};

export interface CatalogRequestGeneration {
  begin(): number;
  invalidate(): void;
  isCurrent(generation: number): boolean;
}

export function createCatalogRequestGeneration(): CatalogRequestGeneration {
  let current = 0;
  return {
    begin: () => {
      current += 1;
      return current;
    },
    invalidate: () => {
      current += 1;
    },
    isCurrent: (generation) => generation === current,
  };
}

export function catalogProvider(
  catalog: CompanionModelCatalog | undefined,
  provider: ProviderName,
): ModelCatalogProvider | undefined {
  return catalog?.providers.find((candidate) => candidate.provider === provider);
}

export function mergeOpencodeReloadCatalog(
  current: CompanionModelCatalog | undefined,
  refreshed: CompanionModelCatalog,
): CompanionModelCatalog {
  if (current === undefined) return refreshed;
  const refreshedOpencode = catalogProvider(refreshed, "opencode");
  if (refreshedOpencode === undefined) return current;
  return {
    ...current,
    providers: current.providers.map((provider) => provider.provider === "opencode"
      ? { ...provider, candidates: refreshedOpencode.candidates }
      : provider),
    opencodeError: refreshed.opencodeError,
  };
}

export function modelCandidates(
  catalog: CompanionModelCatalog | undefined,
  provider: ProviderName,
  currentModel = "",
): readonly string[] {
  const providerCatalog = catalogProvider(catalog, provider);
  const builtin = provider === "codex" || provider === "claude"
    ? BUILTIN_MODEL_CANDIDATES[provider]
    : [];
  return unique([
    ...(providerCatalog?.candidates ?? builtin),
    ...(providerCatalog?.history ?? []),
    currentModel,
  ]);
}

export function defaultModelForProvider(
  catalog: CompanionModelCatalog | undefined,
  provider: ProviderName,
): string {
  const providerCatalog = catalogProvider(catalog, provider);
  const configuredDefault = providerCatalog?.defaultModel.trim() ?? "";
  if (configuredDefault !== "") {
    return configuredDefault;
  }
  if (provider === "claude") return "sonnet";
  const candidates = modelCandidates(catalog, provider);
  return candidates[0] ?? "";
}

export function effortCandidates(
  catalog: CompanionModelCatalog | undefined,
  provider: ProviderName,
  model: string,
): readonly string[] {
  const providerCatalog = catalogProvider(catalog, provider);
  const modelSpecific = providerCatalog?.modelEfforts[model];
  const providerEfforts = providerCatalog?.efforts;
  const base = providerEfforts !== undefined && providerEfforts.length > 0
    ? providerEfforts
    : EFFORT_OPTIONS;
  return unique(modelSpecific !== undefined && modelSpecific.length > 0 ? modelSpecific : base);
}

export function companionProviderModelPatch(provider: ProviderName, model: string): ConfigPatch {
  return { companion: { provider, model } };
}

export function companionEffortPatch(effort: string): ConfigPatch {
  return { companion: { effort } };
}

function unique(values: readonly string[]): readonly string[] {
  return values.reduce<string[]>((result, value) => {
    const trimmed = value.trim();
    if (trimmed !== "" && !result.includes(trimmed)) result.push(trimmed);
    return result;
  }, []);
}
