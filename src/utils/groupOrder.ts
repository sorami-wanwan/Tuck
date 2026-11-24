import { isVideoMime, type GalleryEntry, type GroupPage } from "../types/vault";

const naturalCollator = new Intl.Collator("ja", {
  numeric: true,
  sensitivity: "base",
});

export function compareNamesNatural(a: string, b: string): number {
  return naturalCollator.compare(a, b);
}

/**
 * Filter standalone non-video images and sort them oldest-first (createdAt ascending),
 * breaking ties deterministically by natural title order then ID.
 */
export function sortImagesOldestFirst(entries: readonly GalleryEntry[]): GalleryEntry[] {
  return entries
    .filter((e) => e.kind === "image" && !isVideoMime(e.coverMimeType))
    .sort((a, b) => {
      if (a.createdAt !== b.createdAt) {
        return a.createdAt - b.createdAt;
      }
      const titleCmp = naturalCollator.compare(a.title, b.title);
      if (titleCmp !== 0) {
        return titleCmp;
      }
      return a.id.localeCompare(b.id);
    });
}

export type GroupPageSortMode = "createdAsc" | "createdDesc" | "titleAsc" | "reverse";

/**
 * Return a newly sorted copy of group pages for bulk reorder actions.
 */
export function sortGroupPages(
  pages: readonly GroupPage[],
  mode: GroupPageSortMode
): GroupPage[] {
  if (mode === "reverse") {
    return [...pages].reverse();
  }
  return [...pages].sort((a, b) => {
    if (mode === "createdAsc") {
      if (a.createdAt !== b.createdAt) return a.createdAt - b.createdAt;
      const titleCmp = naturalCollator.compare(a.title, b.title);
      if (titleCmp !== 0) return titleCmp;
      return a.itemId.localeCompare(b.itemId);
    }
    if (mode === "createdDesc") {
      if (a.createdAt !== b.createdAt) return b.createdAt - a.createdAt;
      const titleCmp = naturalCollator.compare(a.title, b.title);
      if (titleCmp !== 0) return titleCmp;
      return a.itemId.localeCompare(b.itemId);
    }
    const titleCmp = naturalCollator.compare(a.title, b.title);
    if (titleCmp !== 0) return titleCmp;
    if (a.createdAt !== b.createdAt) return a.createdAt - b.createdAt;
    return a.itemId.localeCompare(b.itemId);
  });
}

/**
 * Move an element from `fromIndex` to `targetIndex` without mutating the source array.
 */
export function applyPageMove<T>(
  items: readonly T[],
  fromIndex: number,
  targetIndex: number
): T[] {
  const next = [...items];
  if (
    fromIndex < 0 ||
    fromIndex >= next.length ||
    targetIndex < 0 ||
    targetIndex >= next.length ||
    fromIndex === targetIndex
  ) {
    return next;
  }
  const [moved] = next.splice(fromIndex, 1);
  next.splice(targetIndex, 0, moved);
  return next;
}

export interface CardRect {
  index: number;
  left: number;
  right: number;
  top: number;
  bottom: number;
}

export interface GridDropTarget {
  insertIndex: number;
  targetIndex: number;
  anchorIndex: number;
  edge: "left" | "right";
  isNoOp: boolean;
}

interface VisualRow {
  top: number;
  bottom: number;
  cards: CardRect[];
}

/**
 * Compute insertion slot and visual anchor inside a 2D CSS grid from pointer coordinates.
 */
export function computeGridDropTarget(
  cards: readonly CardRect[],
  fromIndex: number,
  clientX: number,
  clientY: number
): GridDropTarget | null {
  if (cards.length === 0 || fromIndex < 0 || fromIndex >= cards.length) {
    return null;
  }

  const ordered = [...cards].sort((a, b) => a.index - b.index);
  const rows: VisualRow[] = [];

  for (const card of ordered) {
    const lastRow = rows[rows.length - 1];
    if (!lastRow) {
      rows.push({ top: card.top, bottom: card.bottom, cards: [card] });
      continue;
    }
    const rowHeight = Math.max(0, lastRow.bottom - lastRow.top);
    const sameRowThreshold = Math.max(12, rowHeight * 0.5);
    if (Math.abs(card.top - lastRow.top) < sameRowThreshold) {
      lastRow.top = Math.min(lastRow.top, card.top);
      lastRow.bottom = Math.max(lastRow.bottom, card.bottom);
      lastRow.cards.push(card);
    } else {
      rows.push({ top: card.top, bottom: card.bottom, cards: [card] });
    }
  }

  let targetRow = rows[0];
  if (clientY < rows[0].top) {
    targetRow = rows[0];
  } else if (clientY > rows[rows.length - 1].bottom) {
    targetRow = rows[rows.length - 1];
  } else {
    const containing = rows.find((r) => clientY >= r.top && clientY <= r.bottom);
    if (containing) {
      targetRow = containing;
    } else {
      let bestDist = Infinity;
      for (const row of rows) {
        const centerY = (row.top + row.bottom) / 2;
        const dist = Math.abs(clientY - centerY);
        if (dist < bestDist) {
          bestDist = dist;
          targetRow = row;
        }
      }
    }
  }

  const rowCards = [...targetRow.cards].sort((a, b) => a.left - b.left);
  let insertIndex = rowCards[rowCards.length - 1].index + 1;
  let anchorIndex = rowCards[rowCards.length - 1].index;
  let edge: "left" | "right" = "right";

  for (const card of rowCards) {
    const midX = (card.left + card.right) / 2;
    if (clientX < midX) {
      insertIndex = card.index;
      anchorIndex = card.index;
      edge = "left";
      break;
    }
  }

  const isNoOp = insertIndex === fromIndex || insertIndex === fromIndex + 1;
  const targetIndex = insertIndex > fromIndex ? insertIndex - 1 : insertIndex;

  return {
    insertIndex,
    targetIndex,
    anchorIndex,
    edge,
    isNoOp,
  };
}
