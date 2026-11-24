import { describe, expect, it } from "vitest";
import type { GalleryEntry } from "../types/vault";
import { formatSelectionRatingStatus, summarizeSelectionRating } from "./bulkRating";

function makeEntry(id: string, rating: number): GalleryEntry {
  return {
    kind: "image",
    id,
    coverItemId: id,
    coverMimeType: "image/jpeg",
    coverHasThumbnail: true,
    pageCount: 1,
    title: id,
    width: 100,
    height: 100,
    byteSize: 512,
    durationMs: 0,
    sourceUrl: "",
    rating,
    memoPreview: "",
    tags: [],
    folderIds: [],
    palette: [],
    createdAt: 1000,
    deletedAt: null,
  };
}

describe("summarizeSelectionRating", () => {
  it("returns zeroed summary for empty selection", () => {
    expect(summarizeSelectionRating([])).toEqual({
      total: 0,
      uniformRating: 0,
      minRating: 0,
      maxRating: 0,
      ratedCount: 0,
    });
  });

  it("summarizes uniform rated and unrated selections", () => {
    expect(summarizeSelectionRating([makeEntry("1", 4), makeEntry("2", 4)])).toEqual({
      total: 2,
      uniformRating: 4,
      minRating: 4,
      maxRating: 4,
      ratedCount: 2,
    });
    expect(summarizeSelectionRating([makeEntry("1", 0), makeEntry("2", 0)])).toEqual({
      total: 2,
      uniformRating: 0,
      minRating: 0,
      maxRating: 0,
      ratedCount: 0,
    });
  });

  it("summarizes mixed ratings and clamps invalid numbers", () => {
    const summary = summarizeSelectionRating([
      makeEntry("1", -2),
      makeEntry("2", 3.8),
      makeEntry("3", 9),
      makeEntry("4", Number.NaN),
    ]);
    expect(summary).toEqual({
      total: 4,
      uniformRating: null,
      minRating: 0,
      maxRating: 5,
      ratedCount: 2,
    });
  });
});

describe("formatSelectionRatingStatus", () => {
  it("formats empty, uniform, and mixed rating summaries", () => {
    expect(formatSelectionRatingStatus(summarizeSelectionRating([]))).toBe("");
    expect(formatSelectionRatingStatus(summarizeSelectionRating([makeEntry("1", 0)]))).toBe("未評価");
    expect(formatSelectionRatingStatus(summarizeSelectionRating([makeEntry("1", 3)]))).toBe("★3");
    expect(
      formatSelectionRatingStatus(
        summarizeSelectionRating([makeEntry("1", 0), makeEntry("2", 5)])
      )
    ).toBe("混在 (未評価〜★5)");
    expect(
      formatSelectionRatingStatus(
        summarizeSelectionRating([makeEntry("1", 2), makeEntry("2", 4)])
      )
    ).toBe("混在 (★2〜★4)");
  });
});
