import { isVideoMime, type MediaFilter } from "../types/vault";

export interface MediaFilterOption {
  value: MediaFilter;
  label: string;
}

export const MEDIA_FILTER_OPTIONS: readonly MediaFilterOption[] = [
  { value: "all", label: "すべて" },
  { value: "image", label: "画像" },
  { value: "video", label: "動画" },
  { value: "manga", label: "漫画" },
] as const;

export interface MimeFilterOption {
  label: string;
  mime: string;
  category: "image" | "video";
}

export const MIME_FILTERS: readonly MimeFilterOption[] = [
  { label: "PNG", mime: "image/png", category: "image" },
  { label: "JPEG", mime: "image/jpeg", category: "image" },
  { label: "WebP", mime: "image/webp", category: "image" },
  { label: "GIF", mime: "image/gif", category: "image" },
  { label: "AVIF", mime: "image/avif", category: "image" },
  { label: "MP4", mime: "video/mp4", category: "video" },
  { label: "WebM", mime: "video/webm", category: "video" },
  { label: "MOV", mime: "video/quicktime", category: "video" },
] as const;

export function isMimeCompatibleWithMediaFilter(mime: string, mediaType: MediaFilter): boolean {
  if (mediaType === "all") return true;
  if (mediaType === "video") return isVideoMime(mime);
  // "image" と "manga" はどちらも画像MIMEのみ対象
  return !isVideoMime(mime);
}

export function pruneMimeTypesForMediaFilter(
  mimeTypes: readonly string[],
  mediaType: MediaFilter
): string[] {
  if (mediaType === "all" || mimeTypes.length === 0) {
    return [...mimeTypes];
  }
  return mimeTypes.filter((mime) => isMimeCompatibleWithMediaFilter(mime, mediaType));
}

export function getVisibleMimeFilters(mediaType: MediaFilter): readonly MimeFilterOption[] {
  if (mediaType === "all") return MIME_FILTERS;
  const targetCategory = mediaType === "video" ? "video" : "image";
  return MIME_FILTERS.filter((item) => item.category === targetCategory);
}
