import type { GalleryEntry } from "../types/vault";

export interface SelectionRatingSummary {
  total: number;
  uniformRating: number | null;
  minRating: number;
  maxRating: number;
  ratedCount: number;
}

function clampRating(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(5, Math.trunc(value)));
}

export function summarizeSelectionRating(entries: GalleryEntry[]): SelectionRatingSummary {
  const total = entries.length;
  if (total === 0) {
    return { total: 0, uniformRating: 0, minRating: 0, maxRating: 0, ratedCount: 0 };
  }

  let minRating = 5;
  let maxRating = 0;
  let ratedCount = 0;
  const first = clampRating(entries[0].rating);
  let allSame = true;

  for (const entry of entries) {
    const r = clampRating(entry.rating);
    if (r !== first) allSame = false;
    if (r < minRating) minRating = r;
    if (r > maxRating) maxRating = r;
    if (r > 0) ratedCount += 1;
  }

  return {
    total,
    uniformRating: allSame ? first : null,
    minRating,
    maxRating,
    ratedCount,
  };
}

export function formatSelectionRatingStatus(summary: SelectionRatingSummary): string {
  if (summary.total === 0) return "";
  if (summary.uniformRating === 0) return "未評価";
  if (summary.uniformRating !== null) return `★${summary.uniformRating}`;
  const minLabel = summary.minRating === 0 ? "未評価" : `★${summary.minRating}`;
  return `混在 (${minLabel}〜★${summary.maxRating})`;
}
