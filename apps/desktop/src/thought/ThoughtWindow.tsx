import { useEffect, useState, type ReactElement } from "react";

import type { ThoughtWindowView } from "../app-view.js";
import { ThoughtBubble } from "../components/ThoughtBubble.js";
import { desktopApi, setIpcLocale } from "../ipc.js";
import { applyAppearance } from "../appearance.js";
import { I18nProvider } from "../i18n/index.js";

export function ThoughtWindow(): ReactElement {
  const [view, setView] = useState<ThoughtWindowView>();

  useEffect(() => {
    const thought = desktopApi.subscribeThoughtView((incoming) => {
      applyAppearance({ theme: incoming.theme, font: incoming.font });
      setIpcLocale(incoming.language);
      setView(incoming);
    });
    void thought.ready.then(() => desktopApi.ready());
    return thought.dispose;
  }, []);

  return <I18nProvider locale={view?.language ?? "ja"}>
    <div className="thought-window">
      <ThoughtBubble view={view?.thought ?? null} bubbleTail={view?.bubbleTail ?? false} />
    </div>
  </I18nProvider>;
}
