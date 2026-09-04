export type Provider = "codex" | "claude" | "opencode";

export type ProviderSessionMode = "new" | "resume" | "ephemeral";

export interface ProviderSession {
  readonly mode: ProviderSessionMode;
  readonly id?: string;
}

export interface ProviderImageAttachment {
  readonly path: string;
}

export interface ProviderCapabilities {
  readonly defaultModel: string;
  readonly modelCandidates: readonly string[];
  readonly imageInput: boolean;
  readonly nativeStructuredOutput: boolean;
  readonly effectiveStructuredOutput: boolean;
  readonly streaming: boolean;
  readonly cancellation: boolean;
  readonly sessionResume: boolean;
  readonly sessionCompact: boolean;
  readonly effort: boolean;
  readonly midTurnInput: boolean;
}

export interface ProviderAppendInput {
  readonly requestId: string;
  readonly message: string;
  readonly images: readonly ProviderImageAttachment[];
}

export interface ProviderCallOptions {
  readonly requestId: string;
  readonly session: ProviderSession;
  readonly model?: string;
  readonly effort?: string;
  readonly systemPrompt: string;
  readonly message: string;
  readonly images: readonly ProviderImageAttachment[];
  readonly schema?: Record<string, unknown>;
  readonly executable?: string;
  readonly cwd: string;
  readonly toolsDisabled: boolean;
  readonly signal: AbortSignal;
  readonly emitDelta: (text: string) => void;
  readonly resetDelta: () => void;
}

export interface ProviderCapabilityOptions {
  readonly model?: string;
  readonly executable?: string;
  readonly cwd: string;
  readonly signal: AbortSignal;
}

export interface ProviderUsage {
  readonly inputTokens?: number;
  readonly outputTokens?: number;
  readonly totalTokens?: number;
  readonly cachedInputTokens?: number;
}

export interface ProviderCallResult {
  readonly text: string;
  readonly value?: unknown;
  readonly sessionId?: string;
  readonly usage?: ProviderUsage;
}

export interface ProviderCompactSessionOptions {
  readonly sessionId: string;
  readonly model?: string;
  readonly signal: AbortSignal;
}

export interface ProviderAgent {
  readonly provider: Provider;
  readonly capabilities: ProviderCapabilities;
  resolveCapabilities?(options: ProviderCapabilityOptions): Promise<ProviderCapabilities>;
  send(options: ProviderCallOptions): Promise<ProviderCallResult>;
  append?(options: ProviderAppendInput): Promise<void>;
  compactSession(options: ProviderCompactSessionOptions): Promise<void>;
  close(): Promise<void>;
}
