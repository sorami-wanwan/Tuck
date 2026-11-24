import { invoke } from "@tauri-apps/api/core";
import type {
  BackupReport,
  BatchRatingReport,
  BatchTagReport,
  EntryRef,
  FolderNode,
  GalleryEntry,
  GroupDetail,
  ImportGroupReport,
  ImportReport,
  ItemDetail,
  LanAccessMode,
  LanAuthStatus,
  LanInterfaceInfo,
  LanShareStatus,
  LockSettings,
  OptimizeReport,
  PackageExportReport,
  PackageExportRequest,
  PackageImportReport,
  PackageImportRequest,
  PlainExportBatchRequest,
  PlainExportItemRequest,
  PlainExportReport,
  SearchQuery,
  StartLanShareRequest,
  TagWithCount,
  UrlImportSettings,
  VaultCommandError,
  VaultHealthInfo,
  VaultStatus,
  WebBatchExportTicketResponse,
  YtdlpStatus,
} from "../types/vault";
import { isTauriRuntime } from "./runtime";

export function formatErrorMessage(err: unknown): string {
  if (typeof err === "object" && err !== null && "message" in err) {
    const maybe = err as Partial<VaultCommandError>;
    if (typeof maybe.message === "string" && maybe.message.length > 0) {
      return maybe.message;
    }
  }
  if (err instanceof Error) {
    return err.message;
  }
  if (typeof err === "string") {
    return err;
  }
  return "予期しないエラーが発生しました";
}

type AuthInvalidatedHandler = (reason: "unauthorized" | "locked") => void;
let authInvalidatedHandler: AuthInvalidatedHandler | null = null;

export function setAuthInvalidatedHandler(handler: AuthInvalidatedHandler | null): void {
  authInvalidatedHandler = handler;
}

function buildWebHeaders(
  method: string,
  extra?: Record<string, string>,
  jsonContent = false
): Record<string, string> {
  const headers: Record<string, string> = { ...(extra ?? {}) };
  if (method !== "GET" && method !== "HEAD") {
    headers["X-Tuck-Client"] = "web";
  }
  if (jsonContent) {
    headers["Content-Type"] = "application/json";
  }
  return headers;
}

async function parseWebError(res: Response): Promise<VaultCommandError> {
  const ct = res.headers.get("content-type") ?? "";
  if (ct.includes("application/json")) {
    try {
      const data = (await res.json()) as Partial<VaultCommandError>;
      if (data && typeof data.message === "string") {
        return {
          code: typeof data.code === "string" ? data.code : `HTTP_${res.status}`,
          message: data.message,
        };
      }
    } catch {
      // fall through
    }
  }
  if (res.status === 401) {
    return { code: "LAN_UNAUTHORIZED", message: "認証が必要です" };
  }
  if (res.status === 403) {
    return { code: "FORBIDDEN", message: "この操作は許可されていません" };
  }
  return { code: `HTTP_${res.status}`, message: `通信エラー (${res.status})` };
}

async function webRequest(
  path: string,
  init: RequestInit = {},
  notifyOnAuthFailure = true
): Promise<Response> {
  const res = await fetch(path, {
    ...init,
    credentials: "same-origin",
    cache: "no-store",
  });
  if (!res.ok) {
    const err = await parseWebError(res);
    if (notifyOnAuthFailure && authInvalidatedHandler) {
      if (res.status === 401) {
        authInvalidatedHandler("unauthorized");
      } else if (res.status === 403 && err.code === "VAULT_NOT_OPEN") {
        authInvalidatedHandler("locked");
      }
    }
    throw err;
  }
  return res;
}

async function webJson<T>(
  path: string,
  method = "GET",
  body?: unknown,
  notifyOnAuthFailure = true
): Promise<T> {
  const hasBody = body !== undefined;
  const res = await webRequest(
    path,
    {
      method,
      headers: buildWebHeaders(method, undefined, hasBody),
      body: hasBody ? JSON.stringify(body) : undefined,
    },
    notifyOnAuthFailure
  );
  if (res.status === 204) {
    return undefined as T;
  }
  return (await res.json()) as T;
}

async function webVoid(path: string, method = "POST", body?: unknown): Promise<void> {
  const hasBody = body !== undefined;
  await webRequest(path, {
    method,
    headers: buildWebHeaders(method, undefined, hasBody),
    body: hasBody ? JSON.stringify(body) : undefined,
  });
}

async function webArrayBuffer(path: string): Promise<ArrayBuffer> {
  const res = await webRequest(path, { method: "GET" });
  return res.arrayBuffer();
}

function toFetchBody(input: Uint8Array | ArrayBuffer | Blob): BodyInit {
  if (input instanceof Blob) {
    return input;
  }
  if (input instanceof ArrayBuffer) {
    return input;
  }
  return new Uint8Array(input).buffer;
}

export const vaultApi = {
  createVault(path: string, passphrase: string, overwrite = false): Promise<VaultStatus> {
    return invoke("create_vault", { path, passphrase, overwrite });
  },

  openVault(path: string, passphrase: string): Promise<VaultStatus> {
    return invoke("open_vault", { path, passphrase });
  },

  unlockVault(passphrase: string): Promise<VaultStatus> {
    return invoke("unlock_vault", { passphrase });
  },

  lockVault(): Promise<VaultStatus> {
    return invoke("lock_vault");
  },

  closeVault(): Promise<void> {
    return invoke("close_vault");
  },

  getVaultStatus(): Promise<VaultStatus> {
    if (isTauriRuntime()) {
      return invoke("get_vault_status");
    }
    return webJson<VaultStatus>("/api/vault/status");
  },

  setLockSettings(settings: LockSettings): Promise<LockSettings> {
    return invoke("set_lock_settings", { settings });
  },

  setUrlImportSettings(settings: UrlImportSettings): Promise<UrlImportSettings> {
    return invoke("set_url_import_settings", { settings });
  },

  checkYtdlpStatus(customPath?: string, customFfmpegPath?: string): Promise<YtdlpStatus> {
    if (isTauriRuntime()) {
      return invoke("check_ytdlp_status", {
        customPath: customPath ?? null,
        customFfmpegPath: customFfmpegPath ?? null,
      });
    }
    return webJson<YtdlpStatus>("/api/import/ytdlp-status");
  },

  cancelUrlImport(): Promise<boolean> {
    if (isTauriRuntime()) {
      return invoke("cancel_url_import");
    }
    return webJson<boolean>("/api/import/url/cancel", "POST");
  },

  setClipboardWatch(enabled: boolean): Promise<VaultStatus> {
    return invoke("set_clipboard_watch", { enabled });
  },

  importPaths(paths: string[], folderId?: string | null): Promise<ImportReport> {
    return invoke("import_paths", { paths, folderId: folderId ?? null });
  },

  importFolderAsGroup(dirPath: string, folderId?: string | null): Promise<ImportGroupReport> {
    return invoke("import_folder_as_group", { dirPath, folderId: folderId ?? null });
  },

  async importBytes(
    bytes: Uint8Array | ArrayBuffer | Blob,
    title?: string,
    folderId?: string | null
  ): Promise<ImportReport> {
    const headers: Record<string, string> = {};
    if (title && title.trim().length > 0) {
      headers["x-title"] = encodeURIComponent(title.trim());
    }
    if (folderId) {
      headers["x-folder-id"] = folderId;
    }
    if (isTauriRuntime()) {
      const payload = bytes instanceof Blob ? await bytes.arrayBuffer() : bytes;
      return invoke("import_bytes", payload, { headers });
    }
    const res = await webRequest("/api/import/bytes", {
      method: "POST",
      headers: buildWebHeaders("POST", headers, false),
      body: toFetchBody(bytes),
    });
    return (await res.json()) as ImportReport;
  },

  importClipboard(folderId?: string | null): Promise<ImportReport> {
    return invoke("import_clipboard", { folderId: folderId ?? null });
  },

  importUrl(url: string, folderId?: string | null): Promise<ImportReport> {
    if (isTauriRuntime()) {
      return invoke("import_url", { url, folderId: folderId ?? null });
    }
    return webJson<ImportReport>("/api/import/url", "POST", {
      url,
      folderId: folderId ?? null,
    });
  },

  saveAvifThumbnail(
    itemId: string,
    pngBytes: ArrayBuffer | Uint8Array,
    width: number,
    height: number
  ): Promise<ItemDetail> {
    return vaultApi.saveMediaThumbnail(itemId, pngBytes, width, height, 0);
  },

  async saveMediaThumbnail(
    itemId: string,
    pngBytes: ArrayBuffer | Uint8Array,
    width: number,
    height: number,
    durationMs = 0
  ): Promise<ItemDetail> {
    if (isTauriRuntime()) {
      return invoke("save_media_thumbnail", pngBytes, {
        headers: {
          "x-item-id": itemId,
          "x-width": String(width),
          "x-height": String(height),
          "x-duration-ms": String(durationMs),
        },
      });
    }
    const res = await webRequest(`/api/items/${encodeURIComponent(itemId)}/thumbnail`, {
      method: "POST",
      headers: buildWebHeaders(
        "POST",
        {
          "x-width": String(width),
          "x-height": String(height),
          "x-duration-ms": String(durationMs),
        },
        false
      ),
      body: toFetchBody(pngBytes),
    });
    return (await res.json()) as ItemDetail;
  },

  queryEntries(query: SearchQuery): Promise<GalleryEntry[]> {
    if (isTauriRuntime()) {
      return invoke("query_entries", { query });
    }
    return webJson<GalleryEntry[]>("/api/entries/query", "POST", query);
  },

  getItemDetail(itemId: string): Promise<ItemDetail> {
    if (isTauriRuntime()) {
      return invoke("get_item_detail", { itemId });
    }
    return webJson<ItemDetail>(`/api/items/${encodeURIComponent(itemId)}`);
  },

  fetchThumbnailBytes(itemId: string): Promise<ArrayBuffer> {
    if (isTauriRuntime()) {
      return invoke("get_thumbnail", { itemId });
    }
    return webArrayBuffer(`/api/thumbnails/${encodeURIComponent(itemId)}`);
  },

  fetchImageBytes(itemId: string): Promise<ArrayBuffer> {
    if (isTauriRuntime()) {
      return invoke("get_image", { itemId });
    }
    return webArrayBuffer(`/api/images/${encodeURIComponent(itemId)}`);
  },

  setItemTitle(itemId: string, title: string): Promise<ItemDetail> {
    if (isTauriRuntime()) {
      return invoke("set_item_title", { itemId, title });
    }
    return webJson<ItemDetail>(`/api/items/${encodeURIComponent(itemId)}/title`, "PATCH", {
      title,
    });
  },

  setItemRating(itemId: string, rating: number): Promise<ItemDetail> {
    if (isTauriRuntime()) {
      return invoke("set_item_rating", { itemId, rating });
    }
    return webJson<ItemDetail>(`/api/items/${encodeURIComponent(itemId)}/rating`, "PATCH", {
      rating,
    });
  },

  setItemMemo(itemId: string, memo: string): Promise<ItemDetail> {
    if (isTauriRuntime()) {
      return invoke("set_item_memo", { itemId, memo });
    }
    return webJson<ItemDetail>(`/api/items/${encodeURIComponent(itemId)}/memo`, "PATCH", {
      memo,
    });
  },

  setItemSourceUrl(itemId: string, sourceUrl: string): Promise<ItemDetail> {
    if (isTauriRuntime()) {
      return invoke("set_item_source_url", { itemId, sourceUrl });
    }
    return webJson<ItemDetail>(`/api/items/${encodeURIComponent(itemId)}/source-url`, "PATCH", {
      sourceUrl,
    });
  },

  addItemTag(itemId: string, tagName: string): Promise<ItemDetail> {
    if (isTauriRuntime()) {
      return invoke("add_item_tag", { itemId, tagName });
    }
    return webJson<ItemDetail>(`/api/items/${encodeURIComponent(itemId)}/tags`, "POST", {
      tagName,
    });
  },

  removeItemTag(itemId: string, tagName: string): Promise<ItemDetail> {
    if (isTauriRuntime()) {
      return invoke("remove_item_tag", { itemId, tagName });
    }
    return webJson<ItemDetail>(
      `/api/items/${encodeURIComponent(itemId)}/tags/${encodeURIComponent(tagName)}`,
      "DELETE"
    );
  },

  batchUpdateEntryTags(
    entries: EntryRef[],
    addTags: string[],
    removeTags: string[] = []
  ): Promise<BatchTagReport> {
    if (isTauriRuntime()) {
      return invoke("batch_update_entry_tags", { entries, addTags, removeTags });
    }
    return webJson<BatchTagReport>("/api/entries/tags/batch", "POST", {
      entries,
      addTags,
      removeTags,
    });
  },

  batchSetEntryRating(entries: EntryRef[], rating: number): Promise<BatchRatingReport> {
    if (isTauriRuntime()) {
      return invoke("batch_set_entry_rating", { entries, rating });
    }
    return webJson<BatchRatingReport>("/api/entries/rating/batch", "POST", {
      entries,
      rating,
    });
  },

  listTags(): Promise<TagWithCount[]> {
    if (isTauriRuntime()) {
      return invoke("list_tags");
    }
    return webJson<TagWithCount[]>("/api/tags");
  },

  // Folders
  listFolders(): Promise<FolderNode[]> {
    if (isTauriRuntime()) {
      return invoke("list_folders");
    }
    return webJson<FolderNode[]>("/api/folders");
  },

  createFolder(parentId: string | null, name: string, color = ""): Promise<FolderNode> {
    if (isTauriRuntime()) {
      return invoke("create_folder", { parentId, name, color });
    }
    return webJson<FolderNode>("/api/folders", "POST", { parentId, name, color });
  },

  renameFolder(folderId: string, name: string, color?: string | null): Promise<FolderNode> {
    if (isTauriRuntime()) {
      return invoke("rename_folder", { folderId, name, color: color ?? null });
    }
    return webJson<FolderNode>(`/api/folders/${encodeURIComponent(folderId)}`, "PATCH", {
      name,
      color: color ?? null,
    });
  },

  moveFolder(folderId: string, newParentId: string | null): Promise<FolderNode> {
    if (isTauriRuntime()) {
      return invoke("move_folder", { folderId, newParentId });
    }
    return webJson<FolderNode>(`/api/folders/${encodeURIComponent(folderId)}/move`, "POST", {
      newParentId,
    });
  },

  deleteFolder(folderId: string): Promise<void> {
    if (isTauriRuntime()) {
      return invoke("delete_folder", { folderId });
    }
    return webVoid(`/api/folders/${encodeURIComponent(folderId)}`, "DELETE");
  },

  addEntriesToFolder(folderId: string, entries: EntryRef[]): Promise<void> {
    if (isTauriRuntime()) {
      return invoke("add_entries_to_folder", { folderId, entries });
    }
    return webVoid(`/api/folders/${encodeURIComponent(folderId)}/entries`, "POST", { entries });
  },

  removeEntriesFromFolder(folderId: string, entries: EntryRef[]): Promise<void> {
    if (isTauriRuntime()) {
      return invoke("remove_entries_from_folder", { folderId, entries });
    }
    return webVoid(`/api/folders/${encodeURIComponent(folderId)}/entries`, "DELETE", { entries });
  },

  // Manga Groups
  createGroupFromItems(
    itemIds: string[],
    title?: string | null,
    targetFolderId?: string | null
  ): Promise<GroupDetail> {
    if (isTauriRuntime()) {
      return invoke("create_group_from_items", {
        itemIds,
        title: title ?? null,
        targetFolderId: targetFolderId ?? null,
      });
    }
    return webJson<GroupDetail>("/api/groups", "POST", {
      itemIds,
      title: title ?? null,
      targetFolderId: targetFolderId ?? null,
    });
  },

  addItemsToGroup(groupId: string, itemIds: string[]): Promise<GroupDetail> {
    if (isTauriRuntime()) {
      return invoke("add_items_to_group", { groupId, itemIds });
    }
    return webJson<GroupDetail>(`/api/groups/${encodeURIComponent(groupId)}/items`, "POST", {
      itemIds,
    });
  },

  reorderGroupPages(groupId: string, orderedItemIds: string[]): Promise<GroupDetail> {
    if (isTauriRuntime()) {
      return invoke("reorder_group_pages", { groupId, orderedItemIds });
    }
    return webJson<GroupDetail>(
      `/api/groups/${encodeURIComponent(groupId)}/pages/order`,
      "PUT",
      { orderedItemIds }
    );
  },

  removePageFromGroup(groupId: string, itemId: string): Promise<GroupDetail | null> {
    if (isTauriRuntime()) {
      return invoke("remove_page_from_group", { groupId, itemId });
    }
    return webJson<GroupDetail | null>(
      `/api/groups/${encodeURIComponent(groupId)}/pages/${encodeURIComponent(itemId)}`,
      "DELETE"
    );
  },

  ungroup(groupId: string): Promise<string[]> {
    if (isTauriRuntime()) {
      return invoke("ungroup", { groupId });
    }
    return webJson<string[]>(`/api/groups/${encodeURIComponent(groupId)}/ungroup`, "POST");
  },

  getGroupDetail(groupId: string): Promise<GroupDetail> {
    if (isTauriRuntime()) {
      return invoke("get_group_detail", { groupId });
    }
    return webJson<GroupDetail>(`/api/groups/${encodeURIComponent(groupId)}`);
  },

  setGroupTitle(groupId: string, title: string): Promise<GroupDetail> {
    if (isTauriRuntime()) {
      return invoke("set_group_title", { groupId, title });
    }
    return webJson<GroupDetail>(`/api/groups/${encodeURIComponent(groupId)}/title`, "PATCH", {
      title,
    });
  },

  setGroupRating(groupId: string, rating: number): Promise<GroupDetail> {
    if (isTauriRuntime()) {
      return invoke("set_group_rating", { groupId, rating });
    }
    return webJson<GroupDetail>(`/api/groups/${encodeURIComponent(groupId)}/rating`, "PATCH", {
      rating,
    });
  },

  setGroupMemo(groupId: string, memo: string): Promise<GroupDetail> {
    if (isTauriRuntime()) {
      return invoke("set_group_memo", { groupId, memo });
    }
    return webJson<GroupDetail>(`/api/groups/${encodeURIComponent(groupId)}/memo`, "PATCH", {
      memo,
    });
  },

  setGroupReaderPrefs(
    groupId: string,
    readingDir: "rtl" | "ltr",
    spreadMode: "single" | "spread",
    coverAlone: boolean
  ): Promise<GroupDetail> {
    if (isTauriRuntime()) {
      return invoke("set_group_reader_prefs", {
        groupId,
        readingDir,
        spreadMode,
        coverAlone,
      });
    }
    return webJson<GroupDetail>(
      `/api/groups/${encodeURIComponent(groupId)}/reader-prefs`,
      "PATCH",
      { readingDir, spreadMode, coverAlone }
    );
  },

  addGroupTag(groupId: string, tagName: string): Promise<GroupDetail> {
    if (isTauriRuntime()) {
      return invoke("add_group_tag", { groupId, tagName });
    }
    return webJson<GroupDetail>(`/api/groups/${encodeURIComponent(groupId)}/tags`, "POST", {
      tagName,
    });
  },

  removeGroupTag(groupId: string, tagName: string): Promise<GroupDetail> {
    if (isTauriRuntime()) {
      return invoke("remove_group_tag", { groupId, tagName });
    }
    return webJson<GroupDetail>(
      `/api/groups/${encodeURIComponent(groupId)}/tags/${encodeURIComponent(tagName)}`,
      "DELETE"
    );
  },

  // Trash
  moveEntriesToTrash(entries: EntryRef[]): Promise<number> {
    if (isTauriRuntime()) {
      return invoke("move_entries_to_trash", { entries });
    }
    return webJson<number>("/api/trash/move", "POST", { entries });
  },

  restoreEntriesFromTrash(entries: EntryRef[]): Promise<number> {
    if (isTauriRuntime()) {
      return invoke("restore_entries_from_trash", { entries });
    }
    return webJson<number>("/api/trash/restore", "POST", { entries });
  },

  permanentlyDeleteEntries(entries: EntryRef[]): Promise<number> {
    if (isTauriRuntime()) {
      return invoke("permanently_delete_entries", { entries });
    }
    return webJson<number>("/api/trash/delete", "POST", { entries });
  },

  emptyTrash(): Promise<number> {
    if (isTauriRuntime()) {
      return invoke("empty_trash");
    }
    return webJson<number>("/api/trash/empty", "POST");
  },

  // Maintenance, Optimization, Backup & Partial Exchange Packages (Host Only)
  getVaultHealth(): Promise<VaultHealthInfo> {
    return invoke("get_vault_health");
  },

  optimizeVault(): Promise<OptimizeReport> {
    return invoke("optimize_vault");
  },

  backupVault(destPath: string, overwrite = false): Promise<BackupReport> {
    return invoke("backup_vault", { destPath, overwrite });
  },

  exportPackage(request: PackageExportRequest): Promise<PackageExportReport> {
    return invoke("export_package", { request });
  },

  importPackage(request: PackageImportRequest): Promise<PackageImportReport> {
    return invoke("import_package", { request });
  },

  // Plain-File Export & Browser Downloads
  setSkipPlainExportConfirm(skip: boolean): Promise<boolean> {
    return invoke("set_skip_plain_export_confirm", { skip });
  },

  exportPlainItem(request: PlainExportItemRequest): Promise<PlainExportReport> {
    return invoke("export_plain_item", { request });
  },

  exportPlainBatch(request: PlainExportBatchRequest): Promise<PlainExportReport> {
    return invoke("export_plain_batch", { request });
  },

  async verifyWebExportHead(path: string): Promise<void> {
    await webRequest(path, { method: "HEAD" });
  },

  createWebExportBatchTicket(
    entries: EntryRef[],
    folderIds: string[],
    includeSubfolders: boolean
  ): Promise<WebBatchExportTicketResponse> {
    return webJson<WebBatchExportTicketResponse>("/api/export/batch-ticket", "POST", {
      entries,
      folderIds,
      includeSubfolders,
    });
  },

  // LAN Sharing Controls (Host Only)
  getLanNetworkInterfaces(): Promise<LanInterfaceInfo[]> {
    return invoke("get_lan_network_interfaces");
  },

  getLanShareStatus(): Promise<LanShareStatus> {
    return invoke("get_lan_share_status");
  },

  startLanShare(request: StartLanShareRequest): Promise<LanShareStatus> {
    return invoke("start_lan_share", { request });
  },

  stopLanShare(): Promise<LanShareStatus> {
    return invoke("stop_lan_share");
  },

  updateLanShareMode(mode: LanAccessMode): Promise<LanShareStatus> {
    return invoke("update_lan_share_mode", { mode });
  },

  extendLanShareTimer(extraMinutes: number): Promise<LanShareStatus> {
    return invoke("extend_lan_share_timer", { extraMinutes });
  },

  regenerateLanCredentials(disconnectExisting: boolean): Promise<LanShareStatus> {
    return invoke("regenerate_lan_credentials", { disconnectExisting });
  },

  disconnectLanClients(): Promise<LanShareStatus> {
    return invoke("disconnect_lan_clients");
  },

  // LAN Auth (Web Client Only)
  getLanAuthStatus(): Promise<LanAuthStatus> {
    return webJson<LanAuthStatus>("/api/auth/status", "GET", undefined, false);
  },

  authenticateLanPin(pin: string): Promise<LanAuthStatus> {
    return webJson<LanAuthStatus>("/api/auth/pin", "POST", { pin }, false);
  },

  authenticateLanToken(token: string): Promise<LanAuthStatus> {
    return webJson<LanAuthStatus>("/api/auth/token", "POST", { token }, false);
  },

  logoutLanSession(): Promise<void> {
    return webJson<void>("/api/auth/logout", "POST", undefined, false);
  },
};
