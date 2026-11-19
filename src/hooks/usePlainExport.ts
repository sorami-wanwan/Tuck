import { useCallback, useState, type Dispatch, type SetStateAction } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { formatErrorMessage, vaultApi } from "../services/vaultApi";
import type { PlainExportTarget, ToastItem, VaultStatus } from "../types/vault";
import { formatBytes } from "../utils/format";

function mimeToExt(mime: string): string {
  switch (mime.trim().toLowerCase()) {
    case "image/png":
      return "png";
    case "image/jpeg":
      return "jpg";
    case "image/webp":
      return "webp";
    case "image/gif":
      return "gif";
    case "image/avif":
      return "avif";
    case "image/bmp":
    case "image/x-ms-bmp":
      return "bmp";
    case "image/tiff":
      return "tiff";
    case "image/x-icon":
    case "image/vnd.microsoft.icon":
      return "ico";
    case "video/mp4":
      return "mp4";
    case "video/webm":
      return "webm";
    case "video/quicktime":
      return "mov";
    default:
      return "bin";
  }
}

function sanitizeFileSegment(raw: string, fallback: string): string {
  const cleaned = raw
    .normalize("NFC")
    .replace(/[\u0000-\u001F\u007F/\\:*?"<>|]/g, "_")
    .replace(/^[\s.]+|[\s.]+$/g, "")
    .slice(0, 80)
    .replace(/^[\s.]+|[\s.]+$/g, "");
  if (!cleaned || cleaned === "." || cleaned === "..") {
    return fallback;
  }
  return cleaned;
}

function buildDefaultItemFileName(title: string, itemId: string, mimeType: string): string {
  const ext = mimeToExt(mimeType);
  let stripped = title.trim();
  const knownExts: Record<string, string[]> = {
    "image/png": [".png"],
    "image/jpeg": [".jpg", ".jpeg", ".jpe"],
    "image/webp": [".webp"],
    "image/gif": [".gif"],
    "image/avif": [".avif"],
    "image/bmp": [".bmp", ".dib"],
    "image/tiff": [".tiff", ".tif"],
    "image/x-icon": [".ico"],
    "video/mp4": [".mp4", ".m4v"],
    "video/webm": [".webm"],
    "video/quicktime": [".mov", ".qt"],
  };
  const candidates = knownExts[mimeType.trim().toLowerCase()] ?? [];
  const lower = stripped.toLowerCase();
  for (const suffix of candidates) {
    if (lower.endsWith(suffix) && stripped.length > suffix.length) {
      stripped = stripped.slice(0, stripped.length - suffix.length);
      break;
    }
  }
  const stem = sanitizeFileSegment(stripped, `item_${itemId.slice(0, 8) || "file"}`);
  return `${stem}.${ext}`;
}

function triggerBrowserDownload(url: string): void {
  const a = document.createElement("a");
  a.href = url;
  a.download = "";
  a.style.display = "none";
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
}

interface UsePlainExportOptions {
  isWebClient: boolean;
  status: VaultStatus;
  setStatus: Dispatch<SetStateAction<VaultStatus>>;
  pushToast: (tone: ToastItem["tone"], message: string) => void;
}

/**
 * 通常ファイル（平文）としての書き出し確認ダイアログ・保存先選択・Web ブラウザダウンロードを管理するフック。
 */
export function usePlainExport({
  isWebClient,
  status,
  setStatus,
  pushToast,
}: UsePlainExportOptions) {
  const [pendingTarget, setPendingTarget] = useState<PlainExportTarget | null>(null);
  const [exporting, setExporting] = useState(false);
  const [webSkipConfirm, setWebSkipConfirm] = useState(false);

  const resetPlainExportState = useCallback(() => {
    setPendingTarget(null);
    setExporting(false);
    setWebSkipConfirm(false);
  }, []);

  const persistSkipIfRequested = useCallback(
    async (dontAskAgain: boolean) => {
      if (!dontAskAgain) return;
      if (isWebClient) {
        setWebSkipConfirm(true);
        return;
      }
      if (status.skipPlainExportConfirm) return;
      const saved = await vaultApi.setSkipPlainExportConfirm(true);
      setStatus((prev) => ({ ...prev, skipPlainExportConfirm: saved }));
    },
    [isWebClient, status.skipPlainExportConfirm, setStatus]
  );

  const executePlainExport = useCallback(
    async (target: PlainExportTarget, includeSubfolders: boolean, dontAskAgain: boolean) => {
      setExporting(true);
      try {
        if (isWebClient) {
          let downloadUrl = "";
          switch (target.kind) {
            case "item": {
              downloadUrl = `/api/export/item/${encodeURIComponent(target.itemId)}`;
              await vaultApi.verifyWebExportHead(downloadUrl);
              break;
            }
            case "group": {
              downloadUrl = `/api/export/group/${encodeURIComponent(target.groupId)}`;
              await vaultApi.verifyWebExportHead(downloadUrl);
              break;
            }
            case "folder": {
              downloadUrl = `/api/export/folder/${encodeURIComponent(target.folderId)}?includeSubfolders=${
                includeSubfolders ? "true" : "false"
              }`;
              await vaultApi.verifyWebExportHead(downloadUrl);
              break;
            }
            case "entries": {
              const ticketRes = await vaultApi.createWebExportBatchTicket(
                target.entries,
                [],
                true
              );
              downloadUrl = `/api/export/batch/${encodeURIComponent(ticketRes.ticket)}`;
              break;
            }
          }
          await persistSkipIfRequested(dontAskAgain);
          triggerBrowserDownload(downloadUrl);
          pushToast("info", "ダウンロードを開始しました");
          return;
        }

        if (target.kind === "item") {
          const ext = mimeToExt(target.mimeType);
          const defaultPath = buildDefaultItemFileName(
            target.title,
            target.itemId,
            target.mimeType
          );
          const destPath = await save({
            title: "通常のファイルとして書き出し",
            defaultPath,
            filters: [{ name: `${ext.toUpperCase()} ファイル`, extensions: [ext] }],
          });
          if (!destPath) {
            return;
          }
          await persistSkipIfRequested(dontAskAgain);
          const report = await vaultApi.exportPlainItem({
            itemId: target.itemId,
            destPath,
            overwrite: true,
          });
          pushToast("success", `ファイルを書き出しました（${formatBytes(report.totalBytes)}）`);
          return;
        }

        const destDir = await open({
          title: "書き出し先のフォルダを選択",
          multiple: false,
          directory: true,
        });
        if (typeof destDir !== "string" || !destDir) {
          return;
        }
        await persistSkipIfRequested(dontAskAgain);

        const report = await vaultApi.exportPlainBatch({
          destDir,
          entries:
            target.kind === "group"
              ? [{ kind: "group", id: target.groupId }]
              : target.kind === "entries"
                ? target.entries
                : [],
          folderIds: target.kind === "folder" ? [target.folderId] : [],
          includeSubfolders,
        });
        pushToast(
          "success",
          `${report.fileCount}件のファイルを書き出しました（${formatBytes(report.totalBytes)}）`
        );
      } catch (err) {
        pushToast("error", formatErrorMessage(err));
      } finally {
        setExporting(false);
      }
    },
    [isWebClient, persistSkipIfRequested, pushToast]
  );

  const requestPlainExport = useCallback(
    (target: PlainExportTarget) => {
      const skip = isWebClient ? webSkipConfirm : status.skipPlainExportConfirm;
      if (skip) {
        void executePlainExport(target, true, false);
        return;
      }
      setPendingTarget(target);
    },
    [isWebClient, webSkipConfirm, status.skipPlainExportConfirm, executePlainExport]
  );

  const cancelPendingExport = useCallback(() => {
    if (!exporting) {
      setPendingTarget(null);
    }
  }, [exporting]);

  const confirmPendingExport = useCallback(
    ({ includeSubfolders, dontAskAgain }: { includeSubfolders: boolean; dontAskAgain: boolean }) => {
      const target = pendingTarget;
      if (!target) return;
      setPendingTarget(null);
      void executePlainExport(target, includeSubfolders, dontAskAgain);
    },
    [pendingTarget, executePlainExport]
  );

  return {
    pendingTarget,
    exporting,
    requestPlainExport,
    cancelPendingExport,
    confirmPendingExport,
    resetPlainExportState,
  };
}
