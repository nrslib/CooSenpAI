import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const env = { ...process.env, CI: process.env.CI ?? "true" };
if (process.platform === "darwin") {
  const toolsPath = fileURLToPath(new URL("../../../tools/", import.meta.url));
  env.PATH = env.PATH ? `${toolsPath}:${env.PATH}` : toolsPath;
}

const child = spawn("tauri", process.argv.slice(2), {
  cwd: process.cwd(),
  env,
  stdio: "inherit",
});

child.once("error", (error) => {
  console.error(error.message);
  process.exitCode = 1;
});
child.once("exit", (code, signal) => {
  process.exitCode = signal === null ? (code ?? 1) : 1;
});
