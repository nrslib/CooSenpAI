import { existsSync } from "node:fs";
import { copyFile, mkdtemp, mkdir, realpath, rm, stat } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { basename, isAbsolute, relative, resolve, join } from "node:path";
import { BridgeError } from "../errors.js";
import type { ProviderCallOptions } from "../types.js";

export interface ObservationAccess {
  readonly options: ProviderCallOptions;
  readonly readableDirectories: readonly string[];
  cleanup(): Promise<void>;
}

export function observationDirectories(productRoot = process.env.COOSENPAI_HOME): string[] {
  if (productRoot === undefined) return [];
  return ["frames", "transcripts"]
    .map((name) => resolve(productRoot, "state", name))
    .filter((directory) => existsSync(directory));
}

/** 話者名照会では候補 transcript だけの一時領域を作り、prompt 内 path も差し替える。 */
export async function prepareObservationAccess(options: ProviderCallOptions): Promise<ObservationAccess> {
  if (options.allowedTranscriptPaths === undefined) {
    return {
      options,
      readableDirectories: observationDirectories(),
      cleanup: async () => {},
    };
  }
  if (options.allowedTranscriptPaths.length === 0) {
    return { options, readableDirectories: [], cleanup: async () => {} };
  }

  const productRoot = resolve(process.env.COOSENPAI_HOME ?? join(homedir(), ".coosenpai"));
  let transcriptRoot: string;
  try {
    transcriptRoot = await realpath(join(productRoot, "state", "transcripts"));
  } catch (error) {
    throw new BridgeError("protocol", "許可された transcript directory を確認できません", { cause: error });
  }
  const stagingRoot = await mkdtemp(join(tmpdir(), "coosenpai-transcript-allowlist-"));
  try {
    const replacements = new Map<string, string>();
    const uniquePaths = [...new Set(options.allowedTranscriptPaths)];
    for (const [index, originalPath] of uniquePaths.entries()) {
      if (!isAbsolute(originalPath)) throw new BridgeError("protocol", "許可 transcript path は絶対 path が必要です");
      const resolvedPath = await realpath(originalPath).catch((error: unknown) => {
        throw new BridgeError("protocol", "許可された transcript file を確認できません", { cause: error });
      });
      const pathFromRoot = relative(transcriptRoot, resolvedPath);
      const parentPrefix = `..${process.platform === "win32" ? "\\" : "/"}`;
      if (pathFromRoot === "" || pathFromRoot === ".." || pathFromRoot.startsWith(parentPrefix) || isAbsolute(pathFromRoot)) {
        throw new BridgeError("protocol", "許可された transcript path が transcript directory 外です");
      }
      const fileInfo = await stat(resolvedPath);
      if (!fileInfo.isFile()) throw new BridgeError("protocol", "許可された transcript path は file ではありません");
      const targetDirectory = join(stagingRoot, String(index));
      await mkdir(targetDirectory, { recursive: true });
      const stagedPath = join(targetDirectory, basename(resolvedPath));
      await copyFile(resolvedPath, stagedPath);
      replacements.set(originalPath, stagedPath);
      replacements.set(resolvedPath, stagedPath);
    }
    let message = options.message;
    let systemPrompt = options.systemPrompt;
    for (const [source, target] of replacements) {
      message = message.replaceAll(source, target);
      systemPrompt = systemPrompt.replaceAll(source, target);
    }
    const stagedToOriginal = new Map(
      uniquePaths.map((originalPath) => [replacements.get(originalPath)!, originalPath]),
    );
    const onToolExecution = options.onToolExecution === undefined
      ? undefined
      : (execution: Parameters<NonNullable<ProviderCallOptions["onToolExecution"]>>[0]): void => {
        options.onToolExecution?.({
          ...execution,
          input: replacePaths(execution.input, stagedToOriginal),
          output: replacePaths(execution.output, stagedToOriginal),
        });
      };
    return {
      options: {
        ...options,
        message,
        systemPrompt,
        ...(onToolExecution === undefined ? {} : { onToolExecution }),
      },
      readableDirectories: [stagingRoot],
      cleanup: () => rm(stagingRoot, { recursive: true, force: true }),
    };
  } catch (error) {
    await rm(stagingRoot, { recursive: true, force: true });
    throw error;
  }
}

function replacePaths(value: unknown, replacements: ReadonlyMap<string, string>): unknown {
  if (typeof value === "string") {
    let result = value;
    for (const [stagedPath, originalPath] of replacements) {
      result = result.replaceAll(stagedPath, originalPath);
    }
    return result;
  }
  if (Array.isArray(value)) return value.map((entry) => replacePaths(entry, replacements));
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, entry]) => [key, replacePaths(entry, replacements)]),
    );
  }
  return value;
}
