import { describe, expect, test } from "vitest";
import type { MediaInfo, Scene, SceneAsset } from "./api";
import { rangeBounds, rangeCost } from "./rangeRun";

const scenes: Scene[] = [0, 1, 2].map((position) => ({
  id: position + 1,
  position,
  text: "abc",
  image_prompt: "p",
}));

const media = (assets: readonly Partial<SceneAsset>[]): MediaInfo =>
  ({
    assets,
    image_model: "m",
    image_models: [{ id: "m", credits: 10 }],
    speech_credits_per_char: 1,
  }) as unknown as MediaInfo;

describe("range runs", () => {
  test("covers from the scene to the end, or from the start to the scene", () => {
    expect(rangeBounds(scenes, "from", 1)).toEqual({ from: 1, to: 2 });
    expect(rangeBounds(scenes, "till", 1)).toEqual({ from: 0, to: 1 });
  });

  test("skips ready and in-progress assets", () => {
    const assets = [
      { position: 0, kind: "image", status: "ready", stale: false },
      { position: 1, kind: "image", status: "requested", in_progress: true },
    ] as const;

    expect(rangeCost(media(assets), scenes, "image", 0, 2)).toEqual({ count: 1, credits: 10, exact: false });
  });
});
