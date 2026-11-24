import type { GalleryEntry } from "../types/vault";

export interface SelectionTagSummary {
  name: string;
  key: string;
  count: number;
  total: number;
  state: "all" | "partial";
}

export function normalizeTagKey(tagName: string): string {
  return tagName.normalize("NFC").trim().toLowerCase();
}

export function summarizeSelectionTags(entries: GalleryEntry[]): SelectionTagSummary[] {
  const total = entries.length;
  if (total === 0) return [];

  const counts = new Map<string, { name: string; count: number }>();

  for (const entry of entries) {
    const seenInEntry = new Set<string>();
    for (const rawTag of entry.tags) {
      const trimmed = rawTag.normalize("NFC").trim();
      if (!trimmed) continue;
      const key = trimmed.toLowerCase();
      if (seenInEntry.has(key)) continue;
      seenInEntry.add(key);

      const existing = counts.get(key);
      if (existing) {
        existing.count += 1;
      } else {
        counts.set(key, { name: trimmed, count: 1 });
      }
    }
  }

  const summaries: SelectionTagSummary[] = [];
  for (const [key, item] of counts.entries()) {
    summaries.push({
      name: item.name,
      key,
      count: item.count,
      total,
      state: item.count === total ? "all" : "partial",
    });
  }

  summaries.sort((a, b) => {
    if (a.state !== b.state) {
      return a.state === "all" ? -1 : 1;
    }
    if (a.state === "partial" && a.count !== b.count) {
      return b.count - a.count;
    }
    return a.name.localeCompare(b.name, "ja", { sensitivity: "base" });
  });

  return summaries;
}

export function isImeComposing(e: {
  nativeEvent?: { isComposing?: boolean };
  isComposing?: boolean;
  keyCode?: number;
}): boolean {
  return Boolean((e.nativeEvent ? e.nativeEvent.isComposing : e.isComposing) || e.keyCode === 229);
}
