import { useEffect, useState } from "react";
import { api, type Revision } from "./api";

interface Props {
  storyId: number;
  /** Changes whenever the story's stored state changes, to re-read the list. */
  refreshKey: unknown;
  disabled: boolean;
  onRestore: (revisionId: number) => void;
}

const KIND_LABEL = { story_text: "Текст истории", scenes: "Сцены" } as const;

function formatTime(sqliteUtc: string): string {
  return new Date(`${sqliteUtc.replace(" ", "T")}Z`).toLocaleString();
}

export function Revisions({ storyId, refreshKey, disabled, onRestore }: Props) {
  const [items, setItems] = useState<readonly Revision[]>([]);

  useEffect(() => {
    let cancelled = false;
    api
      .listRevisions(storyId)
      .then((list) => !cancelled && setItems(list))
      .catch(() => !cancelled && setItems([]));
    return () => {
      cancelled = true;
    };
  }, [storyId, refreshKey]);

  return (
    <details className="revisions">
      <summary>Версии ({items.length})</summary>
      {items.length === 0 ? (
        <p className="muted">Пока нет. Версия сохраняется перед каждой заменой текста или сцен.</p>
      ) : (
        <ul>
          {items.map((r) => (
            <li key={r.id}>
              <div>
                <strong>{KIND_LABEL[r.kind]}</strong>{" "}
                <span className="muted">{formatTime(r.created_at)}</span>
                <div className="rev-summary">{r.summary}</div>
              </div>
              <button className="secondary" disabled={disabled} onClick={() => onRestore(r.id)}>
                Восстановить
              </button>
            </li>
          ))}
        </ul>
      )}
      <p className="muted small">Восстановление тоже сохраняет текущее состояние, его можно отменить.</p>
    </details>
  );
}
