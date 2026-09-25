import { Codex, type CodexOptions, type ThreadEvent, type ThreadOptions } from "@openai/codex-sdk";
import { chmod, copyFile, mkdir, mkdtemp, rename, rm } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { PROVIDER_CAPABILITIES } from "../provider-capabilities.js";
import { BridgeError, invalidJsonOutput } from "../errors.js";
import { validateImages } from "../images.js";
import type { EffortSelection, ProviderAgent, ProviderCallOptions, ProviderCallResult, ProviderCompactSessionOptions, ProviderUsage } from "../types.js";
import { codexProviderError } from "./codex-errors.js";
import { codexInput, codexOutputSchema } from "./inputs.js";
import { observationDirectories } from "./observation-frame-directory.js";

function environment(): Record<string, string> {
  return Object.fromEntries(
    Object.entries(process.env).filter((entry): entry is [string, string] => entry[1] !== undefined),
  );
}

function hasApiKey(): boolean {
  const value = process.env.OPENAI_API_KEY;
  return value !== undefined && value.trim() !== "";
}

async function prepareAuth(root: string, sourceHome: string): Promise<void> {
  if (hasApiKey()) {
    await rm(join(root, "auth.json"), { force: true });
    return;
  }
  await copyFile(join(sourceHome, "auth.json"), join(root, "auth.json")).catch((error: unknown) => {
    throw new BridgeError("auth", "Codex のログイン情報を読み込めません", { cause: error });
  });
}

function model(value: string | undefined): string | undefined {
  return value === undefined || value === "default" ? undefined : value;
}

type CodexReasoningEffort = NonNullable<ThreadOptions["modelReasoningEffort"]>;

const CODEX_REASONING_EFFORTS: readonly CodexReasoningEffort[] = [
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
  "ultra",
  "persistent",
];

function effort(selection: EffortSelection | undefined): CodexReasoningEffort | undefined {
  if (selection === undefined || selection.value === "default") return undefined;
  if (selection.kind !== "candidate") {
    throw new BridgeError("unsupported", "Codex の reasoning effort が不正です");
  }
  const candidate = CODEX_REASONING_EFFORTS.find((effort) => effort === selection.value);
  if (candidate !== undefined) return candidate;
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

export type CodexClientFactory = (options: CodexOptions) => Codex;

async function ephemeralEnvironment(): Promise<{ env: Record<string, string>; cleanup: () => Promise<void> }> {
  const root = await mkdtemp(join(tmpdir(), "coosenpai-codex-"));
  const sourceHome = process.env.CODEX_HOME ?? join(homedir(), ".codex");
  await mkdir(root, { recursive: true });
  await prepareAuth(root, sourceHome);
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
    if (hasApiKey()) {
      await rm(join(root, "auth.json"), { force: true });
    } else {
      await copyFile(join(sourceHome, "auth.json"), temporaryAuth).catch((error: unknown) => {
        throw new BridgeError("auth", "Codex のログイン情報を読み込めません", { cause: error });
      });
      await chmod(temporaryAuth, 0o600);
      await rename(temporaryAuth, join(root, "auth.json"));
    }
    return { ...environment(), CODEX_HOME: root };
  })();
  return persistentEnvironmentPromise;
}

export class CodexAgent implements ProviderAgent {
  readonly provider = "codex" as const;
  readonly capabilities = PROVIDER_CAPABILITIES.codex;

  constructor(private readonly createClient: CodexClientFactory = (options) => new Codex(options)) {}

  async send(options: ProviderCallOptions): Promise<ProviderCallResult> {
    if (options.isolateTools === true) throw new BridgeError("unsupported", "Codex の作業用 tool 隔離は未対応です");
    const ephemeral = options.session.mode === "ephemeral" ? await ephemeralEnvironment() : undefined;
    let streamError: BridgeError | undefined;
    try {
      const sdkEnvironment = ephemeral?.env ?? await persistentEnvironment();
      const config = {
        developer_instructions: options.systemPrompt,
        mcp_servers: {},
        web_search: options.webSearchEnabled === true ? "live" : "disabled",
        model_reasoning_summary: "auto",
      } as CodexOptions["config"];
      const client = this.createClient({
        env: sdkEnvironment,
        ...(config === undefined ? {} : { config }),
        ...(options.executable === undefined ? {} : { codexPathOverride: options.executable }),
      });
      const readableObservationDirectories = observationDirectories();
      const reasoningEffort = effort(options.effort);
      const selectedModel = model(options.model);
      const threadOptions: ThreadOptions = {
        workingDirectory: options.cwd,
        skipGitRepoCheck: true,
        sandboxMode: "read-only",
        approvalPolicy: "never",
        networkAccessEnabled: false,
        webSearchMode: options.webSearchEnabled === true ? "live" : "disabled",
        ...(readableObservationDirectories.length === 0
          ? {}
          : { additionalDirectories: readableObservationDirectories }),
        ...(selectedModel === undefined ? {} : { model: selectedModel }),
        ...(reasoningEffort === undefined
          ? {}
          : { modelReasoningEffort: reasoningEffort }),
      };
      const thread = options.session.mode === "resume"
        ? (() => {
          const sessionId = options.session.id;
          if (sessionId === undefined) {
            throw new BridgeError("protocol", "resume session ID がありません");
          }
          return client.resumeThread(sessionId, threadOptions);
        })()
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
        options.emitProgress();
        if (event.type === "turn.failed") throw codexProviderError(new Error(event.error.message));
        if (event.type === "error") streamError = codexProviderError(new Error(event.message));
        if (event.type === "thread.started") sessionId = event.thread_id;
        usage = usageFromEvent(event) ?? usage;
        if (event.type !== "item.updated" && event.type !== "item.completed") continue;
        if (event.type === "item.completed" && event.item.type === "command_execution") {
          options.onToolExecution?.({
            provider: "codex",
            tool: "command_execution",
            input: { command: event.item.command },
            output: event.item.aggregated_output,
          });
        }
        if (event.type === "item.completed" && event.item.type === "web_search") {
          options.onToolExecution?.({
            provider: "codex",
            tool: "web_search",
            input: { query: event.item.query },
            output: { completed: true },
          });
        }
        if (event.type === "item.completed" && event.item.type === "mcp_tool_call") {
          options.onToolExecution?.({
            provider: "codex",
            tool: `${event.item.server}/${event.item.tool}`,
            input: event.item.arguments,
            output: event.item.result ?? event.item.error,
          });
        }
        if (event.item.type !== "agent_message") continue;
        const previous = offsets.get(event.item.id) ?? 0;
        if (event.item.text.length > previous) {
          const delta = event.item.text.slice(previous);
          offsets.set(event.item.id, event.item.text.length);
          options.emitDelta(delta);
        }
        if (event.type === "item.completed") finalText = event.item.text;
      }
      if (finalText.length === 0) throw streamError ?? new BridgeError("invalid-output", "Codex の応答本文がありません");
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
      throw error instanceof BridgeError || options.signal.aborted ? codexProviderError(error) : streamError ?? codexProviderError(error);
    } finally {
      await ephemeral?.cleanup();
    }
  }

  async compactSession(_options: ProviderCompactSessionOptions): Promise<void> {
    throw new BridgeError("unsupported", "Codex SDK は明示的な session compact に対応していません");
  }

  close(): Promise<void> {
    return Promise.resolve();
  }
}
