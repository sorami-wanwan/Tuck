import { describe, expect, it } from "vitest";
import {
  getVisibleMimeFilters,
  isMimeCompatibleWithMediaFilter,
  MEDIA_FILTER_OPTIONS,
  MIME_FILTERS,
  pruneMimeTypesForMediaFilter,
} from "./mediaFilter";

describe("mediaFilter", () => {
  it("defines expected media filter options", () => {
    expect(MEDIA_FILTER_OPTIONS.map((o) => o.value)).toEqual(["all", "image", "video", "manga"]);
  });

  it("checks MIME compatibility per media filter", () => {
    expect(isMimeCompatibleWithMediaFilter("image/png", "all")).toBe(true);
    expect(isMimeCompatibleWithMediaFilter("video/mp4", "all")).toBe(true);
    expect(isMimeCompatibleWithMediaFilter("image/png", "image")).toBe(true);
    expect(isMimeCompatibleWithMediaFilter("video/mp4", "image")).toBe(false);
    expect(isMimeCompatibleWithMediaFilter("image/webp", "manga")).toBe(true);
    expect(isMimeCompatibleWithMediaFilter("video/webm", "manga")).toBe(false);
    expect(isMimeCompatibleWithMediaFilter("video/mp4", "video")).toBe(true);
    expect(isMimeCompatibleWithMediaFilter("image/jpeg", "video")).toBe(false);
  });

  it("prunes incompatible mime types and filters visible mime chips", () => {
    const mixed = ["image/png", "video/mp4", "image/webp"];
    expect(pruneMimeTypesForMediaFilter(mixed, "all")).toEqual(mixed);
    expect(pruneMimeTypesForMediaFilter(mixed, "video")).toEqual(["video/mp4"]);
    expect(pruneMimeTypesForMediaFilter(mixed, "manga")).toEqual(["image/png", "image/webp"]);

    expect(getVisibleMimeFilters("all")).toHaveLength(MIME_FILTERS.length);
    expect(getVisibleMimeFilters("video").every((m) => m.category === "video")).toBe(true);
    expect(getVisibleMimeFilters("image").every((m) => m.category === "image")).toBe(true);
    expect(getVisibleMimeFilters("manga").every((m) => m.category === "image")).toBe(true);
  });
});
