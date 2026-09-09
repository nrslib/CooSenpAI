import { readMotionSettings, saveMotionSelection } from "./motion-settings.js";
import { localizeVrmError, vrmError } from "./errors.js";
import type { Locale } from "../i18n/index.js";
import type { MotionInput, MotionStorageCommand, MotionView } from "./motion-view.js";

export async function executeMotionStorage(command: Exclude<MotionStorageCommand, { kind: "release" }>, files: Map<string, File>, signal: AbortSignal, locale: Locale): Promise<MotionInput> {
  try {
    if (command.kind === "load") {
      const stored = await readMotionSettings(signal);
      const settings = Object.fromEntries(Object.entries(stored).map(([slot, value]) => [slot, value.kind === "file" ? { kind: "file", name: value.name } : value])) as MotionView["settings"];
      return { type: "storageLoaded", generation: command.generation, settings, error: null };
    }
    if (command.kind === "import") {
      const file = files.get(command.fileId);
      if (file === undefined) throw vrmError("vrm.errors.motion.invalidSettings");
      signal.throwIfAborted(); const data = await file.arrayBuffer(); signal.throwIfAborted();
      await saveMotionSelection(command.slot, { kind: "file", name: file.name, data }, signal);
    } else { await saveMotionSelection(command.slot, command.selection, signal); }
    return { type: "storageSaved", generation: command.generation, error: null };
  } catch (cause) {
    const error = localizeVrmError(cause, locale);
    return command.kind === "load" ? { type: "storageLoaded", generation: command.generation, settings: null, error } : { type: "storageSaved", generation: command.generation, error };
  } finally { if (command.kind === "import") files.delete(command.fileId); }
}
