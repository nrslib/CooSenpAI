import type { VrmModelFile } from "./model-file.js";
import { vrmError } from "./errors.js";
import { DEFAULT_VRM_IMAGE_QUALITY, isVrmImageQuality, type VrmImageQuality } from "./texture-quality.js";

const STORE = "avatar";
const KEY = "selected";
const QUALITY_KEY = "image-quality";

async function openDatabase(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open("coosenpai-vrm", 1);
    let blocked = false;
    request.onupgradeneeded = () => request.result.createObjectStore(STORE);
    request.onsuccess = () => {
      if (blocked) request.result.close();
      else resolve(request.result);
    };
    request.onerror = () => reject(request.error);
    request.onblocked = () => {
      blocked = true;
      reject(vrmError("vrm.errors.storeUnavailable"));
    };
  });
}

export async function readVrmModel(): Promise<VrmModelFile | undefined> {
  const db = await openDatabase();
  try {
    return await new Promise((resolve, reject) => {
      const transaction = db.transaction(STORE, "readonly");
      const request = transaction.objectStore(STORE).get(KEY);
      transaction.oncomplete = () => resolve(request.result);
      transaction.onabort = () => reject(transaction.error);
      transaction.onerror = () => reject(transaction.error);
    });
  } finally { db.close(); }
}

export async function saveVrmModel(model: VrmModelFile | undefined): Promise<void> {
  const db = await openDatabase();
  try {
    await new Promise<void>((resolve, reject) => {
      const transaction = db.transaction(STORE, "readwrite");
      const store = transaction.objectStore(STORE);
      if (model === undefined) store.delete(KEY);
      else store.put(model, KEY);
      transaction.oncomplete = () => resolve();
      transaction.onabort = () => reject(transaction.error);
      transaction.onerror = () => reject(transaction.error);
    });
  } finally { db.close(); }
}

export async function readVrmImageQuality(): Promise<VrmImageQuality> {
  const db = await openDatabase();
  try {
    return await new Promise((resolve, reject) => {
      const transaction = db.transaction(STORE, "readonly");
      const request = transaction.objectStore(STORE).get(QUALITY_KEY);
      transaction.oncomplete = () => {
        if (request.result === undefined) {
          resolve(DEFAULT_VRM_IMAGE_QUALITY);
          return;
        }
        if (!isVrmImageQuality(request.result)) {
          reject(vrmError("vrm.errors.invalidQualitySettings"));
          return;
        }
        resolve(request.result);
      };
      transaction.onabort = () => reject(transaction.error);
      transaction.onerror = () => reject(transaction.error);
    });
  } finally { db.close(); }
}

export async function saveVrmImageQuality(quality: VrmImageQuality): Promise<void> {
  if (!isVrmImageQuality(quality)) throw vrmError("vrm.errors.invalidQualitySettings");
  const db = await openDatabase();
  try {
    await new Promise<void>((resolve, reject) => {
      const transaction = db.transaction(STORE, "readwrite");
      transaction.oncomplete = () => resolve();
      transaction.onabort = () => reject(transaction.error);
      transaction.onerror = () => reject(transaction.error);
      try { transaction.objectStore(STORE).put(quality, QUALITY_KEY); }
      catch (cause) { reject(cause); }
    });
  } finally { db.close(); }
}
