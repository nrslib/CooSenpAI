import type { IpcResult } from "./types.js";
import type { SettingsAppearancePreview } from "./settings-form.js";

type PreviewSender = (
  preview?: SettingsAppearancePreview,
) => Promise<IpcResult<null>>;

export function createAppearancePreviewQueue(send: PreviewSender): PreviewSender {
  let tail: Promise<void> = Promise.resolve();

  return (preview) => {
    const current = tail.then(() => send(preview));
    tail = current.then(
      () => undefined,
      () => undefined,
    );
    return current;
  };
}
