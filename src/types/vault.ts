export type SortBy =
  | "createdDesc"
  | "createdAsc"
  | "ratingDesc"
  | "sizeDesc"
  | "titleAsc";

export type EntryKind = "image" | "group";

export interface EntryRef {
  kind: EntryKind;
  id: string;
}

export function entryKey(ref: EntryRef): string {
  return `${ref.kind}:${ref.id}`;
}

export function parseEntryKey(key: string): EntryRef {
  const idx = key.indexOf(":");
  const kind = key.slice(0, idx) as EntryKind;
  const id = key.slice(idx + 1);
  return { kind, id };
}

export interface LockSettings {
  shortcut: string;
  idleTimeoutMinutes: number;
}

export interface UrlImportSettings {
  enabled: boolean;
  ytdlpPath: string;
  ffmpegPath: string;
  cookieBrowser: string;
  cookiesFile: string;
  maxVideoSizeMb: number;
  maxVideoHeight: number;
}

export interface YtdlpStatus {
  available: boolean;
  resolvedPath: string | null;
  version: string | null;
  error: string | null;
  ffmpegAvailable: boolean;
  ffmpegPath: string | null;
  ffmpegVersion: string | null;
}

export interface UrlImportProgressEvent {
  stage: "meta" | "downloading" | "muxing" | "saving" | string;
  percent: number | null;
  speed: string;
  eta: string;
  downloadedBytes: number;
  totalBytes: number | null;
  message: string;
  itemIndex?: number | null;
  itemTotal?: number | null;
}

export const COOKIE_BROWSER_OPTIONS = [
  { value: "", label: "使わない（未ログインで取得）" },
  { value: "arc", label: "Arc Browser (macOS)" },
  { value: "chrome", label: "Google Chrome" },
  { value: "safari", label: "Safari" },
  { value: "firefox", label: "Firefox" },
  { value: "brave", label: "Brave" },
  { value: "edge", label: "Microsoft Edge" },
  { value: "vivaldi", label: "Vivaldi" },
  { value: "opera", label: "Opera" },
  { value: "chromium", label: "Chromium" },
  { value: "file", label: "cookies.txt ファイルを指定..." },
] as const;

export function formatCookieModeLabel(settings: UrlImportSettings): string {
  if (!settings.cookieBrowser) return "オフ";
  if (settings.cookieBrowser === "file") {
    const file = settings.cookiesFile.trim();
    if (!file) return "cookies.txt（未選択）";
    const name = file.split(/[\\/]/).pop() || file;
    return `cookies.txt (${name})`;
  }
  const found = COOKIE_BROWSER_OPTIONS.find((o) => o.value === settings.cookieBrowser);
  return found ? found.label : settings.cookieBrowser;
}

export function isVideoMime(mimeType: string): boolean {
  return mimeType.startsWith("video/");
}

export function formatDurationMs(durationMs: number): string {
  if (!Number.isFinite(durationMs) || durationMs <= 0) return "";
  const totalSec = Math.max(1, Math.round(durationMs / 1000));
  const hours = Math.floor(totalSec / 3600);
  const mins = Math.floor((totalSec % 3600) / 60);
  const secs = totalSec % 60;
  if (hours > 0) {
    return `${hours}:${String(mins).padStart(2, "0")}:${String(secs).padStart(2, "0")}`;
  }
  return `${mins}:${String(secs).padStart(2, "0")}`;
}

export interface VaultStatus {
  isOpen: boolean;
  canUnlock: boolean;
  itemCount: number;
  uncategorizedCount: number;
  untaggedCount: number;
  trashCount: number;
  totalBytes: number;
  clipboardWatchEnabled: boolean;
  skipPlainExportConfirm: boolean;
  lockSettings: LockSettings;
  urlImportSettings: UrlImportSettings;
}

export interface FolderNode {
  id: string;
  parentId: string | null;
  name: string;
  color: string;
  sortOrder: number;
  itemCount: number;
}

export interface GalleryEntry {
  kind: EntryKind;
  id: string;
  coverItemId: string;
  coverMimeType: string;
  coverHasThumbnail: boolean;
  pageCount: number;
  title: string;
  width: number;
  height: number;
  byteSize: number;
  durationMs: number;
  sourceUrl: string;
  rating: number;
  memoPreview: string;
  tags: string[];
  folderIds: string[];
  palette: string[];
  createdAt: number;
  deletedAt: number | null;
}

export interface ItemDetail {
  id: string;
  title: string;
  sha256: string;
  mimeType: string;
  width: number;
  height: number;
  byteSize: number;
  durationMs: number;
  sourceUrl: string;
  rating: number;
  memo: string;
  tags: string[];
  folderIds: string[];
  palette: string[];
  hasThumbnail: boolean;
  createdAt: number;
  updatedAt: number;
  deletedAt: number | null;
}

export interface GroupPage {
  itemId: string;
  pageOrder: number;
  title: string;
  mimeType: string;
  width: number;
  height: number;
  byteSize: number;
  hasThumbnail: boolean;
  createdAt: number;
}

export interface GroupDetail {
  id: string;
  title: string;
  rating: number;
  memo: string;
  readingDir: "rtl" | "ltr";
  spreadMode: "single" | "spread";
  coverAlone: boolean;
  tags: string[];
  folderIds: string[];
  palette: string[];
  totalBytes: number;
  pageCount: number;
  pages: GroupPage[];
  createdAt: number;
  updatedAt: number;
  deletedAt: number | null;
}

export interface TagWithCount {
  id: number;
  name: string;
  count: number;
}

export interface BatchTagReport {
  targetCount: number;
  updatedCount: number;
}

export interface BatchRatingReport {
  targetCount: number;
  updatedCount: number;
}

export type QueryScope =
  | { kind: "all" }
  | { kind: "uncategorized" }
  | { kind: "untagged" }
  | { kind: "trash" }
  | { kind: "folder"; folderId: string };

export interface SearchQuery {
  scope: QueryScope;
  tagNames: string[];
  minRating: number | null;
  mimeTypes: string[];
  text: string;
  sortBy: SortBy;
}

export interface ImportReport {
  importedIds: string[];
  avifPendingIds: string[];
  videoPendingIds: string[];
  duplicateCount: number;
  skippedUnsupportedCount: number;
  errors: string[];
}

export interface ImportGroupReport {
  report: ImportReport;
  group: GroupDetail | null;
}

export interface ClipboardImportEvent {
  importedId: string | null;
  avifPending: boolean;
  duplicate: boolean;
}

export interface VaultCommandError {
  code: string;
  message: string;
}

export type ViewSelection =
  | QueryScope
  | { kind: "tags"; tags: string[] };

export type ThumbFitMode = "ratio" | "square";

export interface ToastItem {
  id: number;
  tone: "info" | "success" | "warning" | "error";
  message: string;
}

export interface VaultHealthInfo {
  fileSizeBytes: number;
  pageCount: number;
  freelistCount: number;
  pageSize: number;
  reclaimableBytes: number;
  integrityOk: boolean;
  integrityDetail: string;
  journalMode: string;
}

export interface OptimizeReport {
  sizeBeforeBytes: number;
  sizeAfterBytes: number;
  reclaimedBytes: number;
  integrityOk: boolean;
}

export interface BackupReport {
  bytesWritten: number;
}

export interface PackageExportRequest {
  destPath: string;
  passphrase: string;
  overwrite: boolean;
  folderIds: string[];
  includeSubfolders: boolean;
  tagNames: string[];
  entries: EntryRef[];
}

export interface PackageExportReport {
  itemCount: number;
  groupCount: number;
  pageCount: number;
  tagCount: number;
  folderCount: number;
  fileSizeBytes: number;
}

export interface PackageImportRequest {
  sourcePath: string;
  passphrase: string;
  targetFolderId: string | null;
}

export interface PackageImportReport {
  importedItems: number;
  mergedDuplicateItems: number;
  importedGroups: number;
  skippedDuplicateGroups: number;
  importedFolders: number;
  mergedTagLinks: number;
  avifPendingIds: string[];
  videoPendingIds: string[];
}

export type LanAccessMode = "readOnly" | "readWrite";

export interface LanInterfaceInfo {
  name: string;
  ip: string;
  isPrimary: boolean;
  isTailscale?: boolean;
}

export interface LanClientInfo {
  id: string;
  ip: string;
  deviceLabel: string;
  connectedAtMs: number;
  lastSeenAtMs: number;
}

export interface StartLanShareRequest {
  mode: LanAccessMode;
  durationMinutes: number;
  bindIp?: string | null;
  pauseIdleLock: boolean;
}

export interface LanShareStatus {
  running: boolean;
  mode: LanAccessMode;
  url: string;
  qrUrl: string;
  qrSvg: string;
  pin: string;
  port: number;
  selectedIp: string;
  interfaces: LanInterfaceInfo[];
  startedAtMs: number | null;
  expiresAtMs: number | null;
  pauseIdleLock: boolean;
  clients: LanClientInfo[];
}

export interface LanAuthStatus {
  authenticated: boolean;
  mode: LanAccessMode;
  expiresAtMs: number | null;
  mediaToken: string | null;
  vaultOpen: boolean;
}

export interface PlainExportItemRequest {
  itemId: string;
  destPath: string;
  overwrite: boolean;
}

export interface PlainExportBatchRequest {
  destDir: string;
  entries: EntryRef[];
  folderIds: string[];
  includeSubfolders: boolean;
}

export interface PlainExportReport {
  fileCount: number;
  groupCount: number;
  folderCount: number;
  totalBytes: number;
  destPath: string;
}

export interface WebBatchExportTicketResponse {
  ticket: string;
  fileCount: number;
  totalBytes: number;
  filename: string;
}

export type PlainExportTarget =
  | { kind: "item"; itemId: string; title: string; mimeType: string }
  | { kind: "group"; groupId: string; title: string; pageCount?: number }
  | { kind: "entries"; entries: EntryRef[]; imageCount: number; groupCount: number }
  | { kind: "folder"; folderId: string; folderName: string };
