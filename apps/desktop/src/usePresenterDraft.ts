import { useCallback, useRef, useState } from "react";

interface EditVersion {
  readonly generation: number;
  readonly editRevision: number;
}

export function usePresenterDraft() {
  const version = useRef<EditVersion | undefined>(undefined);
  const [value, setValue] = useState("");
  const begin = useCallback((generation: number) => {
    if (version.current !== undefined && generation <= version.current.generation) return;
    version.current = { generation, editRevision: 0 };
    setValue("");
  }, []);
  const receive = useCallback((next: EditVersion, text: string) => {
    const current = version.current;
    if (current !== undefined && (next.generation < current.generation
      || (next.generation === current.generation && next.editRevision < current.editRevision))) return;
    version.current = next;
    setValue(text);
  }, []);
  const edit = useCallback((generation: number, text: string): number | undefined => {
    if (version.current?.generation !== generation) return undefined;
    const editRevision = version.current.editRevision + 1;
    version.current = { generation, editRevision };
    // Reactのcontrolled inputは通知を待たず更新し、古いackでは巻き戻さない。
    setValue(text);
    return editRevision;
  }, []);
  return { value, begin, receive, edit };
}
