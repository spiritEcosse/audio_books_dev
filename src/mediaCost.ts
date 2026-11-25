import type { AssetKind, MediaInfo, Scene, SceneAsset } from "./api";

export function formatCredits(credits: number): string {
  return Math.round(credits).toLocaleString("ru-RU");
}

/** A media action that is running: which button started it and what it is doing. */
export interface MediaBusy {
  readonly key: string;
  readonly label: string;
}

export type MediaChange = (action: () => Promise<MediaInfo>, busy?: MediaBusy) => void;

export function findAsset(media: MediaInfo, position: number, kind: AssetKind): SceneAsset | undefined {
  return media.assets.find((asset) => asset.position === position && asset.kind === kind);
}

/** Estimated credits for generating one asset of a scene with the currently selected models. */
export function sceneCredits(media: MediaInfo, scene: Scene, kind: AssetKind): number {
  if (kind === "image") {
    return media.image_models.find((model) => model.id === media.image_model)?.credits ?? 0;
  }
  return [...scene.text].length * media.speech_credits_per_char;
}

/** An asset needs generating unless it is already queued or ready and up to date. */
export function needsGeneration(asset: SceneAsset | undefined): boolean {
  return !asset || asset.status === "failed" || (asset.status === "ready" && asset.stale);
}

export interface Tally {
  readonly items: number;
  readonly credits: number;
}

export interface MediaSummary {
  readonly missing: Tally;
  readonly queued: Tally;
  /** Every queued item already has an exact estimate from ElevenLabs. */
  readonly queuedExact: boolean;
}

const KINDS: readonly AssetKind[] = ["image", "audio"];

export function summarize(media: MediaInfo, scenes: readonly Scene[]): MediaSummary {
  const add = (tally: Tally, credits: number): Tally => ({
    items: tally.items + 1,
    credits: tally.credits + credits,
  });
  return scenes.reduce<MediaSummary>(
    (summary, scene) =>
      KINDS.reduce<MediaSummary>((acc, kind) => {
        const asset = findAsset(media, scene.position, kind);
        const credits = sceneCredits(media, scene, kind);
        if (asset?.status === "requested") {
          return {
            ...acc,
            queued: add(acc.queued, asset.estimate_credits ?? credits),
            queuedExact: acc.queuedExact && asset.estimate_credits !== null,
          };
        }
        if (needsGeneration(asset)) return { ...acc, missing: add(acc.missing, credits) };
        return acc;
      }, summary),
    { missing: { items: 0, credits: 0 }, queued: { items: 0, credits: 0 }, queuedExact: true },
  );
}
