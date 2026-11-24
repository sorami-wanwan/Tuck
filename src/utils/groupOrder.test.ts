import { describe, expect, it } from "vitest";
import type { GalleryEntry, GroupPage } from "../types/vault";
import {
  applyPageMove,
  compareNamesNatural,
  computeGridDropTarget,
  sortGroupPages,
  sortImagesOldestFirst,
} from "./groupOrder";

function makeEntry(
  id: string,
  overrides: Partial<GalleryEntry> = {}
): GalleryEntry {
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
    byteSize: 100,
    durationMs: 0,
    sourceUrl: "",
    rating: 0,
    memoPreview: "",
    tags: [],
    folderIds: [],
    palette: [],
    createdAt: 1000,
    deletedAt: null,
    ...overrides,
  };
}

function makePage(itemId: string, title: string, createdAt: number, pageOrder: number): GroupPage {
  return {
    itemId,
    pageOrder,
    title,
    mimeType: "image/png",
    width: 100,
    height: 100,
    byteSize: 100,
    hasThumbnail: true,
    createdAt,
  };
}

describe("compareNamesNatural", () => {
  it("orders numbered names naturally", () => {
    expect(compareNamesNatural("page2", "page10")).toBeLessThan(0);
  });
});

describe("sortImagesOldestFirst", () => {
  it("excludes groups and videos and sorts by createdAt, natural title, then id", () => {
    const entries = [
      makeEntry("vid", { coverMimeType: "video/mp4", createdAt: 100 }),
      makeEntry("grp", { kind: "group", createdAt: 100 }),
      makeEntry("b", { title: "page10", createdAt: 200 }),
      makeEntry("a", { title: "page2", createdAt: 200 }),
      makeEntry("early", { title: "z", createdAt: 150 }),
    ];

    expect(sortImagesOldestFirst(entries).map((e) => e.id)).toEqual(["early", "a", "b"]);
  });
});

describe("sortGroupPages", () => {
  const pages = [
    makePage("p1", "page10", 200, 0),
    makePage("p2", "page2", 200, 1),
    makePage("p3", "cover", 100, 2),
  ];

  it("sorts without mutating source array", () => {
    expect(sortGroupPages(pages, "createdAsc").map((p) => p.itemId)).toEqual(["p3", "p2", "p1"]);
    expect(sortGroupPages(pages, "createdDesc").map((p) => p.itemId)).toEqual(["p2", "p1", "p3"]);
    expect(sortGroupPages(pages, "titleAsc").map((p) => p.itemId)).toEqual(["p3", "p2", "p1"]);
    expect(sortGroupPages(pages, "reverse").map((p) => p.itemId)).toEqual(["p3", "p2", "p1"]);
    expect(pages.map((p) => p.itemId)).toEqual(["p1", "p2", "p3"]);
  });
});

describe("applyPageMove", () => {
  it("moves items forward and backward and ignores invalid/no-op indices", () => {
    const src = ["a", "b", "c", "d"];
    expect(applyPageMove(src, 0, 2)).toEqual(["b", "c", "a", "d"]);
    expect(applyPageMove(src, 3, 1)).toEqual(["a", "d", "b", "c"]);
    expect(applyPageMove(src, 1, 1)).toEqual(src);
    expect(applyPageMove(src, -1, 2)).toEqual(src);
    expect(applyPageMove(src, 0, 9)).toEqual(src);
    expect(src).toEqual(["a", "b", "c", "d"]);
  });
});

describe("computeGridDropTarget", () => {
  const cards = [
    { index: 0, left: 0, right: 100, top: 0, bottom: 100 },
    { index: 1, left: 110, right: 210, top: 0, bottom: 100 },
    { index: 2, left: 0, right: 100, top: 120, bottom: 220 },
    { index: 3, left: 110, right: 210, top: 120, bottom: 220 },
  ];

  it("returns null for empty cards or out-of-range index", () => {
    expect(computeGridDropTarget([], 0, 10, 10)).toBeNull();
    expect(computeGridDropTarget(cards, -1, 10, 10)).toBeNull();
    expect(computeGridDropTarget(cards, 4, 10, 10)).toBeNull();
  });

  it("marks adjacent slots around dragged card as no-op and computes cross-row targetIndex", () => {
    const beforeSelf = computeGridDropTarget(cards, 1, 120, 50);
    expect(beforeSelf?.isNoOp).toBe(true);

    const dropBefore2 = computeGridDropTarget(cards, 0, 20, 150);
    expect(dropBefore2).toEqual({
      insertIndex: 2,
      targetIndex: 1,
      anchorIndex: 2,
      edge: "left",
      isNoOp: false,
    });

    const dropEndOfRow2 = computeGridDropTarget(cards, 0, 200, 150);
    expect(dropEndOfRow2).toEqual({
      insertIndex: 4,
      targetIndex: 3,
      anchorIndex: 3,
      edge: "right",
      isNoOp: false,
    });
  });
});
