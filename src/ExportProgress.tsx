import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import type { ExportProgress as ExportProgressEvent } from "./api";

const EVENT = "export-progress";
/** Too early to extrapolate: the first clips say little about the rest. */
const ETA_MIN_PERCENT = 3;
const SECONDS_PER_MINUTE = 60;

interface Shown {
  readonly event: ExportProgressEvent;
  /** Seconds left, or null while it is too early to tell. */
  readonly etaSecs: number | null;
}

function formatEta(secs: number): string {
  const minutes = Math.round(secs / SECONDS_PER_MINUTE);
  return minutes < 1 ? "меньше минуты" : `≈ ${minutes} мин`;
}

function stageLabel(event: ExportProgressEvent): string {
  return event.stage === "clip" ? `Сцена ${event.scene} из ${event.scenes}` : "Склейка и озвучка";
}

/** Live progress of a video export: a bar, the scene being rendered and the time left. */
export function ExportProgress({ storyId, active }: { storyId: number; active: boolean }) {
  const [shown, setShown] = useState<Shown | null>(null);

  useEffect(() => {
    if (!active) {
      setShown(null);
      return;
    }
    const startedAt = Date.now();
    let stopped = false;
    let unlisten: (() => void) | undefined;
    listen<ExportProgressEvent>(EVENT, ({ payload }) => {
      if (payload.story_id !== storyId) return;
      const elapsed = (Date.now() - startedAt) / 1000;
      const etaSecs =
        payload.percent >= ETA_MIN_PERCENT ? (elapsed * (100 - payload.percent)) / payload.percent : null;
      setShown({ event: payload, etaSecs });
    }).then((stop) => (stopped ? stop() : (unlisten = stop)));
    return () => {
      stopped = true;
      unlisten?.();
    };
  }, [active, storyId]);

  if (!active || !shown) return null;
  const { event, etaSecs } = shown;
  return (
    <div className="export-progress" role="status">
      <progress max={100} value={event.percent} />
      <span>
        {stageLabel(event)} · {Math.round(event.percent)}%
        {etaSecs !== null && ` · осталось ${formatEta(etaSecs)}`}
      </span>
    </div>
  );
}
