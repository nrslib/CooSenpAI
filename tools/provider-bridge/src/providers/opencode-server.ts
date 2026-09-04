import { spawn, type ChildProcess } from "node:child_process";
import { createServer } from "node:net";
import { createOpencodeClient, type OpencodeClient } from "@opencode-ai/sdk/v2";
import { BridgeError, safeProviderError } from "../errors.js";
import { observationFrameDirectory } from "./observation-frame-directory.js";

const HOST = "127.0.0.1";
const START_TIMEOUT_MS = 60_000;
const STOP_GRACE_MS = 1_000;
const OUTPUT_LIMIT = 64 * 1024;

export interface ServerRecord {
  readonly executable: string;
  readonly client: OpencodeClient;
  isRunning(): boolean;
  onUnexpectedExit(callback: () => void): void;
  close(): Promise<void>;
}

export type OpenCodeServerStarter = (executable: string) => Promise<ServerRecord>;

async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.unref();
    server.once("error", reject);
    server.listen(0, HOST, () => {
      const address = server.address();
      if (address === null || typeof address === "string") {
        server.close(() => reject(new BridgeError("retryable", "OpenCode の待受 port を確保できません")));
        return;
      }
      server.close((error) => error === undefined ? resolve(address.port) : reject(error));
    });
  });
}

function stopChild(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve();
  return new Promise((resolve) => {
    let settled = false;
    const finish = (): void => {
      if (settled) return;
      settled = true;
      clearTimeout(forceTimer);
      resolve();
    };
    child.once("exit", finish);
    child.kill("SIGTERM");
    const forceTimer = setTimeout(() => {
      if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
      const waitTimer = setTimeout(finish, STOP_GRACE_MS);
      waitTimer.unref();
    }, STOP_GRACE_MS);
    forceTimer.unref();
  });
}

async function start(executable: string): Promise<ServerRecord> {
  const port = await freePort();
  const frameDirectory = observationFrameDirectory();
  const framePermission = frameDirectory === undefined
    ? "deny"
    : { "*": "deny", [`${frameDirectory}/**`]: "allow" };
  const permissions = {
    "*": "deny",
    read: framePermission,
    edit: "deny",
    bash: "deny",
    webfetch: "deny",
    external_directory: framePermission,
  };
  const config = {
    mcp: {},
    permission: permissions,
    agent: {
      coosenpai: {
        description: "CooSenpAI provider bridge",
        mode: "primary",
        permission: permissions,
      },
    },
  };
  const child = spawn(executable, ["serve", `--hostname=${HOST}`, `--port=${port}`], {
    env: { ...process.env, OPENCODE_CONFIG_CONTENT: JSON.stringify(config) },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  const append = (chunk: Buffer | string): void => {
    output += chunk.toString();
    if (Buffer.byteLength(output, "utf8") > OUTPUT_LIMIT) output = output.slice(-OUTPUT_LIMIT);
  };
  child.stdout?.on("data", append);
  child.stderr?.on("data", append);
  const client = createOpencodeClient({ baseUrl: `http://${HOST}:${port}` });
  await waitUntilReady(child, client).catch(async (error: unknown) => {
    await stopChild(child);
    const diagnostic = safeProviderError(new Error(
      `${error instanceof Error ? `${error.name}: ${error.message}` : String(error)}; OpenCode output: ${output}`,
    ));
    throw new BridgeError(diagnostic.kind, "OpenCode server を起動できません", {
      cause: error,
      detail: diagnostic.detail,
    });
  });
  let closing = false;
  return {
    executable,
    client,
    isRunning: () => child.exitCode === null && child.signalCode === null,
    onUnexpectedExit: (callback) => {
      child.once("exit", () => {
        if (!closing) callback();
      });
    },
    close: async () => {
      closing = true;
      await stopChild(child);
    },
  };
}

async function waitUntilReady(child: ChildProcess, client: OpencodeClient): Promise<void> {
  const deadline = Date.now() + START_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new BridgeError("retryable", "OpenCode server が起動中に終了しました");
    }
    try {
      const health = await client.global.health();
      if (health.data?.healthy === true) return;
    } catch {
      // 起動完了までは接続拒否を想定する。
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new BridgeError("retryable", "OpenCode server の起動が timeout しました");
}

export class OpenCodeServerOwner {
  private readonly servers = new Map<string, ServerRecord>();
  private readonly starting = new Map<string, Promise<ServerRecord>>();
  private readonly sessionCreateTails = new WeakMap<object, Promise<void>>();
  private readonly imageCapabilities = new WeakMap<object, Map<string, Promise<boolean>>>();

  constructor(private readonly starter: OpenCodeServerStarter = start) {}

  async client(executable: string, signal: AbortSignal): Promise<OpencodeClient> {
    const server = this.servers.get(executable);
    if (server !== undefined && server.isRunning()) {
      return server.client;
    }
    return (await waitWithAbort(this.readyServer(executable), signal)).client;
  }

  async createSession(
    client: OpencodeClient,
    cwd: string,
    signal: AbortSignal,
  ): Promise<string> {
    const key = client as object;
    const previous = this.sessionCreateTails.get(key) ?? Promise.resolve();
    const operation = previous.then(async () => {
      if (signal.aborted) {
        throw new BridgeError("cancelled", "OpenCode session の作成待ちをキャンセルしました");
      }
      const created = await client.session.create({ directory: cwd }, { signal });
      const sessionId = created.data?.id;
      if (sessionId === undefined) {
        const diagnostic = safeProviderError(created.error);
        throw new BridgeError("retryable", "OpenCode session を作成できません", {
          detail: diagnostic.detail,
        });
      }
      return sessionId;
    });
    this.sessionCreateTails.set(key, operation.then(() => undefined, () => undefined));
    return waitWithAbort(operation, signal);
  }

  async modelSupportsImages(
    client: OpencodeClient,
    providerID: string,
    modelID: string,
    cwd: string,
    signal: AbortSignal,
  ): Promise<boolean> {
    const key = `${providerID}/${modelID}`;
    let perClient = this.imageCapabilities.get(client as object);
    if (perClient === undefined) {
      perClient = new Map();
      this.imageCapabilities.set(client as object, perClient);
    }
    let lookup = perClient.get(key);
    if (lookup === undefined) {
      lookup = client.provider.list({ directory: cwd })
        .then((response) => {
          if (response.error !== undefined) throw safeProviderError(response.error);
          const provider = response.data?.all.find((candidate) => candidate.id === providerID);
          const model = provider?.models[modelID];
          if (model === undefined) {
            throw new BridgeError("invalid-model", `OpenCode model が見つかりません: ${providerID}/${modelID}`);
          }
          return model.capabilities.input.image === true;
        })
        .catch((error: unknown) => {
          perClient?.delete(key);
          throw safeProviderError(error);
        });
      perClient.set(key, lookup);
    }
    return waitWithAbort(lookup, signal);
  }

  async close(): Promise<void> {
    const starting = [...this.starting.values()];
    await Promise.all(starting.map((pending) => pending.catch(() => undefined)));
    const servers = [...this.servers.values()];
    this.servers.clear();
    await Promise.all(servers.map((server) => server.close()));
  }

  private readyServer(executable: string): Promise<ServerRecord> {
    const existing = this.servers.get(executable);
    if (existing !== undefined && existing.isRunning()) return Promise.resolve(existing);
    const inProgress = this.starting.get(executable);
    if (inProgress !== undefined) return inProgress;
    const promise = this.starter(executable).then((server) => {
      this.servers.set(executable, server);
      server.onUnexpectedExit(() => {
        if (this.servers.get(executable) === server) this.servers.delete(executable);
      });
      return server;
    }).finally(() => {
      if (this.starting.get(executable) === promise) this.starting.delete(executable);
    });
    this.starting.set(executable, promise);
    return promise;
  }
}

function waitWithAbort<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) {
    return Promise.reject(new BridgeError("cancelled", "OpenCode の起動待ちをキャンセルしました"));
  }
  return new Promise<T>((resolve, reject) => {
    const abort = (): void => {
      cleanup();
      reject(new BridgeError("cancelled", "OpenCode の起動待ちをキャンセルしました"));
    };
    const cleanup = (): void => signal.removeEventListener("abort", abort);
    signal.addEventListener("abort", abort, { once: true });
    promise.then(
      (value) => {
        cleanup();
        resolve(value);
      },
      (error: unknown) => {
        cleanup();
        reject(error);
      },
    );
  });
}
