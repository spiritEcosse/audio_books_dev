import { convertFileSrc } from "@tauri-apps/api/core";
import { useState } from "react";
import { api, type AssetKind, type MediaInfo, type Scene, type SceneAsset } from "./api";
import { BusyButton } from "./BusyButton";
import { type MediaBusy, type MediaChange, findAsset, formatCredits, rowLocked, sceneCredits } from "./mediaCost";
import { RANGE_OPTIONS, type RangeMode, costTip, rangeBounds, rangeCost } from "./rangeRun";

interface Props {
  storyId: number;
  scene: Scene;
  media: MediaInfo;
  /** All scenes of the story: a run can cover a range of them. */
  scenes: readonly Scene[];
  disabled: boolean;
  /** The media actions running right now (any scene). */
  busy: readonly MediaBusy[];
  onChange: MediaChange;
  /** Starts a run over this scene and the following / preceding ones (see rangeRun). */
  onRange: (kind: AssetKind, mode: RangeMode, position: number) => void;
}

const KIND_LABEL: Record<AssetKind, string> = { image: "Картинка", audio: "Озвучка" };

function statusText(asset: SceneAsset | undefined): string {
  if (!asset) return "не создано";
  if (asset.status === "failed") return `ошибка: ${asset.error ?? "неизвестно"}`;
  if (asset.status === "requested") {
    if (asset.awaiting_result) return "оплачено, результат ещё не получен";
    if (asset.in_progress) return "генерируется…";
    return asset.stale ? "в очереди (текст изменился, обновите запрос)" : "в очереди";
  }
  return asset.stale ? "устарело: текст сцены изменился" : "готово";
}

function Preview({ kind, path }: { kind: AssetKind; path: string }) {
  const src = convertFileSrc(path);
  return kind === "image" ? (
    <img className="scene-image" src={src} alt="Иллюстрация сцены" />
  ) : (
    <audio className="scene-audio" controls src={src} />
  );
}

const UPSCALE_BUSY_PREFIX = "Улучшаю";

/** Free local enlargement of a finished image to the video size (Upscayl, then ffmpeg). */
function UpscaleRow({ asset, storyId, position, rowKey, locked, busy, onChange }: {
  asset: SceneAsset;
  storyId: number;
  position: number;
  rowKey: string;
  locked: boolean;
  busy: readonly MediaBusy[];
  onChange: MediaChange;
}) {
  const running = busy.find((op) => op.key === rowKey && op.label.startsWith(UPSCALE_BUSY_PREFIX));
  const done = asset.upscaled !== null;
  return (
    <div className="upscale">
      <span className="muted">Для видео:</span>
      <button
        type="button"
        disabled={locked || done}
        title={
          done
            ? "Готово: файл лежит рядом с картинкой"
            : "Увеличивает через Upscayl и ffmpeg до точного размера. Бесплатно, около 25 секунд"
        }
        onClick={() =>
          onChange(() => api.upscaleImages(storyId, position), {
            key: rowKey,
            label: `${UPSCALE_BUSY_PREFIX} до 4K… около 25 секунд`,
          })
        }
      >
        {done ? "4K ✓" : "Улучшить до 4K"}
      </button>
      {running && (
        <span>
          <span className="spinner" aria-hidden="true" /> {running.label}
        </span>
      )}
    </div>
  );
}

const QUEUE_LABEL = "Поставить в очередь";
const QUEUE_BUSY = "Ставлю в очередь…";
const PRICE_BUSY = "Измеряю точную цену…";
const COLLECT_BUSY = "Забираю результат… до 10 минут, кредиты не тратятся";
const GENERATE_BUSY = "Генерирую… обычно до пары минут";

interface MainAction {
  label: string;
  /** What the button says while its action runs. */
  busyLabel: string;
  /** null: nothing to do right now, the button stays disabled. */
  run: (() => Promise<MediaInfo>) | null;
}

function rowCalls(kind: AssetKind, storyId: number, position: number) {
  return {
    request: () => api.requestAsset(storyId, position, kind),
    estimate: () => (kind === "audio" ? api.estimateAudioQueue : api.estimateImageQueue)(storyId, position, position),
    generate: () => (kind === "audio" ? api.generateAudioQueue : api.generateImageQueue)(storyId, position, position),
  };
}

/** The one button of an asset row: queue it, then generate (the price is on the button and in the tooltip). */
function mainAction(
  asset: SceneAsset | undefined,
  kind: AssetKind,
  storyId: number,
  scene: Scene,
  media: MediaInfo,
): MainAction {
  const { request, generate } = rowCalls(kind, storyId, scene.position);
  if (asset?.status !== "requested") return { label: QUEUE_LABEL, run: request, busyLabel: QUEUE_BUSY };
  if (asset.awaiting_result) {
    return { label: "Забрать результат", run: () => api.collectMedia(storyId, kind), busyLabel: COLLECT_BUSY };
  }
  if (asset.in_progress) return { label: "Генерируется…", run: null, busyLabel: GENERATE_BUSY };
  if (asset.stale) return { label: "Обновить запрос", run: request, busyLabel: QUEUE_BUSY };
  const exact = asset.estimate_credits != null;
  const credits = asset.estimate_credits ?? sceneCredits(media, scene, kind);
  const verb = asset.file_path ? "Перегенерировать" : "Сгенерировать";
  return {
    label: `${verb} (${exact ? "" : "≈"}${formatCredits(credits)} кр.)`,
    run: generate,
    busyLabel: GENERATE_BUSY,
  };
}

function AssetRow({ kind, scene, scenes, storyId, media, disabled, busy, onChange, onRange }: Props & { kind: AssetKind }) {
  const [menuOpen, setMenuOpen] = useState(false);
  const rowKey = `${kind}:${scene.position}`;
  const locked = disabled || rowLocked(busy, rowKey);
  const anyBusy = busy.length > 0;
  const asset = findAsset(media, scene.position, kind);
  const action = mainAction(asset, kind, storyId, scene, media);
  const queuedFresh = asset?.status === "requested" && !asset.stale && !asset.in_progress;
  const mainTip =
    asset?.status === "requested"
      ? costTip(rangeCost(media, scenes, kind, scene.position, scene.position))
      : `Стоимость: ≈${formatCredits(sceneCredits(media, scene, kind))} кр. Кредиты не тратятся, пока не нажмёте «Сгенерировать»`;
  const costOf = (mode: RangeMode) => {
    const { from, to } = rangeBounds(scenes, mode, scene.position);
    return rangeCost(media, scenes, kind, from, to);
  };
  return (
    <div className="asset">
      <div className="asset-head">
        <strong>{KIND_LABEL[kind]}</strong>
        <span className={`status ${asset?.status ?? "none"}`}>{statusText(asset)}</span>
        <span className="muted">
          {asset?.estimate_credits != null
            ? `${formatCredits(asset.estimate_credits)} кр. (точно)`
            : `≈${formatCredits(sceneCredits(media, scene, kind))} кр.`}
        </span>
        <span className="run-split">
          <BusyButton
            busy={busy}
            tag={rowKey}
            disabled={locked || action.run === null}
            title={mainTip}
            onClick={() => action.run && onChange(action.run, { key: rowKey, label: action.busyLabel })}
          >
            {action.label}
          </BusyButton>
          <button
            type="button"
            className="run-split-toggle"
            aria-label="Варианты запуска"
            aria-expanded={menuOpen}
            disabled={locked}
            title="Запустить несколько сцен"
            onClick={() => setMenuOpen((open) => !open)}
          >
            ▾
          </button>
          {menuOpen && (
            <div className="run-menu" role="menu">
              <button
                type="button"
                role="menuitem"
                disabled={locked || asset?.in_progress === true}
                onClick={() => {
                  setMenuOpen(false);
                  const calls = rowCalls(kind, storyId, scene.position);
                  const measure = queuedFresh ? calls.estimate : () => calls.request().then(calls.estimate);
                  onChange(measure, { key: rowKey, label: PRICE_BUSY });
                }}
              >
                <strong>Получить точную цену из ElevenLabs</strong>
                <span>
                  {kind === "audio" ? "Озвучка" : "Картинка"}, эта сцена. Бесплатно, около 15 секунд
                  {queuedFresh ? "" : "; сначала поставит в очередь"}
                </span>
              </button>
              {RANGE_OPTIONS.map((option) => {
                const cost = costOf(option.mode);
                return (
                  <button
                    key={option.mode}
                    type="button"
                    role="menuitem"
                    disabled={cost.count === 0 || anyBusy}
                    title={costTip(cost)}
                    onClick={() => {
                      setMenuOpen(false);
                      onRange(kind, option.mode, scene.position);
                    }}
                  >
                    <strong>{option.label}</strong>
                    <span>{option.hint}</span>
                  </button>
                );
              })}
            </div>
          )}
        </span>
      </div>
      {asset?.awaiting_result && (
        <div className="muted">
          Генерация уже оплачена. Если результат не приходит, можно{" "}
          <button
            type="button"
            disabled={locked}
            title="Следующая генерация будет оплачена заново"
            onClick={() => {
              if (window.confirm("Отказаться от оплаченной генерации? Новая будет стоить кредитов заново.")) {
                onChange(() => api.abandonGeneration(storyId, scene.position, kind), {
                  key: rowKey,
                  label: "Отказываюсь…",
                });
              }
            }}
          >
            отказаться и сгенерировать заново
          </button>
        </div>
      )}
      {kind === "image" && asset?.status === "ready" && asset.file_path && (
        <UpscaleRow
          asset={asset}
          storyId={storyId}
          position={scene.position}
          rowKey={rowKey}
          locked={locked}
          busy={busy}
          onChange={onChange}
        />
      )}
      {asset?.file_path && <Preview kind={kind} path={asset.file_path} />}
    </div>
  );
}

export async function readBytes(file: File): Promise<number[]> {
  return Array.from(new Uint8Array(await file.arrayBuffer()));
}

function ReferenceRow({ scene, storyId, media, disabled, onChange }: Props) {
  const reference = media.references.find((ref) => ref.position === scene.position);
  const fileName = reference?.file_path.split(/[\\/]/).pop();
  return (
    <div className="asset reference">
      <label>
        Референс (необязательно)
        <input
          type="file"
          accept="image/png,image/jpeg,image/webp"
          disabled={disabled}
          onChange={(e) => {
            const file = e.target.files?.[0];
            e.target.value = "";
            if (file) {
              onChange(async () =>
                api.setSceneReference(storyId, scene.position, file.name, await readBytes(file)),
              );
            }
          }}
        />
      </label>
      {reference && (
        <span>
          {fileName}{" "}
          <button disabled={disabled} onClick={() => onChange(() => api.clearSceneReference(storyId, scene.position))}>
            ×
          </button>
        </span>
      )}
      {reference && <img className="scene-reference" src={convertFileSrc(reference.file_path)} alt="Референс" />}
    </div>
  );
}

/** Which of the story's characters go into this scene's image (all of them unless switched off). */
function CharacterRow({ scene, storyId, media, disabled, onChange }: Props) {
  if (media.characters.length === 0) return null;
  const isOff = (characterId: number) =>
    media.character_off.some((off) => off.position === scene.position && off.character_id === characterId);
  return (
    <div className="asset scene-characters">
      <span>Персонажи в картинке:</span>
      {media.characters.map((character) => (
        <label key={character.id}>
          <input
            type="checkbox"
            checked={!isOff(character.id)}
            disabled={disabled}
            onChange={(e) =>
              onChange(() => api.setSceneCharacter(storyId, scene.position, character.id, e.target.checked))
            }
          />{" "}
          {character.name}
        </label>
      ))}
    </div>
  );
}

export function SceneMedia(props: Props) {
  return (
    <div className="scene-media">
      <AssetRow {...props} kind="image" />
      <CharacterRow {...props} />
      <ReferenceRow {...props} />
      <AssetRow {...props} kind="audio" />
    </div>
  );
}
