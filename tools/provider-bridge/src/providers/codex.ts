import { Codex, type CodexOptions, type ThreadEvent, type ThreadOptions } from "@openai/codex-sdk";
import { chmod, copyFile, mkdir, mkdtemp, rename, rm } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { PROVIDER_CAPABILITIES } from "../provider-capabilities.js";
import { BridgeError, invalidJsonOutput, safeProviderError } from "../errors.js";
import { validateImages } from "../images.js";
import type { ProviderAgent, ProviderCallOptions, ProviderCallResult, ProviderUsage } from "../types.js";
import { codexInput, codexOutputSchema } from "./inputs.js";
import { observationFrameDirectory } from "./observation-frame-directory.js";

function environment(): Record<string, string> {
  return Object.fromEntries(
    Object.entries(process.env).filter((entry): entry is [string, string] => entry[1] !== undefined),
  );
}

function model(value: string | undefined): string | undefined {
  return value === undefined || value === "default" ? undefined : value;
}

function effort(value: string | undefined): "minimal" | "low" | "medium" | "high" | "xhigh" | undefined {
  if (value === undefined || value === "default") return undefined;
  if (["minimal", "low", "medium", "high", "xhigh"].includes(value)) {
    return value as "minimal" | "low" | "medium" | "high" | "xhigh";
  }
  throw new BridgeError("unsupported", "Codex の reasoning effort が不正です");
}

function usageFromEvent(event: ThreadEvent): ProviderUsage | undefined {
  if (event.type !== "turn.completed" || event.usage === null) return undefined;
  return {
    inputTokens: event.usage.input_tokens,
    outputTokens: event.usage.output_tokens,
    totalTokens: event.usage.input_tokens + event.usage.output_tokens,
    ...(event.usage.cached_input_tokens === undefined
      ? {}
      : { cachedInputTokens: event.usage.cached_input_tokens }),
  };
}

async function ephemeralEnvironment(): Promise<{ env: Record<string, string>; cleanup: () => Promise<void> }> {
  const root = await mkdtemp(join(tmpdir(), "coosenpai-codex-"));
  const sourceHome = process.env.CODEX_HOME ?? join(homedir(), ".codex");
  await mkdir(root, { recursive: true });
  await copyFile(join(sourceHome, "auth.json"), join(root, "auth.json")).catch((error: unknown) => {
    throw new BridgeError("auth", "Codex のログイン情報を読み込めません", { cause: error });
  });
  return {
    env: { ...environment(), CODEX_HOME: root },
    cleanup: () => rm(root, { recursive: true, force: true }),
  };
}

let persistentEnvironmentPromise: Promise<Record<string, string>> | undefined;

async function persistentEnvironment(): Promise<Record<string, string>> {
  persistentEnvironmentPromise ??= (async () => {
    const productRoot = process.env.COOSENPAI_HOME ?? join(homedir(), ".coosenpai");
    const root = join(productRoot, "state", "provider", "codex");
    const sourceHome = process.env.CODEX_HOME ?? join(homedir(), ".codex");
    const temporaryAuth = join(root, `.auth.json.${process.pid}.tmp`);
    await mkdir(root, { recursive: true, mode: 0o700 });
    await copyFile(join(sourceHome, "auth.json"), temporaryAuth).catch((error: unknown) => {
      throw new BridgeError("auth", "Codex のログイン情報を読み込めません", { cause: error });
    });
    await chmod(temporaryAuth, 0o600);
    await rename(temporaryAuth, join(root, "auth.json"));
    return { ...environment(), CODEX_HOME: root };
  })();
  return persistentEnvironmentPromise;
}

export class CodexAgent implements ProviderAgent {
  readonly provider = "codex" as const;
  readonly capabilities = PROVIDER_CAPABILITIES.codex;

  async send(options: ProviderCallOptions): Promise<ProviderCallResult> {
    const ephemeral = options.session.mode === "ephemeral" ? await ephemeralEnvironment() : undefined;
    try {
      const sdkEnvironment = ephemeral?.env ?? await persistentEnvironment();
      const config = {
        developer_instructions: options.systemPrompt,
        mcp_servers: {},
        web_search: "disabled",
        model_reasoning_summary: "auto",
      } as CodexOptions["config"];
      const client = new Codex({
        env: sdkEnvironment,
        ...(config === undefined ? {} : { config }),
        ...(options.executable === undefined ? {} : { codexPathOverride: options.executable }),
      });
      const frameDirectory = observationFrameDirectory();
      const threadOptions: ThreadOptions = {
        workingDirectory: options.cwd,
        skipGitRepoCheck: true,
        sandboxMode: "read-only",
        approvalPolicy: "never",
        networkAccessEnabled: false,
        webSearchMode: "disabled",
        ...(frameDirectory === undefined ? {} : { additionalDirectories: [frameDirectory] }),
        ...(model(options.model) === undefined ? {} : { model: model(options.model) as string }),
        ...(effort(options.effort) === undefined
          ? {}
          : { modelReasoningEffort: effort(options.effort) as NonNullable<ThreadOptions["modelReasoningEffort"]> }),
      };
      if (options.session.mode === "resume" && options.session.id === undefined) {
        throw new BridgeError("protocol", "resume session ID がありません");
      }
      const thread = options.session.mode === "resume"
        ? client.resumeThread(options.session.id as string, threadOptions)
        : client.startThread(threadOptions);
      validateImages(options.images);
      const turn = await thread.runStreamed(codexInput(options.message, options.images), {
        signal: options.signal,
        ...(options.schema === undefined ? {} : { outputSchema: codexOutputSchema(options.schema) }),
      });
      const offsets = new Map<string, number>();
      let finalText = "";
      let sessionId = options.session.id;
      let usage: ProviderUsage | undefined;
      for await (const event of turn.events) {
        if (event.type === "thread.started") sessionId = event.thread_id;
        usage = usageFromEvent(event) ?? usage;
        if (event.type !== "item.updated" && event.type !== "item.completed") continue;
        if (event.item.type !== "agent_message") continue;
        const previous = offsets.get(event.item.id) ?? 0;
        if (event.item.text.length > previous) {
          const delta = event.item.text.slice(previous);
          offsets.set(event.item.id, event.item.text.length);
          options.emitDelta(delta);
        }
        if (event.type === "item.completed") finalText = event.item.text;
      }
      if (finalText.length === 0) throw new BridgeError("invalid-output", "Codex の応答本文がありません");
      let value: unknown;
      if (options.schema !== undefined) {
        try {
          value = JSON.parse(finalText) as unknown;
        } catch (error) {
          throw invalidJsonOutput("Codex", finalText, error);
        }
      }
      return {
        text: finalText,
        ...(value === undefined ? {} : { value }),
        ...(sessionId === undefined ? {} : { sessionId }),
        ...(usage === undefined ? {} : { usage }),
      };
    } catch (error) {
      throw safeProviderError(error);
    } finally {
      await ephemeral?.cleanup();
    }
  }

  async compactSession(): Promise<void> {
    throw new BridgeError("unsupported", "Codex SDK は明示的な session compact に対応していません");
  }

  close(): Promise<void> {
    return Promise.resolve();
  }
}
