import { useState } from "react";
import { api, type AssetKind, type MediaInfo, type Scene } from "./api";
import { BusyButton } from "./BusyButton";
import { type MediaBusy, type MediaChange, formatCredits } from "./mediaCost";
import { type RangeMode, queueRange, rangeBounds, rangeCost } from "./rangeRun";
import { SceneMedia } from "./SceneMedia";

interface Props {
  storyId: number;
  scenes: readonly Scene[];
  media: MediaInfo | null;
  disabled: boolean;
  busy: readonly MediaBusy[];
  onMediaChange: MediaChange;
}

const AUDIO_TAG = /(\[[^\]]*\])/;
const withoutAudioTags = (text: string) =>
  text.replace(/\s*\[[^\]]*\]\s*/g, " ").trim();

/** Narration text with audio tags like [softly] rendered dimmed so the spoken words stay readable. */
function NarrationText({ text, showTags }: { text: string; showTags: boolean }) {
  if (!showTags) return <p>{withoutAudioTags(text)}</p>;
  return (
    <p>
      {text.split(AUDIO_TAG).map((part, index) =>
        AUDIO_TAG.test(part) ? (
          <span key={index} className="audio-tag">{part}</span>
        ) : (
          part
        ),
      )}
    </p>
  );
}

const RANGE_KEY = "range";
const KIND_NAME: Record<AssetKind, string> = { image: "Картинки", audio: "Озвучка" };

/** A run over several scenes that has been queued and priced, waiting for the go-ahead. */
interface RunPlan {
  readonly kind: AssetKind;
  readonly from: number;
  readonly to: number;
}

const PREVIEW_CHARS = 60;

function previewOf(text: string): string {
  const plain = withoutAudioTags(text);
  return plain.length > PREVIEW_CHARS ? `${plain.slice(0, PREVIEW_CHARS)}…` : plain;
}

export function SceneList({ storyId, scenes, media, disabled, busy, onMediaChange }: Props) {
  const [showTags, setShowTags] = useState(true);
  const [plan, setPlan] = useState<RunPlan | null>(null);
  const startRange = (kind: AssetKind, mode: RangeMode, position: number) => {
    if (!media) return;
    const { from, to } = rangeBounds(scenes, mode, position);
    setPlan({ kind, from, to });
    onMediaChange(() => queueRange(storyId, kind, scenes, media, from, to), {
      key: RANGE_KEY,
      label: "Ставлю в очередь…",
    });
  };
  const planCost = plan && media ? rangeCost(media, scenes, plan.kind, plan.from, plan.to) : null;
  const runPlan = () => {
    if (!plan) return;
    const { kind, from, to } = plan;
    setPlan(null);
    onMediaChange(() => (kind === "audio" ? api.generateAudioQueue : api.generateImageQueue)(storyId, from, to), {
      key: RANGE_KEY,
      label: `Генерирую ${planCost?.count ?? 0} шт.… это может занять несколько минут`,
    });
  };
  const [collapsedIds, setCollapsedIds] = useState<ReadonlySet<number>>(new Set());
  const toggleScene = (id: number) =>
    setCollapsedIds((prev) => {
      const next = new Set(prev);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  const allCollapsed = scenes.every((scene) => collapsedIds.has(scene.id));
  const toggleAll = () => setCollapsedIds(allCollapsed ? new Set() : new Set(scenes.map((s) => s.id)));
  if (scenes.length === 0) {
    return <p className="muted">Сцен пока нет. Попросите Claude в чате разбить историю на сцены.</p>;
  }
  return (
    <>
      <div className="scene-toolbar">
        <button type="button" onClick={toggleAll}>
          {allCollapsed ? "Развернуть все сцены" : "Свернуть все сцены"}
        </button>
        <label className="tag-toggle">
          <input type="checkbox" checked={showTags} onChange={(e) => setShowTags(e.target.checked)} />
          Показывать теги озвучки
        </label>
        {plan && planCost && planCost.count > 0 && (
          <div className="run-plan">
            <span>
              {KIND_NAME[plan.kind]}, сцены {plan.from + 1}–{plan.to + 1}: {planCost.count} шт.,{" "}
              {planCost.exact ? "" : "≈"}
              {formatCredits(planCost.credits)} кр.
            </span>
            <BusyButton busy={busy} tag={RANGE_KEY} disabled={disabled || busy.length > 0} onClick={runPlan}>
              Сгенерировать
            </BusyButton>
            <button onClick={() => setPlan(null)}>Отмена</button>
          </div>
        )}
      </div>
    <ol className="scenes">
      {scenes.map((scene) => (
        <li key={scene.id}>
          <button
            type="button"
            className="scene-toggle"
            aria-expanded={!collapsedIds.has(scene.id)}
            onClick={() => toggleScene(scene.id)}
          >
            {collapsedIds.has(scene.id) ? "▸" : "▾"} Сцена {scene.position + 1}
            {collapsedIds.has(scene.id) && <span className="muted"> — {previewOf(scene.text)}</span>}
          </button>
          {!collapsedIds.has(scene.id) && <NarrationText text={scene.text} showTags={showTags} />}
          {!collapsedIds.has(scene.id) && <p className="prompt">🖼 {scene.image_prompt}</p>}
          {media && !collapsedIds.has(scene.id) && (
            <SceneMedia
              storyId={storyId}
              scene={scene}
              media={media}
              scenes={scenes}
              disabled={disabled}
              busy={busy}
              onChange={onMediaChange}
              onRange={startRange}
            />
          )}
        </li>
      ))}
    </ol>
    </>
  );
}
