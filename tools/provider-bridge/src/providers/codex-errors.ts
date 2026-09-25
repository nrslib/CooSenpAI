import { BridgeError, type BridgeErrorKind } from "../errors.js";

// CLI diagnostics may contain prompts, headers and local paths. Only the classification leaves this boundary.
export function codexProviderError(error: unknown): BridgeError {
  if (error instanceof BridgeError) return error;
  const message = error instanceof Error ? error.message.toLowerCase() : "";
  const status = message.match(/(?:http(?:\/\d(?:\.\d)?)?\s+|status(?:\s+code)?[\s:=]+)([45]\d{2})\b/u)?.[1];
  let kind: BridgeErrorKind;
  let summary: string;
  if (/model/u.test(message) && /not supported|unsupported|not found|does not exist|do not have access|invalid model|model_not_found/u.test(message)) {
    kind = "invalid-model";
    summary = "設定された Codex model はこのアカウントで利用できません。モデル設定を確認してください";
  } else if (status === "401" || /authentication|unauthorized|not logged in|login required|invalid_api_key|token (?:has )?expired|refresh_token/u.test(message)) {
    kind = "auth";
    summary = "Codex の認証に失敗しました。ログイン情報を確認してください";
  } else if (status === "403" || /permission_denied|permission denied|forbidden|access denied/u.test(message)) {
    kind = "permission";
    summary = "Codex の利用権限がありません。アカウントの権限を確認してください";
  } else if (/insufficient_quota|usage limit|quota exceeded|billing_hard_limit/u.test(message)) {
    kind = "quota";
    summary = "Codex の利用上限に達しました。利用枠を確認してください";
  } else if (status === "429" || /rate_limit|rate limit|too many requests/u.test(message)) {
    kind = "rate-limit";
    summary = "Codex のリクエストが集中しています。しばらく待って再試行してください";
  } else if (/timeout|timed out/u.test(message) || status === "408") {
    kind = "timeout";
    summary = "Codex の応答待ちがタイムアウトしました";
  } else if (/abort|cancel|interrupt/u.test(message)) {
    kind = "cancelled";
    summary = "Codex 呼び出しがキャンセルされました";
  } else if (status === "400" || status === "404" || status === "422" || /invalid_request_error|unsupported (?:value|parameter)/u.test(message)) {
    kind = "invalid-request";
    summary = "Codex が設定を受け付けませんでした。モデルと effort の設定を確認してください";
  } else {
    kind = "retryable";
    summary = "Codex の通信またはサービスでエラーが発生しました";
  }
  return new BridgeError(kind, summary, { detail: status === undefined ? kind : `${kind} (HTTP ${status})` });
}
