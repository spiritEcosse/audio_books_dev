import { describe, expect, test } from "vitest";
import type { MediaInfo, Scene, SceneAsset } from "./api";
import { needsGeneration, rowLocked, sceneCredits, summarize } from "./mediaCost";

const scene: Scene = { id: 1, position: 0, text: "Жили-были", image_prompt: "a hut" };

const media = (assets: readonly Partial<SceneAsset>[]): MediaInfo =>
  ({
    assets,
    image_model: "m",
    image_models: [{ id: "m", credits: 100 }],
    speech_credits_per_char: 2,
  }) as unknown as MediaInfo;

describe("media cost", () => {
  test("prices an image by model and narration by character", () => {
    expect(sceneCredits(media([]), scene, "image")).toBe(100);
    expect(sceneCredits(media([]), scene, "audio")).toBe(18);
  });

  test("asks to generate only missing, failed or outdated assets", () => {
    expect(needsGeneration(undefined)).toBe(true);
    expect(needsGeneration({ status: "failed" } as SceneAsset)).toBe(true);
    expect(needsGeneration({ status: "ready", stale: true } as SceneAsset)).toBe(true);
    expect(needsGeneration({ status: "ready", stale: false } as SceneAsset)).toBe(false);
  });

  test("separates queued items from missing ones and prefers the exact estimate", () => {
    const queued = { position: 0, kind: "image", status: "requested", estimate_credits: 75 } as const;

    const summary = summarize(media([queued]), [scene]);

    expect(summary.queued).toEqual({ items: 1, credits: 75 });
    expect(summary.missing).toEqual({ items: 1, credits: 18 });
    expect(summary.queuedExact).toBe(true);
  });

  test("locks a row for its own or a global operation only", () => {
    expect(rowLocked([{ key: "image:1", label: "" }], "image:0")).toBe(false);
    expect(rowLocked([{ key: "image:0", label: "" }], "image:0")).toBe(true);
    expect(rowLocked([{ key: "export", label: "" }], "image:0")).toBe(true);
  });
});
