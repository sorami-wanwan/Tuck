import { useCallback, useEffect, useRef, useState, type ChangeEvent } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { imageMemoryCache } from "../services/imageCache";
import { formatErrorMessage, vaultApi } from "../services/vaultApi";
import { entryKey, type ImportReport, type ToastItem } from "../types/vault";

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target.tagName === "INPUT" ||
    target.tagName === "TEXTAREA" ||
    target.tagName === "SELECT" ||
    target.isContentEditable
  );
}

interface UseImportFlowOptions {
  isOpen: boolean;
  canEdit: boolean;
  isWebClient: boolean;
  currentFolderId: string | null;
  autoUrlImportEnabled: boolean;
  lockSettingsOpen: boolean;
  refreshLibrary: () => Promise<void>;
  onSelectImportedKey: (key: string) => void;
  pushToast: (tone: ToastItem["tone"], message: string) => void;
}

/**
 * ファイル選択・OS / ブラウザ D&D・クリップボード貼り付け・URL・漫画フォルダ取り込みと後続サムネイル生成を管理するフック。
 */
export function useImportFlow({
  isOpen,
  canEdit,
  isWebClient,
  currentFolderId,
  autoUrlImportEnabled,
  lockSettingsOpen,
  refreshLibrary,
  onSelectImportedKey,
  pushToast,
}: UseImportFlowOptions) {
  const [isDraggingOsFiles, setIsDraggingOsFiles] = useState(false);
  const [urlImportDialog, setUrlImportDialog] = useState<{
    open: boolean;
    initialUrl: string;
    autoStart: boolean;
  }>({
    open: false,
    initialUrl: "",
    autoStart: false,
  });

  const browserUploadInputRef = useRef<HTMLInputElement>(null);
  const pasteHandledRef = useRef(false);

  const processPendingThumbnails = useCallback(
    (avifIds: string[], videoIds: string[]) => {
      for (const avifId of avifIds) {
        void imageMemoryCache
          .processPendingAvifThumbnail(avifId)
          .then(() => refreshLibrary())
          .catch(() => {});
      }
      for (const videoId of videoIds) {
        void imageMemoryCache
          .processPendingVideoThumbnail(videoId, "")
          .then(() => refreshLibrary())
          .catch(() => {});
      }
    },
    [refreshLibrary]
  );

  const handleImportReport = useCallback(
    async (report: ImportReport) => {
      const imported = report.importedIds.length;
      const dups = report.duplicateCount;
      const skipped = report.skippedUnsupportedCount;

      if (imported > 0) {
        const notes: string[] = [];
        if (dups > 0) notes.push(`${dups}件は重複スキップ`);
        if (skipped > 0) notes.push(`${skipped}件は非対応形式`);
        const detail = notes.length > 0 ? `（${notes.join("、")}）` : "";
        pushToast("success", `${imported}件を取り込みました${detail}`);
        const k = entryKey({
          kind: "image",
          id: report.importedIds[report.importedIds.length - 1],
        });
        onSelectImportedKey(k);
      } else if (dups > 0) {
        pushToast("info", `${dups}件はすでに保管庫に登録済みです（重複スキップ）`);
      } else if (skipped > 0) {
        pushToast("warning", `非対応のファイル形式のため ${skipped}件をスキップしました`);
      }

      if (report.errors.length > 0) {
        pushToast("error", report.errors[0]);
      }

      await refreshLibrary();
      processPendingThumbnails(report.avifPendingIds, report.videoPendingIds);
    },
    [pushToast, refreshLibrary, processPendingThumbnails, onSelectImportedKey]
  );

  const handleImportUrl = useCallback(
    async (rawUrl: string) => {
      const report = await vaultApi.importUrl(rawUrl, currentFolderId);
      await handleImportReport(report);
    },
    [currentFolderId, handleImportReport]
  );

  const uploadBrowserFiles = useCallback(
    async (files: File[], folderId: string | null): Promise<ImportReport> => {
      const agg: ImportReport = {
        importedIds: [],
        avifPendingIds: [],
        videoPendingIds: [],
        duplicateCount: 0,
        skippedUnsupportedCount: 0,
        errors: [],
      };
      for (const file of files) {
        const title = file.name ? file.name.replace(/\.[^.]+$/, "") : undefined;
        try {
          const rep = await vaultApi.importBytes(file, title, folderId);
          agg.importedIds.push(...rep.importedIds);
          agg.avifPendingIds.push(...rep.avifPendingIds);
          agg.videoPendingIds.push(...rep.videoPendingIds);
          agg.duplicateCount += rep.duplicateCount;
          agg.skippedUnsupportedCount += rep.skippedUnsupportedCount;
          agg.errors.push(...rep.errors);
        } catch (err) {
          const msg = formatErrorMessage(err);
          if (msg.includes("非対応")) {
            agg.skippedUnsupportedCount += 1;
          } else {
            agg.errors.push(`${file.name}: ${msg}`);
          }
        }
      }
      return agg;
    },
    []
  );

  const handleImportFiles = useCallback(async () => {
    if (!canEdit) return;
    if (isWebClient) {
      browserUploadInputRef.current?.click();
      return;
    }
    try {
      const selected = await open({
        title: "取り込む画像・動画を選択",
        multiple: true,
        directory: false,
        filters: [
          {
            name: "画像・動画ファイル",
            extensions: ["png", "jpg", "jpeg", "webp", "gif", "avif", "mp4", "webm", "mov", "m4v"],
          },
        ],
      });
      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      if (paths.length === 0) return;
      const report = await vaultApi.importPaths(paths, currentFolderId);
      await handleImportReport(report);
    } catch (err) {
      pushToast("error", formatErrorMessage(err));
    }
  }, [canEdit, isWebClient, currentFolderId, handleImportReport, pushToast]);

  const handleImportFolderAsGroup = useCallback(async () => {
    try {
      const selected = await open({
        title: "1冊の漫画として取り込むフォルダを選択",
        multiple: false,
        directory: true,
      });
      if (typeof selected !== "string" || !selected) return;
      const res = await vaultApi.importFolderAsGroup(selected, currentFolderId);
      if (res.group) {
        const gk = entryKey({ kind: "group", id: res.group.id });
        onSelectImportedKey(gk);
        pushToast(
          "success",
          `「${res.group.title}」（${res.group.pageCount}ページ）を取り込みました`
        );
      } else {
        await handleImportReport(res.report);
      }
      await refreshLibrary();
      processPendingThumbnails(res.report.avifPendingIds, res.report.videoPendingIds);
    } catch (err) {
      pushToast("error", formatErrorMessage(err));
    }
  }, [
    currentFolderId,
    handleImportReport,
    onSelectImportedKey,
    processPendingThumbnails,
    pushToast,
    refreshLibrary,
  ]);

  const handleBrowserUploadChange = useCallback(
    (e: ChangeEvent<HTMLInputElement>) => {
      const files = Array.from(e.currentTarget.files ?? []);
      e.currentTarget.value = "";
      if (files.length === 0 || !canEdit) return;
      void uploadBrowserFiles(files, currentFolderId)
        .then(handleImportReport)
        .catch((err) => pushToast("error", formatErrorMessage(err)));
    },
    [canEdit, currentFolderId, uploadBrowserFiles, handleImportReport, pushToast]
  );

  const closeUrlImportDialog = useCallback(() => {
    setUrlImportDialog({ open: false, initialUrl: "", autoStart: false });
  }, []);

  // OS / Browser Drag & Drop listener
  useEffect(() => {
    const prevent = (e: DragEvent) => {
      e.preventDefault();
    };
    window.addEventListener("dragover", prevent);
    window.addEventListener("drop", prevent);

    if (!isOpen || !canEdit) {
      setIsDraggingOsFiles(false);
      return () => {
        window.removeEventListener("dragover", prevent);
        window.removeEventListener("drop", prevent);
      };
    }

    if (isWebClient) {
      let dragDepth = 0;
      const onDragEnter = (e: DragEvent) => {
        if (e.dataTransfer?.types.includes("Files")) {
          dragDepth += 1;
          setIsDraggingOsFiles(true);
        }
      };
      const onDragLeave = (e: DragEvent) => {
        if (e.dataTransfer?.types.includes("Files")) {
          dragDepth = Math.max(0, dragDepth - 1);
          if (dragDepth === 0) setIsDraggingOsFiles(false);
        }
      };
      const onDropFiles = (e: DragEvent) => {
        dragDepth = 0;
        setIsDraggingOsFiles(false);
        const files = Array.from(e.dataTransfer?.files ?? []);
        if (files.length === 0) return;
        void uploadBrowserFiles(files, currentFolderId)
          .then(handleImportReport)
          .catch((err) => pushToast("error", formatErrorMessage(err)));
      };
      window.addEventListener("dragenter", onDragEnter);
      window.addEventListener("dragleave", onDragLeave);
      window.addEventListener("drop", onDropFiles);
      return () => {
        setIsDraggingOsFiles(false);
        window.removeEventListener("dragenter", onDragEnter);
        window.removeEventListener("dragleave", onDragLeave);
        window.removeEventListener("drop", onDropFiles);
        window.removeEventListener("dragover", prevent);
        window.removeEventListener("drop", prevent);
      };
    }

    let unlisten: (() => void) | undefined;
    let active = true;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (!active) return;
        if (event.payload.type === "enter" || event.payload.type === "over") {
          setIsDraggingOsFiles(true);
        } else if (event.payload.type === "leave") {
          setIsDraggingOsFiles(false);
        } else if (event.payload.type === "drop") {
          setIsDraggingOsFiles(false);
          const paths = event.payload.paths;
          if (paths.length > 0) {
            void vaultApi
              .importPaths(paths, currentFolderId)
              .then(handleImportReport)
              .catch((err) => pushToast("error", formatErrorMessage(err)));
          }
        }
      })
      .then((fn) => {
        if (active) unlisten = fn;
        else fn();
      });

    return () => {
      active = false;
      setIsDraggingOsFiles(false);
      unlisten?.();
      window.removeEventListener("dragover", prevent);
      window.removeEventListener("drop", prevent);
    };
  }, [
    isOpen,
    canEdit,
    isWebClient,
    currentFolderId,
    handleImportReport,
    uploadBrowserFiles,
    pushToast,
  ]);

  // Global clipboard paste (Cmd/Ctrl+V)
  useEffect(() => {
    if (!isOpen || !canEdit) return;

    const handlePaste = async (e: ClipboardEvent) => {
      if (
        isEditableTarget(e.target) ||
        isEditableTarget(document.activeElement) ||
        urlImportDialog.open ||
        lockSettingsOpen
      ) {
        return;
      }
      pasteHandledRef.current = true;
      e.preventDefault();

      const clipItems = Array.from(e.clipboardData?.items ?? []);
      const imageFiles = clipItems
        .filter((it) => it.type.startsWith("image/"))
        .map((it) => it.getAsFile())
        .filter((f): f is File => f !== null);
      const pastedText = e.clipboardData?.getData("text/plain")?.trim() ?? "";

      try {
        if (imageFiles.length > 0) {
          const rep = await uploadBrowserFiles(imageFiles, currentFolderId);
          await handleImportReport(rep);
          return;
        }
        if (/^https?:\/\/\S+$/i.test(pastedText)) {
          setUrlImportDialog({
            open: true,
            initialUrl: pastedText,
            autoStart: autoUrlImportEnabled,
          });
          return;
        }
        if (!isWebClient) {
          const report = await vaultApi.importClipboard(currentFolderId);
          await handleImportReport(report);
        }
      } catch (err) {
        pushToast("warning", formatErrorMessage(err));
      }
    };

    const handleKeyDown = (e: KeyboardEvent) => {
      if (isWebClient) return;
      if (
        isEditableTarget(e.target) ||
        isEditableTarget(document.activeElement) ||
        urlImportDialog.open ||
        lockSettingsOpen
      ) {
        return;
      }
      if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "v") {
        pasteHandledRef.current = false;
        window.setTimeout(() => {
          if (pasteHandledRef.current) return;
          void vaultApi
            .importClipboard(currentFolderId)
            .then(handleImportReport)
            .catch((err) => pushToast("warning", formatErrorMessage(err)));
        }, 60);
      }
    };

    window.addEventListener("paste", handlePaste);
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("paste", handlePaste);
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [
    isOpen,
    canEdit,
    isWebClient,
    autoUrlImportEnabled,
    urlImportDialog.open,
    lockSettingsOpen,
    currentFolderId,
    handleImportReport,
    uploadBrowserFiles,
    pushToast,
  ]);

  return {
    isDraggingOsFiles,
    urlImportDialog,
    setUrlImportDialog,
    closeUrlImportDialog,
    browserUploadInputRef,
    processPendingThumbnails,
    handleImportReport,
    handleImportUrl,
    uploadBrowserFiles,
    handleImportFiles,
    handleImportFolderAsGroup,
    handleBrowserUploadChange,
  };
}
