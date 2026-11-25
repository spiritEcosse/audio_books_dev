import { api, type AssetKind, type MediaInfo, type Scene, type SceneAsset } from "./api";
import { findAsset, formatCredits, sceneCredits } from "./mediaCost";

export type RangeMode = "from" | "till";

export const RANGE_OPTIONS: readonly { mode: RangeMode; label: string; hint: string }[] = [
  { mode: "from", label: "Запустить отсюда", hint: "Эта сцена и все следующие. Готовое пропускается" },
  { mode: "till", label: "Запустить до этой", hint: "Эта сцена и все предыдущие. Готовое пропускается" },
];

export interface RangeCost {
  /** Scenes that would be generated. */
  readonly count: number;
  readonly credits: number;
  /** Every one of them already has an exact estimate from ElevenLabs. */
  readonly exact: boolean;
}

/** Inclusive scene positions covered by a run started at `position`. */
export function rangeBounds(
  scenes: readonly Scene[],
  mode: RangeMode,
  position: number,
): { from: number; to: number } {
  const positions = scenes.map((scene) => scene.position);
  return mode === "from"
    ? { from: position, to: Math.max(...positions) }
    : { from: Math.min(...positions), to: position };
}

/** Ready-and-current assets and ones already being generated are left alone; the rest is (re)generated. */
function wantsRun(asset: SceneAsset | undefined): boolean {
  if (!asset) return true;
  if (asset.in_progress) return false;
  return asset.status !== "ready" || asset.stale;
}

/** A request is missing, failed or outdated: it has to be (re)queued before it can be priced. */
function wantsRequest(asset: SceneAsset | undefined): boolean {
  return wantsRun(asset) && (asset?.status !== "requested" || asset.stale);
}

export function rangeCost(
  media: MediaInfo,
  scenes: readonly Scene[],
  kind: AssetKind,
  from: number,
  to: number,
): RangeCost {
  return scenes
    .filter((scene) => scene.position >= from && scene.position <= to)
    .reduce<RangeCost>(
      (total, scene) => {
        const asset = findAsset(media, scene.position, kind);
        if (!wantsRun(asset)) return total;
        const exact = asset?.status === "requested" && !asset.stale && asset.estimate_credits != null;
        return {
          count: total.count + 1,
          credits: total.credits + (exact ? (asset?.estimate_credits ?? 0) : sceneCredits(media, scene, kind)),
          exact: total.exact && exact,
        };
      },
      { count: 0, credits: 0, exact: true },
    );
}

const ALL_KINDS: readonly AssetKind[] = ["image", "audio"];

/** The cost of running both images and narration over the range. */
export function allCost(media: MediaInfo, scenes: readonly Scene[], from: number, to: number): RangeCost {
  return ALL_KINDS.map((kind) => rangeCost(media, scenes, kind, from, to)).reduce<RangeCost>(
    (total, part) => ({
      count: total.count + part.count,
      credits: total.credits + part.credits,
      exact: total.exact && part.exact,
    }),
    { count: 0, credits: 0, exact: true },
  );
}

/** Tooltip text: what a run over this range would cost. */
export function costTip(cost: RangeCost): string {
  if (cost.count === 0) return "Запускать нечего: всё готово или уже генерируется";
  return `Этот запуск: ${cost.exact ? "" : "≈"}${formatCredits(cost.credits)} кр. (${cost.count} шт.)`;
}

/** Queues what images and narration still need over the range (instant, nothing is spent). */
export async function queueAll(
  storyId: number,
  scenes: readonly Scene[],
  media: MediaInfo,
  from: number,
  to: number,
): Promise<MediaInfo> {
  let latest = media;
  for (const kind of ALL_KINDS) {
    latest = await queueRange(storyId, kind, scenes, latest, from, to);
  }
  return latest;
}

/** Queues what the range still needs (instant, nothing is spent). */
export async function queueRange(
  storyId: number,
  kind: AssetKind,
  scenes: readonly Scene[],
  media: MediaInfo,
  from: number,
  to: number,
): Promise<MediaInfo> {
  let latest = media;
  for (const scene of scenes) {
    if (scene.position < from || scene.position > to) continue;
    if (wantsRequest(findAsset(latest, scene.position, kind))) {
      latest = await api.requestAsset(storyId, scene.position, kind);
    }
  }
  return latest;
}
