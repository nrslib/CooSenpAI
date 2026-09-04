import { ProviderRegistry } from "./index.js";
import { BridgeError, safeProviderError } from "./errors.js";
import { DeltaProjector } from "./delta-projector.js";
import { requireSupportedNode } from "./node-version.js";
import { BoundedJsonlReader } from "./bounded-jsonl.js";
import { acceptBridgeRequestLine } from "./bridge-input.js";
import {
  DELTA_MAX_BYTES,
  PENDING_REQUEST_LIMIT,
  PROTOCOL_VERSION,
  REQUEST_LINE_MAX_BYTES,
  RESPONSE_LINE_MAX_BYTES,
  STREAM_MAX_BYTES,
  type AppendRequest,
  type BridgeRequest,
  type ResolveRequest,
  type SendRequest,
} from "./protocol.js";

interface BridgeEvent {
  readonly id: string;
  readonly event: "session" | "delta" | "final" | "usage" | "error" | "closed";
  readonly [key: string]: unknown;
}

interface ActiveRequest {
  readonly controller: AbortController;
  readonly agent: ReturnType<ProviderRegistry["get"]>;
  readonly completion: Promise<void>;
  readonly complete: () => void;
}

export class BridgeHost {
  private readonly registry: ProviderRegistry;
  private readonly pending = new Map<string, ActiveRequest>();
  private closing = false;
  private readonly startupError: BridgeError | undefined;

  constructor(registry = new ProviderRegistry(), startupError?: BridgeError) {
    this.registry = registry;
    this.startupError = startupError;
  }

  accept(request: BridgeRequest): void {
    if (this.startupError !== undefined) {
      this.emitError(request.id, this.startupError);
      return;
    }
    if (this.closing && request.op !== "close") {
      this.emitError(request.id, new BridgeError("protocol", "bridge is closing"));
      return;
    }
    if (request.op === "open") {
      const agent = this.registry.get(request.provider);
      this.emit({
        id: request.id,
        event: "session",
        protocolVersion: PROTOCOL_VERSION,
        provider: request.provider,
        capabilities: agent.capabilities,
      });
      this.emit({ id: request.id, event: "closed" });
      return;
    }
    if (request.op === "cancel") {
      void this.cancel(request.id, request.targetId);
      return;
    }
    if (request.op === "append") {
      void this.append(request);
      return;
    }
    if (request.op === "resolve") {
      if (this.pending.size >= PENDING_REQUEST_LIMIT) {
        this.emitError(request.id, new BridgeError("retryable", "pending request limit exceeded"));
        return;
      }
      if (this.pending.has(request.id)) {
        this.emitError(request.id, new BridgeError("protocol", "request ID is already active"));
        return;
      }
      void this.resolve(request);
      return;
    }
    if (request.op === "close") {
      void this.close(request.id);
      return;
    }
    if (this.pending.size >= PENDING_REQUEST_LIMIT) {
      this.emitError(request.id, new BridgeError("retryable", "pending request limit exceeded"));
      return;
    }
    if (this.pending.has(request.id)) {
      this.emitError(request.id, new BridgeError("protocol", "request ID is already active"));
      return;
    }
    void this.send(request);
  }

  private async resolve(request: ResolveRequest): Promise<void> {
    const controller = new AbortController();
    const agent = this.registry.get(request.provider);
    const active = pendingRequest(controller, agent);
    this.pending.set(request.id, active);
    try {
      const capabilities = agent.resolveCapabilities === undefined
        ? agent.capabilities
        : await agent.resolveCapabilities({
          ...(request.model === undefined ? {} : { model: request.model }),
          ...(request.executable === undefined ? {} : { executable: request.executable }),
          cwd: request.cwd,
          signal: controller.signal,
        });
      this.emit({
        id: request.id,
        event: "session",
        protocolVersion: PROTOCOL_VERSION,
        provider: request.provider,
        capabilities,
      });
      this.emit({ id: request.id, event: "closed" });
    } catch (error) {
      this.emitError(request.id, safeProviderError(error));
    } finally {
      this.pending.delete(request.id);
      active.complete();
    }
  }

  private async send(request: SendRequest): Promise<void> {
    const controller = new AbortController();
    const agent = this.registry.get(request.provider);
    const active = pendingRequest(controller, agent);
    this.pending.set(request.id, active);
    let streamBytes = 0;
    const projector = new DeltaProjector(request.schema);
    const timeout = setTimeout(() => controller.abort(new Error("timeout")), request.timeoutMs);
    try {
      const result = await agent.send({
        requestId: request.id,
        session: request.session,
        ...(request.model === undefined ? {} : { model: request.model }),
        ...(request.effort === undefined ? {} : { effort: request.effort }),
        systemPrompt: request.systemPrompt,
        message: request.message,
        images: request.images.map((path) => ({ path })),
        ...(request.schema === undefined ? {} : { schema: request.schema }),
        ...(request.executable === undefined ? {} : { executable: request.executable }),
        cwd: request.cwd,
        toolsDisabled: request.toolsDisabled,
        signal: controller.signal,
        emitDelta: (text) => {
          const bytes = Buffer.byteLength(text, "utf8");
          streamBytes += bytes;
          if (bytes > DELTA_MAX_BYTES || streamBytes > STREAM_MAX_BYTES) {
            controller.abort(new BridgeError("invalid-output", "provider stream exceeds the byte limit"));
            return;
          }
          const visible = projector.push(text);
          if (visible.length > 0) this.emit({ id: request.id, event: "delta", text: visible });
        },
        resetDelta: () => {
          projector.reset();
          this.emit({ id: request.id, event: "delta", text: "", reset: true });
        },
      });
      if (result.sessionId !== undefined) {
        this.emit({ id: request.id, event: "session", session: result.sessionId });
      }
      if (result.usage !== undefined) {
        this.emit({ id: request.id, event: "usage", ...result.usage });
      }
      this.emit({ id: request.id, event: "final", text: result.text, value: result.value });
    } catch (error) {
      this.emitError(request.id, safeProviderError(error));
    } finally {
      clearTimeout(timeout);
      this.pending.delete(request.id);
      active.complete();
    }
  }

  private async cancel(id: string, targetId: string): Promise<void> {
    const active = this.pending.get(targetId);
    if (active === undefined) {
      this.emit({ id, event: "closed", targetId });
      return;
    }
    active.controller.abort(new Error("cancelled"));
    await active.completion;
    this.emit({ id, event: "closed", targetId });
  }

  private async append(request: AppendRequest): Promise<void> {
    const active = this.pending.get(request.targetId);
    if (active === undefined) {
      this.emitError(request.id, new BridgeError("retryable", "append target is not active"));
      return;
    }
    if (!active.agent.capabilities.midTurnInput || active.agent.append === undefined) {
      this.emitError(request.id, new BridgeError("unsupported", "provider does not support mid-turn input"));
      return;
    }
    try {
      await active.agent.append({
        requestId: request.targetId,
        message: request.message,
        images: request.images.map((path) => ({ path })),
      });
      this.emit({ id: request.id, event: "closed", targetId: request.targetId });
    } catch (error) {
      this.emitError(request.id, safeProviderError(error));
    }
  }

  private async close(id: string): Promise<void> {
    if (this.closing) {
      this.emit({ id, event: "closed" });
      return;
    }
    this.closing = true;
    const completions = [...this.pending.values()].map((active) => {
      active.controller.abort(new Error("bridge closing"));
      return active.completion;
    });
    await Promise.all(completions);
    await this.registry.close();
    this.emit({ id, event: "closed" });
    stopInput();
    process.exitCode = 0;
  }

  private emitError(id: string, error: BridgeError): void {
    const safe = safeProviderError(error);
    this.emit({ id, event: "error", kind: safe.kind, message: safe.message, detail: safe.detail });
  }

  private emit(event: BridgeEvent): void {
    const line = JSON.stringify(event);
    if (Buffer.byteLength(line, "utf8") > RESPONSE_LINE_MAX_BYTES) {
      const bounded = JSON.stringify({ id: event.id, event: "error", kind: "invalid-output", message: "bridge response exceeds the byte limit" });
      process.stdout.write(`${bounded}\n`);
      return;
    }
    process.stdout.write(`${line}\n`);
  }
}

function pendingRequest(
  controller: AbortController,
  agent: ReturnType<ProviderRegistry["get"]>,
): ActiveRequest {
  let complete!: () => void;
  const completion = new Promise<void>((resolve) => {
    complete = resolve;
  });
  return { controller, agent, completion, complete };
}

let startupError: BridgeError | undefined;
try {
  requireSupportedNode(process.versions.node);
} catch (error) {
  startupError = safeProviderError(error);
}
const host = new BridgeHost(new ProviderRegistry(), startupError);
const input = new BoundedJsonlReader(REQUEST_LINE_MAX_BYTES);
const stopInput = (): void => {
  process.stdin.pause();
  process.stdin.unref();
};
process.stdin.on("data", (chunk: Buffer) => {
  const result = input.push(chunk);
  for (let index = 0; index < result.oversizedLines; index += 1) {
    process.stdout.write(`${JSON.stringify({ id: "protocol", event: "error", kind: "protocol", message: "request line exceeds the byte limit" })}\n`);
  }
  for (const line of result.lines) {
    if (line.length === 0) continue;
    const errorEvent = acceptBridgeRequestLine(line, (request) => host.accept(request));
    if (errorEvent !== undefined) {
      process.stdout.write(`${JSON.stringify(errorEvent)}\n`);
    }
  }
});
