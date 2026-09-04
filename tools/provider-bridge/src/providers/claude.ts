import { randomUUID } from "node:crypto";
import {
  query,
  type Options,
  type SDKAssistantMessage,
  type SDKMessage,
  type SDKPartialAssistantMessage,
  type SDKResultMessage,
  type SDKUserMessage,
  type Query,
} from "@anthropic-ai/claude-agent-sdk";
import { PROVIDER_CAPABILITIES } from "../provider-capabilities.js";
import { BridgeError, safeProviderError } from "../errors.js";
import { readImage, validateImages } from "../images.js";
import type { ProviderAgent, ProviderAppendInput, ProviderCallOptions, ProviderCallResult, ProviderImageAttachment, ProviderUsage } from "../types.js";
import { AsyncInput } from "./async-input.js";
import { claudeContent } from "./inputs.js";
import { resolveStructuredOutput } from "../structured-output.js";
import { observationFrameDirectory } from "./observation-frame-directory.js";

async function userMessage(
  message: string,
  attachments: readonly ProviderImageAttachment[],
  priority?: "now",
): Promise<SDKUserMessage> {
  const images = await Promise.all(attachments.map(async (image) => ({
    ...image,
    data: await readImage(image),
  })));
  return {
    type: "user",
    message: { role: "user", content: claudeContent(message, images) },
    parent_tool_use_id: null,
    uuid: randomUUID() as NonNullable<SDKUserMessage["uuid"]>,
    timestamp: new Date().toISOString(),
    shouldQuery: true,
    ...(priority === undefined ? {} : { priority }),
  };
}

interface ActiveClaudeQuery {
  readonly stream: Query;
  readonly input: AsyncInput<SDKUserMessage>;
}

function textDelta(message: SDKPartialAssistantMessage): string | undefined {
  const event = message.event;
  if (event.type !== "content_block_delta" || !("delta" in event)) return undefined;
  const delta = event.delta as { type?: unknown; text?: unknown };
  return delta.type === "text_delta" && typeof delta.text === "string" ? delta.text : undefined;
}

function assistantText(message: SDKAssistantMessage): string {
  return message.message.content
    .filter((block): block is Extract<typeof block, { type: "text" }> => block.type === "text")
    .map((block) => block.text)
    .join("");
}

function usage(message: SDKResultMessage): ProviderUsage | undefined {
  const raw = (message as SDKResultMessage & { usage?: unknown }).usage;
  if (raw === null || typeof raw !== "object") return undefined;
  const record = raw as Record<string, unknown>;
  const inputTokens = typeof record.input_tokens === "number" ? record.input_tokens : undefined;
  const outputTokens = typeof record.output_tokens === "number" ? record.output_tokens : undefined;
  if (inputTokens === undefined && outputTokens === undefined) return undefined;
  return {
    ...(inputTokens === undefined ? {} : { inputTokens }),
    ...(outputTokens === undefined ? {} : { outputTokens }),
    ...(inputTokens === undefined || outputTokens === undefined ? {} : { totalTokens: inputTokens + outputTokens }),
  };
}

export function isMidTurnRestart(result: SDKResultMessage): boolean {
  const terminalReason = (result as SDKResultMessage & { terminal_reason?: unknown }).terminal_reason;
  return result.subtype === "error_during_execution" && terminalReason === "aborted_streaming";
}

function model(value: string | undefined): string | undefined {
  return value === undefined || value === "default" ? undefined : value;
}

export class ClaudeAgent implements ProviderAgent {
  readonly provider = "claude" as const;
  readonly capabilities = PROVIDER_CAPABILITIES.claude;
  private readonly activeQueries = new Map<string, ActiveClaudeQuery>();

  constructor(private readonly createQuery: typeof query = query) {}

  async send(options: ProviderCallOptions): Promise<ProviderCallResult> {
    validateImages(options.images);
    if (options.session.mode === "resume" && options.session.id === undefined) {
      throw new BridgeError("protocol", "resume session ID がありません");
    }
    const controller = new AbortController();
    const abort = (): void => controller.abort(options.signal.reason);
    if (options.signal.aborted) abort();
    else options.signal.addEventListener("abort", abort, { once: true });
    const selectedModel = model(options.model);
    const frameDirectory = observationFrameDirectory();
    const readTools = frameDirectory === undefined ? [] : ["Read"];
    const sdkOptions: Options = {
      abortController: controller,
      cwd: options.cwd,
      ...(frameDirectory === undefined ? {} : { additionalDirectories: [frameDirectory] }),
      systemPrompt: options.systemPrompt,
      tools: readTools,
      allowedTools: readTools,
      skills: [],
      mcpServers: {},
      strictMcpConfig: true,
      settingSources: [],
      permissionMode: "dontAsk",
      includePartialMessages: true,
      persistSession: options.session.mode !== "ephemeral",
      extraArgs: { "replay-user-messages": null },
      ...(selectedModel === undefined ? {} : { model: selectedModel }),
      ...(options.effort === undefined || options.effort === "default"
        ? {}
        : { effort: options.effort as NonNullable<Options["effort"]> }),
      ...(options.schema === undefined
        ? {}
        : { outputFormat: { type: "json_schema", schema: options.schema } }),
      ...(options.executable === undefined ? {} : { pathToClaudeCodeExecutable: options.executable }),
      ...(options.session.mode === "resume" ? { resume: options.session.id } : {}),
    };
    let finalText = "";
    let sessionId = options.session.id;
    let value: unknown;
    let providerUsage: ProviderUsage | undefined;
    const input = new AsyncInput<SDKUserMessage>();
    try {
      const initial = await userMessage(options.message, options.images);
      void input.push(initial);
      const stream = this.createQuery({
        prompt: input,
        options: sdkOptions,
      });
      const active: ActiveClaudeQuery = { stream, input };
      this.activeQueries.set(options.requestId, active);
      for await (const message of stream as AsyncIterable<SDKMessage>) {
        if ("session_id" in message && typeof message.session_id === "string") sessionId = message.session_id;
        if (message.type === "user") {
          const echoed = message as SDKUserMessage;
          if (echoed.uuid !== undefined) {
            active.input.acknowledge((pending) => pending.uuid === echoed.uuid);
          }
        } else if (message.type === "stream_event") {
          const delta = textDelta(message as SDKPartialAssistantMessage);
          if (delta !== undefined) options.emitDelta(delta);
        } else if (message.type === "assistant") {
          finalText = assistantText(message as SDKAssistantMessage);
        } else if (message.type === "result") {
          const result = message as SDKResultMessage;
          if (isMidTurnRestart(result)) {
            options.resetDelta();
            finalText = "";
            value = undefined;
            providerUsage = undefined;
            continue;
          }
          active.input.close();
          providerUsage = usage(result);
          const structured = (result as SDKResultMessage & { structured_output?: unknown }).structured_output;
          if (structured !== undefined) value = structured;
          if (result.subtype !== "success") throw new BridgeError("retryable", "Claude SDK の呼び出しに失敗しました");
          if (typeof result.result === "string" && result.result.length > 0) finalText = result.result;
        }
      }
      if (finalText.length === 0 && value === undefined) {
        throw new BridgeError("invalid-output", "Claude の応答本文がありません");
      }
      if (options.schema !== undefined) {
        value = resolveStructuredOutput("Claude", options.schema, value, finalText);
      }
      return {
        text: finalText,
        ...(value === undefined ? {} : { value }),
        ...(sessionId === undefined ? {} : { sessionId }),
        ...(providerUsage === undefined ? {} : { usage: providerUsage }),
      };
    } catch (error) {
      throw safeProviderError(error);
    } finally {
      const active = this.activeQueries.get(options.requestId);
      active?.input.close();
      input.close();
      this.activeQueries.delete(options.requestId);
      options.signal.removeEventListener("abort", abort);
    }
  }

  async append(options: ProviderAppendInput): Promise<void> {
    validateImages(options.images);
    const active = this.activeQueries.get(options.requestId);
    if (active === undefined) {
      throw new BridgeError("retryable", "追加先の Claude turn は終了しています");
    }
    const appended = await userMessage(options.message, options.images, "now");
    if (!await active.input.push(appended)) {
      throw new BridgeError("retryable", "追加先の Claude turn は終了しています");
    }
  }

  async compactSession(): Promise<void> {
    throw new BridgeError("unsupported", "Claude SDK はこの bridge の明示的な compact に対応していません");
  }

  close(): Promise<void> {
    for (const active of this.activeQueries.values()) {
      active.input.close();
      active.stream.close();
    }
    this.activeQueries.clear();
    return Promise.resolve();
  }
}
