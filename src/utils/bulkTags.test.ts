import { describe, expect, it } from "vitest";
import type { GalleryEntry } from "../types/vault";
import { isImeComposing, normalizeTagKey, summarizeSelectionTags } from "./bulkTags";

function makeEntry(id: string, tags: string[]): GalleryEntry {
  return {
    kind: "image",
    id,
    coverItemId: id,
    coverMimeType: "image/png",
    coverHasThumbnail: true,
    pageCount: 1,
    title: id,
    width: 100,
    height: 100,
    byteSize: 1024,
    durationMs: 0,
    sourceUrl: "",
    rating: 0,
    memoPreview: "",
    tags,
    folderIds: [],
    palette: [],
    createdAt: 1000,
    deletedAt: null,
  };
}

describe("normalizeTagKey", () => {
  it("trims, lowercases, and NFC-normalizes strings", () => {
    expect(normalizeTagKey("  FooBar  ")).toBe("foobar");
    expect(normalizeTagKey("ホ\u309Aケモン")).toBe("ポケモン");
  });
});

describe("summarizeSelectionTags", () => {
  it("returns empty list when selection is empty", () => {
    expect(summarizeSelectionTags([])).toEqual([]);
  });

  it("distinguishes all vs partial tags and sorts all first then partial count desc", () => {
    const entries = [
      makeEntry("1", ["風景", "夜", "青"]),
      makeEntry("2", ["風景", "夜"]),
      makeEntry("3", ["風景", "朝"]),
    ];

    const summary = summarizeSelectionTags(entries);
    expect(summary.map((s) => ({ name: s.name, state: s.state, count: s.count }))).toEqual([
      { name: "風景", state: "all", count: 3 },
      { name: "夜", state: "partial", count: 2 },
      { name: "青", state: "partial", count: 1 },
      { name: "朝", state: "partial", count: 1 },
    ]);
  });

  it("deduplicates case and NFD variants within the same entry", () => {
    const entries = [
      makeEntry("1", ["Tag", "tag", "  \t "]),
      makeEntry("2", ["ホ\u309Aケモン", "ポケモン"]),
    ];

    const summary = summarizeSelectionTags(entries);
    expect(summary).toHaveLength(2);
    expect(summary.every((s) => s.count === 1 && s.state === "partial")).toBe(true);
  });
});

describe("isImeComposing", () => {
  it("detects composition from nativeEvent, direct isComposing, or keyCode 229", () => {
    expect(isImeComposing({ nativeEvent: { isComposing: true } })).toBe(true);
    expect(isImeComposing({ isComposing: true })).toBe(true);
    expect(isImeComposing({ keyCode: 229 })).toBe(true);
    expect(isImeComposing({ nativeEvent: { isComposing: false }, keyCode: 13 })).toBe(false);
    expect(isImeComposing({})).toBe(false);
  });
});
