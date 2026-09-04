import type { Event, OpencodeClient } from "@opencode-ai/sdk/v2";
import { PROVIDER_CAPABILITIES } from "../provider-capabilities.js";
import { BridgeError, invalidJsonOutput, safeProviderError } from "../errors.js";
import { validateImages } from "../images.js";
import type { ProviderAgent, ProviderCallOptions, ProviderCallResult, ProviderCapabilityOptions, ProviderCompactSessionOptions } from "../types.js";
import { OpenCodeServerOwner } from "./opencode-server.js";
import { opencodeParts } from "./inputs.js";
import { OpenCodeTextProjector } from "./opencode-text.js";

function parseModel(model: string | undefined): { providerID: string; modelID: string } {
  if (model === undefined || model === "default") {
    throw new BridgeError("invalid-model", "OpenCode の model は provider/model 形式で指定してください");
  }
  const separator = model.indexOf("/");
  if (separator <= 0 || separator === model.length - 1) {
    throw new BridgeError("invalid-model", "OpenCode の model は provider/model 形式で指定してください");
  }
  return { providerID: model.slice(0, separator), modelID: model.slice(separator + 1) };
}

function errorMessage(event: Event): string | undefined {
  if (event.type !== "session.error") return undefined;
  const error = event.properties.error;
  if (error === undefined || error === null || typeof error !== "object") return "OpenCode session error";
  const record = error as { data?: { message?: unknown }; message?: unknown };
  if (typeof record.message === "string") return record.message;
  return typeof record.data?.message === "string" ? record.data.message : "OpenCode session error";
}

async function abortSession(client: OpencodeClient, sessionID: string, cwd: string): Promise<void> {
  await client.session.abort({ sessionID, directory: cwd }).catch(() => undefined);
}

export function stripStructuredOutputFence(text: string): string {
  const trimmed = text.trim();
  const fenced = /^```json\s*([\s\S]*?)\s*```$/i.exec(trimmed);
  return (fenced?.[1] ?? trimmed).trim();
}

export class OpenCodeAgent implements ProviderAgent {
  readonly provider = "opencode" as const;
  readonly capabilities = PROVIDER_CAPABILITIES.opencode;

  constructor(private readonly owner = new OpenCodeServerOwner()) {}

  async resolveCapabilities(options: ProviderCapabilityOptions): Promise<typeof this.capabilities> {
    const executable = options.executable ?? "opencode";
    const model = parseModel(options.model ?? this.capabilities.defaultModel);
    const client = await this.owner.client(executable, options.signal);
    const imageInput = await this.owner.modelSupportsImages(
      client,
      model.providerID,
      model.modelID,
      options.cwd,
      options.signal,
    );
    return { ...this.capabilities, imageInput };
  }

  async send(options: ProviderCallOptions): Promise<ProviderCallResult> {
    if (options.session.mode === "resume" && options.session.id === undefined) {
      throw new BridgeError("protocol", "resume session ID がありません");
    }
    const executable = options.executable ?? "opencode";
    const model = parseModel(options.model);
    let ephemeralSession: { client: OpencodeClient; id: string } | undefined;
    try {
      const client = await this.owner.client(executable, options.signal);
      const imageSupport = await this.owner.modelSupportsImages(
        client,
        model.providerID,
        model.modelID,
        options.cwd,
        options.signal,
      );
      const images = imageSupport ? options.images : [];
      validateImages(images);
      let sessionId = options.session.id;
      if (sessionId === undefined) {
        sessionId = await this.owner.createSession(client, options.cwd, options.signal);
      }
      const activeSessionId = sessionId;
      if (options.session.mode === "ephemeral") {
        ephemeralSession = { client, id: activeSessionId };
      }
      const subscribed = await client.event.subscribe({ directory: options.cwd }, { signal: options.signal });
      const iterator = (subscribed.stream as AsyncIterable<Event>)[Symbol.asyncIterator]();
      const abort = (): void => { void abortSession(client, activeSessionId, options.cwd); };
      options.signal.addEventListener("abort", abort, { once: true });
      const prompt = client.session.promptAsync({
        sessionID: activeSessionId,
        directory: options.cwd,
        model,
        agent: "coosenpai",
        tools: {},
        system: options.systemPrompt,
        ...(options.effort === undefined || options.effort === "default" ? {} : { variant: options.effort }),
        parts: opencodeParts(options.message, options.schema, images),
      }, { signal: options.signal });
      let text = "";
      const textProjector = new OpenCodeTextProjector();
      try {
        while (true) {
          const next = await iterator.next();
          if (next.done) break;
          const event = next.value;
          const properties = event.properties as { sessionID?: unknown };
          if (properties.sessionID !== activeSessionId) continue;
          const failure = errorMessage(event);
          if (failure !== undefined) throw safeProviderError(new Error(failure));
          for (const delta of textProjector.push(event)) {
            text += delta;
            options.emitDelta(delta);
          }
          if (event.type === "session.idle") break;
        }
        await prompt;
      } finally {
        options.signal.removeEventListener("abort", abort);
        await iterator.return?.();
      }
      if (text.length === 0) throw new BridgeError("invalid-output", "OpenCode の応答本文がありません");
      let value: unknown;
      if (options.schema !== undefined) {
        try {
          value = JSON.parse(stripStructuredOutputFence(text)) as unknown;
        } catch (error) {
          throw invalidJsonOutput("OpenCode", text, error);
        }
      }
      return { text, value, sessionId: activeSessionId };
    } catch (error) {
      throw safeProviderError(error);
    } finally {
      if (ephemeralSession !== undefined) {
        await ephemeralSession.client.session.delete({
          sessionID: ephemeralSession.id,
          directory: options.cwd,
        }).catch(() => undefined);
      }
    }
  }

  async compactSession(options: ProviderCompactSessionOptions): Promise<void> {
    const client = await this.owner.client("opencode", options.signal);
    const model = parseModel(options.model);
    await client.session.summarize({
      sessionID: options.sessionId,
      providerID: model.providerID,
      modelID: model.modelID,
    }, { signal: options.signal });
  }

  close(): Promise<void> {
    return this.owner.close();
  }
}
