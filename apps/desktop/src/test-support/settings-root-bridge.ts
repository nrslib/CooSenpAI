import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createInterface } from "node:readline";

// cargo のライブラリテストと stdio だけを使い、WebView やブラウザは起動しない。
export class SettingsRootBridge {
  private process: ChildProcessWithoutNullStreams | undefined;
  private sequence = 0;
  private readonly pending = new Map<number, { resolve: (value: unknown) => void; reject: (error: Error) => void }>();
  private exited: Promise<number | null> = Promise.resolve(null);

  async start(): Promise<void> {
    const args = ["test", "--offline", "-q", "-p", "coosenpai-desktop", "--lib", "--no-default-features",
      "panels::root_bridge::settings_dom_bridge", "--", "--exact", "--ignored", "--nocapture"];
    process.stdout.write(`ROOT_TEST_COMMAND: cargo ${args.join(" ")}\n`);
    const child = spawn("cargo", args, { stdio: "pipe" });
    this.process = child;
    let stderr = "";
    child.stderr.setEncoding("utf8").on("data", (chunk: string) => { stderr += chunk; });
    this.exited = new Promise((resolve) => child.once("exit", (code) => {
      process.stdout.write(`ROOT_TEST_EXIT_CODE: ${code}\n`);
      resolve(code);
    }));
    await new Promise<void>((resolve, reject) => {
      child.once("error", reject);
      child.once("exit", (code) => {
        const error = new Error(`Settings Root bridge exited (${code}): ${stderr}`);
        reject(error);
        for (const pending of this.pending.values()) pending.reject(error);
        this.pending.clear();
      });
      createInterface({ input: child.stdout }).on("line", (line) => {
        if (!line.startsWith("SETTINGS_ROOT ")) return;
        const message = JSON.parse(line.slice("SETTINGS_ROOT ".length)) as { ready?: boolean; sequence?: number; value?: unknown };
        if (message.ready) resolve();
        if (message.sequence !== undefined) {
          this.pending.get(message.sequence)?.resolve(message.value);
          this.pending.delete(message.sequence);
        }
      });
    });
  }
  request<T>(request: Readonly<Record<string, unknown>>): Promise<T> {
    const child = this.process;
    if (child === undefined || child.exitCode !== null) return Promise.reject(new Error("Settings Root bridge is not running"));
    const sequence = ++this.sequence;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(sequence, { resolve: (value) => resolve(value as T), reject });
      child.stdin.write(`${JSON.stringify({ ...request, sequence })}\n`);
    });
  }
  async close(): Promise<void> {
    if (this.process === undefined) return;
    if (this.process.exitCode === null) this.process.stdin.end(`${JSON.stringify({ operation: "close" })}\n`);
    const code = await this.exited;
    if (code !== 0) throw new Error(`Settings Root bridge failed (${code})`);
  }
}
