import type { ReactElement } from "react";

interface StartupProps {
  readonly error: string;
  readonly retrying: boolean;
  readonly onRetry: () => void;
  readonly onSettings: () => void;
  readonly onExit: () => void;
}

export function StartupRecoveryScreen({ error, retrying, onRetry, onSettings, onExit }: StartupProps): ReactElement {
  return <main className="recovery-screen" data-testid="startup-recovery">
    <section className="recovery-card">
      <h1>起動状態を読み込めませんでした</h1>
      <p role="alert">{error}</p>
      <div className="button-row">
        <button type="button" disabled={retrying} onClick={onRetry}>{retrying ? "再試行中…" : "もう一度試す"}</button>
        <button type="button" className="secondary" onClick={onSettings}>設定を開く</button>
        <button type="button" className="secondary" onClick={onExit}>終了</button>
      </div>
      <small>終了時は実行中の処理と子プロセスを停止します。</small>
    </section>
  </main>;
}

export function FinishRecoveryScreen({ error, busy, onRetry }: { readonly error?: string; readonly busy: boolean; readonly onRetry: () => void }): ReactElement {
  return <section className="finish-recovery" data-testid="finish-recovery">
    <h1>終了処理を完了できませんでした</h1>
    <p>会話や設定を変更せず、終了処理だけをやり直してください。</p>
    {error === undefined ? null : <p className="error-text" role="alert">{error}</p>}
    <button type="button" disabled={busy} onClick={onRetry}>{busy ? "再試行中…" : "終了処理を再試行"}</button>
  </section>;
}
