export type BridgeErrorKind =
  | "auth"
  | "cancelled"
  | "invalid-model"
  | "invalid-output"
  | "protocol"
  | "retryable"
  | "unsupported";

export class BridgeError extends Error {
  readonly kind: BridgeErrorKind;
  readonly detail: string;

  constructor(
    kind: BridgeErrorKind,
    message: string,
    options?: ErrorOptions & { readonly detail?: string },
  ) {
    super(message, options);
    this.name = "BridgeError";
    this.kind = kind;
    this.detail = options?.detail ?? diagnosticDetail(options?.cause ?? this);
  }
}

export function safeProviderError(error: unknown): BridgeError {
  if (error instanceof BridgeError) {
    return new BridgeError(error.kind, safeDiagnosticDetail(error.message), {
      cause: error.cause,
      detail: safeDiagnosticDetail(error.detail),
    });
  }
  const message = error instanceof Error ? error.message.toLowerCase() : "";
  if (message.includes("abort") || message.includes("cancel") || message.includes("interrupt")) {
    return classifiedError("cancelled", "provider 呼び出しがキャンセルされました", error);
  }
  if (message.includes("auth") || message.includes("unauthorized") || message.includes("login")) {
    return classifiedError("auth", "provider の認証が必要です", error);
  }
  if (message.includes("model") && (message.includes("invalid") || message.includes("not found"))) {
    return classifiedError("invalid-model", "設定された provider model を利用できません", error);
  }
  return classifiedError("retryable", "provider SDK の実行に失敗しました", error);
}

export function invalidJsonOutput(provider: string, body: string, cause: unknown): BridgeError {
  const bodyPreview = redact(body).slice(0, 200);
  const detail = safeDiagnosticDetail(`${diagnosticDetail(cause)}; ${provider} output: ${bodyPreview}`);
  return new BridgeError("invalid-output", `${provider} の structured output が JSON ではありません`, {
    cause,
    detail,
  });
}

function classifiedError(kind: BridgeErrorKind, message: string, cause: unknown): BridgeError {
  return new BridgeError(kind, message, { cause, detail: diagnosticDetail(cause) });
}

function diagnosticDetail(error: unknown): string {
  const raw = error instanceof Error
    ? `${error.name}: ${error.message}`
    : `Error: ${String(error)}`;
  return safeDiagnosticDetail(raw);
}

export function safeDiagnosticDetail(value: string): string {
  return redact(value).slice(0, 300);
}

function redact(value: string): string {
  return value
    .replace(/sk-[A-Za-z0-9_-]+/g, "***")
    .replace(/Bearer\s+\S+/gi, "Bearer ***")
    .replace(/[A-Za-z0-9]{32,}/g, "***");
}
