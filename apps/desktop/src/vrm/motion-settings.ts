import { vrmError } from "./errors.js";
import type { VRMAnimation } from "@pixiv/three-vrm-animation";
import { loadCooMotions } from "./bundled-motions.js";
import { loadMotionFile, MAX_VRMA_BYTES } from "./motion-file.js";

export const motionSlots = {
  idle: "motions.slots.idle", reply: "motions.slots.reply", thinking: "motions.slots.thinking", joy: "motions.slots.joy", embarrassment: "motions.slots.embarrassment",
  concern: "motions.slots.concern", surprise: "motions.slots.surprise", curiosity: "motions.slots.curiosity", frustration: "motions.slots.frustration",
} as const;
export type MotionSlot = keyof typeof motionSlots;
export type MotionSelection = { readonly kind: "builtin"; readonly id: "idle" | "reply" }
  | { readonly kind: "file"; readonly name: string; readonly data: ArrayBuffer }
  | { readonly kind: "none" };
export type MotionSettings = Record<MotionSlot, MotionSelection>;
export type AssignedMotions = Partial<Record<MotionSlot, VRMAnimation>> & { idle: VRMAnimation };

export function defaultMotion(slot: MotionSlot): MotionSelection {
  return slot === "idle" || slot === "reply" ? { kind: "builtin", id: slot } : { kind: "none" };
}

export function isMotionSlot(value: unknown): value is MotionSlot {
  return typeof value === "string" && Object.hasOwn(motionSlots, value);
}

function validateSelection(value: unknown, slot: MotionSlot): MotionSelection {
  if (typeof value !== "object" || value === null) throw vrmError("vrm.errors.motion.invalidSettings");
  if ("kind" in value) {
    if (value.kind === "none" && slot !== "idle") return { kind: "none" };
    if (value.kind === "builtin" && "id" in value && (value.id === "idle" || value.id === "reply")) return { kind: "builtin", id: value.id };
    if (value.kind === "file" && "name" in value && typeof value.name === "string" && value.name.length > 0 && value.name.length <= 255
      && "data" in value && value.data instanceof ArrayBuffer && value.data.byteLength > 0 && value.data.byteLength <= MAX_VRMA_BYTES) {
      return { kind: "file", name: value.name, data: value.data };
    }
  }
  throw vrmError("vrm.errors.motion.corruptSettings");
}

const STORE = "assignments";
async function openDatabase(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open("coosenpai-motions", 1);
    let blocked = false;
    request.onupgradeneeded = () => request.result.createObjectStore(STORE);
    request.onsuccess = () => { if (blocked) request.result.close(); else resolve(request.result); };
    request.onerror = () => reject(request.error);
    request.onblocked = () => { blocked = true; reject(vrmError("vrm.errors.motion.storeUnavailable")); };
  });
}

async function readStoredMotionSettings(): Promise<MotionSettings> {
  const db = await openDatabase();
  try {
    return await new Promise((resolve, reject) => {
      const transaction = db.transaction(STORE, "readonly");
      const requests = Object.keys(motionSlots).map(slot => ({ slot: slot as MotionSlot, request: transaction.objectStore(STORE).get(slot) }));
      transaction.oncomplete = () => {
        try {
          resolve(Object.fromEntries(requests.map(({ slot, request }) => [slot, request.result === undefined ? defaultMotion(slot) : validateSelection(request.result, slot)])) as MotionSettings);
        } catch (cause) { reject(cause); }
      };
      transaction.onabort = () => reject(transaction.error);
      transaction.onerror = () => reject(transaction.error);
    });
  } finally { db.close(); }
}

export async function readMotionSettings(signal: AbortSignal = new AbortController().signal): Promise<MotionSettings> {
  signal.throwIfAborted();
  const settings = await readStoredMotionSettings();
  for (const selection of Object.values(settings)) {
    signal.throwIfAborted();
    if (selection.kind === "file") await loadMotionFile(selection.data, signal);
  }
  return settings;
}

export async function saveMotionSelection(slot: MotionSlot, selection: MotionSelection, signal: AbortSignal): Promise<void> {
  signal.throwIfAborted();
  if (!isMotionSlot(slot)) throw vrmError("vrm.errors.motion.invalidSlot");
  const checked = validateSelection(selection, slot);
  if (checked.kind === "file") await loadMotionFile(checked.data, signal);
  const db = await openDatabase();
  try {
    signal.throwIfAborted();
    await new Promise<void>((resolve, reject) => {
      const transaction = db.transaction(STORE, "readwrite");
      const abort = () => {
        // A committed transaction cannot be canceled before its completion event arrives.
        try { transaction.abort(); } catch (cause) {
          if (!(cause instanceof DOMException && cause.name === "InvalidStateError")) reject(cause);
        }
      };
      const cleanup = () => signal.removeEventListener("abort", abort);
      signal.addEventListener("abort", abort, { once: true });
      transaction.oncomplete = () => { cleanup(); resolve(); };
      transaction.onabort = () => { cleanup(); reject(signal.aborted ? signal.reason : transaction.error); };
      transaction.onerror = () => { cleanup(); reject(transaction.error); };
      try { transaction.objectStore(STORE).put(checked, slot); }
      catch (cause) { cleanup(); abort(); reject(cause); }
    });
  } finally { db.close(); }

}

export async function loadAssignedMotions(signal: AbortSignal): Promise<AssignedMotions> {
  const settings = await readStoredMotionSettings();
  signal.throwIfAborted();
  const bundled = await loadCooMotions(signal);
  // 同梱の返答だけ、最初の掌を開く区間を使う。持ち込み素材は全区間を再生する。
  bundled.reply.duration = Math.min(6.5, bundled.reply.duration);
  const resolved: Partial<Record<MotionSlot, VRMAnimation>> = {};
  for (const slot of Object.keys(motionSlots) as MotionSlot[]) {
    signal.throwIfAborted();
    const selection = settings[slot];
    if (selection.kind === "builtin") resolved[slot] = bundled[selection.id];
    else if (selection.kind === "file") resolved[slot] = await loadMotionFile(selection.data, signal);
  }
  if (!resolved.idle) throw vrmError("vrm.errors.motion.missingIdle");
  return { ...resolved, idle: resolved.idle };
}
