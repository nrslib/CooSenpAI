import type { Provider, ProviderAgent } from "./types.js";
import { ClaudeAgent } from "./providers/claude.js";
import { CodexAgent } from "./providers/codex.js";
import { OpenCodeAgent } from "./providers/opencode.js";

export class ProviderRegistry {
  private readonly providers: ReadonlyMap<Provider, ProviderAgent>;

  constructor(agents: readonly ProviderAgent[] = [new CodexAgent(), new ClaudeAgent(), new OpenCodeAgent()]) {
    this.providers = new Map(agents.map((agent) => [agent.provider, agent]));
  }

  get(provider: Provider): ProviderAgent {
    const agent = this.providers.get(provider);
    if (agent === undefined) throw new Error(`provider is not registered: ${provider}`);
    return agent;
  }

  async close(): Promise<void> {
    await Promise.all([...this.providers.values()].map((agent) => agent.close()));
  }
}

export type {
  Provider,
  ProviderAgent,
  ProviderCallOptions,
  ProviderCapabilityOptions,
  ProviderImageAttachment,
  ProviderCompactSessionOptions,
} from "./types.js";
