import { describe, expect, it } from "vitest";
import type { YoutubeChannel, YoutubeForm } from "./api";
import {
  addTags,
  blockers,
  fromLocalInput,
  selectedChannel,
  toLocalInput,
  visibilityText,
} from "./youtubeForm";

const form: YoutubeForm = {
  title: "Fox",
  description: "",
  tags: [],
  visibility: "private",
  publish_at: null,
  made_for_kids: true,
  synthetic_media: false,
  subtitles: true,
  channel_id: null,
};
const channel = (id: string, connected = true): YoutubeChannel => ({
  id,
  title: id,
  thumbnail: null,
  connected,
});
const ready = { channel: channel("a"), hasChannels: true, hasVideo: true, alreadyUploaded: false, confirmedAgain: false, now: 1000 };

describe("addTags", () => {
  it("splits on commas, trims and skips empty and repeated tags", () => {
    expect(addTags(["fox"], " moon, fox ,, bed time ")).toEqual(["fox", "moon", "bed time"]);
  });
});

describe("local date input", () => {
  it("round-trips a time to the minute", () => {
    const unix = fromLocalInput("2026-12-01T18:30");
    expect(unix).not.toBeNull();
    expect(toLocalInput(unix)).toBe("2026-12-01T18:30");
  });
  it("is empty for no value", () => {
    expect(fromLocalInput("")).toBeNull();
    expect(toLocalInput(null)).toBe("");
  });
});

describe("blockers", () => {
  it("is empty when everything is filled in", () => {
    expect(blockers(form, ready)).toEqual([]);
  });
  it("names the missing answers, account and video", () => {
    const empty = { ...form, title: " ", made_for_kids: null, synthetic_media: null };
    expect(blockers(empty, { ...ready, channel: null, hasVideo: false })).toHaveLength(5);
  });
  it("says what is wrong with the channel", () => {
    expect(blockers(form, { ...ready, channel: null, hasChannels: false })).toEqual([
      "подключите канал в настройках",
    ]);
    expect(blockers(form, { ...ready, channel: null })).toEqual(["выберите канал"]);
    expect(blockers(form, { ...ready, channel: channel("a", false) })).toEqual([
      "переподключите канал в настройках",
    ]);
  });
  it("needs a future time when scheduled", () => {
    const scheduled = { ...form, visibility: "scheduled" as const, publish_at: 1000 };
    expect(blockers(scheduled, ready)).toHaveLength(1);
    expect(blockers({ ...scheduled, publish_at: 2000 }, ready)).toEqual([]);
  });
  it("needs a confirmation for a second upload", () => {
    expect(blockers(form, { ...ready, alreadyUploaded: true })).toHaveLength(1);
    expect(blockers(form, { ...ready, alreadyUploaded: true, confirmedAgain: true })).toEqual([]);
  });
});

describe("selectedChannel", () => {
  it("keeps the channel saved with the story", () => {
    expect(selectedChannel([channel("a"), channel("b")], "b")?.id).toBe("b");
    expect(selectedChannel([channel("a"), channel("b", false)], "b")?.id).toBe("b");
  });
  it("preselects the only connected channel", () => {
    expect(selectedChannel([channel("a"), channel("b", false)], null)?.id).toBe("a");
  });
  it("does not replace a disconnected channel with another one", () => {
    expect(selectedChannel([channel("a")], "gone")).toBeNull();
  });
  it("does not guess between several", () => {
    expect(selectedChannel([channel("a"), channel("b")], null)).toBeNull();
    expect(selectedChannel([], null)).toBeNull();
  });
});

describe("visibilityText", () => {
  it("reports what YouTube returned, not what was asked", () => {
    expect(visibilityText("private", null)).toBe("приватное (видно только вам)");
    expect(visibilityText("public", null)).toBe("опубликовано, видно всем");
    expect(visibilityText("private", "2026-12-01T10:00:00Z")).toMatch(/^запланировано на /);
    expect(visibilityText(null, null)).toBe("YouTube не сообщил доступ");
  });
});
