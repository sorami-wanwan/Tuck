import { useCallback, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { imageMemoryCache } from "../services/imageCache";
import { formatErrorMessage, vaultApi } from "../services/vaultApi";
import { sortImagesOldestFirst } from "../utils/groupOrder";
import {
  isVideoMime,
  type EntryRef,
  type FolderNode,
  type GalleryEntry,
  type GroupDetail,
  type ImportReport,
  type ItemDetail,
  type ToastItem,
  type VaultStatus,
  type ViewSelection,
} from "../types/vault";

interface GalleryMutationTarget {
  inspectorMode: "empty" | "image" | "group" | "multi";
  selectedEntryRefs: EntryRef[];
  groupDetail: GroupDetail | null;
  setItemDetail: (detail: ItemDetail | null) => void;
  setGroupDetail: (detail: GroupDetail | null) => void;
  setActiveModalGroup: (group: GroupDetail | null) => void;
  refreshSelectedDetail: () => void;
  setLightboxId: (id: string | null) => void;
  setReaderGroupId: (id: string | null) => void;
  setEditorGroupId: (id: string | null) => void;
}

interface UseLibraryMutationsOptions {
  entries: GalleryEntry[];
  folders: FolderNode[];
  view: ViewSelection;
  currentFolderId: string | null;
  hasActiveFilters: boolean;
  navigateView: (next: ViewSelection) => void;
  refreshLibrary: () => Promise<void>;
  setStatus: Dispatch<SetStateAction<VaultStatus>>;
  pushToast: (tone: ToastItem["tone"], message: string) => void;
  uploadBrowserFiles: (files: File[], folderId: string | null) => Promise<ImportReport>;
  processPendingThumbnails: (avifPendingIds: string[], videoPendingIds: string[]) => void;
  gallery: GalleryMutationTarget;
}

export function useLibraryMutations({
  entries,
  folders,
  view,
  currentFolderId,
  hasActiveFilters,
  navigateView,
  refreshLibrary,
  setStatus,
  pushToast,
  uploadBrowserFiles,
  processPendingThumbnails,
  gallery,
}: UseLibraryMutationsOptions) {
  const [batchRatingBusy, setBatchRatingBusy] = useState(false);
  const batchRatingBusyRef = useRef(false);
  const applyBatchRatingRef = useRef<(targetEntries: EntryRef[], rating: number) => Promise<void>>(
    async () => {}
  );

  const safeMutate = useCallback(
    async (fn: () => Promise<void>) => {
      try {
        await fn();
      } catch (err) {
        pushToast("error", formatErrorMessage(err));
      }
    },
    [pushToast]
  );

  const refreshSingleSelectedDetail = useCallback(
    async (targetEntries: EntryRef[]) => {
      if (gallery.selectedEntryRefs.length !== 1) return;
      const singleRef = gallery.selectedEntryRefs[0];
      if (!targetEntries.some((t) => t.kind === singleRef.kind && t.id === singleRef.id)) return;
      if (singleRef.kind === "image") {
        gallery.setItemDetail(await vaultApi.getItemDetail(singleRef.id));
      } else {
        gallery.setGroupDetail(await vaultApi.getGroupDetail(singleRef.id));
      }
    },
    [gallery]
  );

  const applyBatchTagUpdate = useCallback(
    async (targetEntries: EntryRef[], addTags: string[], removeTags: string[]) => {
      const report = await vaultApi.batchUpdateEntryTags(targetEntries, addTags, removeTags);
      if (addTags.length === 1 && removeTags.length === 0) {
        if (report.updatedCount > 0) {
          pushToast("success", `「#${addTags[0]}」を ${report.updatedCount}件に付与しました`);
        } else if (report.targetCount > 0) {
          pushToast("info", `選択した項目にはすでに「#${addTags[0]}」が付いています`);
        }
      } else if (addTags.length === 0 && removeTags.length === 1) {
        if (report.updatedCount > 0) {
          pushToast("info", `「#${removeTags[0]}」を ${report.updatedCount}件から外しました`);
        }
      } else if (report.updatedCount > 0) {
        pushToast("success", `${report.updatedCount}件のタグを更新しました`);
      } else {
        pushToast("info", "タグに変更はありませんでした");
      }

      await refreshSingleSelectedDetail(targetEntries);
      await refreshLibrary();
    },
    [pushToast, refreshSingleSelectedDetail, refreshLibrary]
  );

  const applyBatchRatingUpdate = useCallback(
    async (targetEntries: EntryRef[], rating: number) => {
      if (targetEntries.length === 0 || batchRatingBusyRef.current) return;
      batchRatingBusyRef.current = true;
      setBatchRatingBusy(true);
      try {
        const report = await vaultApi.batchSetEntryRating(targetEntries, rating);
        if (rating === 0) {
          if (report.updatedCount > 0) {
            pushToast("info", `${report.updatedCount}件の評価を解除しました`);
          } else if (report.targetCount > 0) {
            pushToast("info", "選択した項目はすでに未評価です");
          }
        } else if (report.updatedCount > 0) {
          pushToast("success", `${report.updatedCount}件の評価を ★${rating} に設定しました`);
        } else if (report.targetCount > 0) {
          pushToast("info", `選択した項目はすでに ★${rating} です`);
        }

        await refreshSingleSelectedDetail(targetEntries);
        await refreshLibrary();
      } catch (err) {
        pushToast("error", formatErrorMessage(err));
      } finally {
        batchRatingBusyRef.current = false;
        setBatchRatingBusy(false);
      }
    },
    [pushToast, refreshSingleSelectedDetail, refreshLibrary]
  );
  applyBatchRatingRef.current = applyBatchRatingUpdate;

  const triggerBatchSetRating = useCallback(
    (targetEntries: EntryRef[], rating: number) =>
      applyBatchRatingRef.current(targetEntries, rating),
    []
  );

  const resetMutationState = useCallback(() => {
    batchRatingBusyRef.current = false;
    setBatchRatingBusy(false);
  }, []);

  const toggleClipboardWatch = useCallback(
    (currentlyEnabled: boolean) => {
      void safeMutate(async () => {
        const nextStatus = await vaultApi.setClipboardWatch(!currentlyEnabled);
        setStatus(nextStatus);
        pushToast(
          "info",
          nextStatus.clipboardWatchEnabled
            ? "クリップボード監視モードをオンにしました"
            : "クリップボード監視モードをオフにしました"
        );
      });
    },
    [safeMutate, setStatus, pushToast]
  );

  const folderHandlers = {
    createFolder: async (parentId: string | null, name: string, color: string) => {
      await safeMutate(async () => {
        await vaultApi.createFolder(parentId, name, color);
        await refreshLibrary();
      });
    },
    renameFolder: async (folderId: string, name: string, color?: string | null) => {
      await safeMutate(async () => {
        await vaultApi.renameFolder(folderId, name, color);
        await refreshLibrary();
      });
    },
    moveFolder: async (folderId: string, newParentId: string | null) => {
      await safeMutate(async () => {
        await vaultApi.moveFolder(folderId, newParentId);
        await refreshLibrary();
      });
    },
    deleteFolder: async (folderId: string) => {
      await safeMutate(async () => {
        await vaultApi.deleteFolder(folderId);
        if (view.kind === "folder" && view.folderId === folderId) {
          navigateView({ kind: "all" });
        }
        pushToast("info", "フォルダを削除しました（中の画像は保管庫に残ります）");
        await refreshLibrary();
      });
    },
  };

  const dragDropHandlers = {
    onDropEntriesToFolder: (refs: EntryRef[], folderId: string) => {
      void safeMutate(async () => {
        await vaultApi.addEntriesToFolder(folderId, refs);
        const fname = folders.find((f) => f.id === folderId)?.name ?? "フォルダ";
        pushToast("success", `「${fname}」へ ${refs.length}件を追加しました`);
        await refreshLibrary();
      });
    },
    onDropEntriesToTrash: (refs: EntryRef[]) => {
      void safeMutate(async () => {
        await vaultApi.moveEntriesToTrash(refs);
        pushToast("info", `${refs.length}件をゴミ箱へ移動しました`);
        await refreshLibrary();
      });
    },
    onDropImagesToGroup: (itemIds: string[], groupId: string) => {
      const entryById = new Map<string, GalleryEntry>();
      for (const entry of entries) {
        if (entry.kind === "image") {
          entryById.set(entry.id, entry);
        }
      }
      const resolvedEntries: GalleryEntry[] = [];
      const fallbackIds: string[] = [];
      for (const id of itemIds) {
        const found = entryById.get(id);
        if (found) {
          if (!isVideoMime(found.coverMimeType)) resolvedEntries.push(found);
        } else {
          fallbackIds.push(id);
        }
      }
      const orderedIds = [
        ...sortImagesOldestFirst(resolvedEntries).map((e) => e.id),
        ...fallbackIds,
      ];
      if (orderedIds.length === 0) return;

      void safeMutate(async () => {
        await vaultApi.addItemsToGroup(groupId, orderedIds);
        pushToast("success", `${orderedIds.length}枚を漫画グループへ追加しました`);
        await refreshLibrary();
      });
    },
    onDropEntriesToTag: (refs: EntryRef[], tagName: string) => {
      void safeMutate(() => applyBatchTagUpdate(refs, [tagName], []));
    },
    onDropFolderToParent: (folderId: string, newParentId: string | null) => {
      void folderHandlers.moveFolder(folderId, newParentId);
    },
  };

  const gridBulkHandlers = {
    removeSelectionFromFolder: () => {
      if (!currentFolderId || gallery.selectedEntryRefs.length === 0) return;
      void safeMutate(async () => {
        await vaultApi.removeEntriesFromFolder(currentFolderId, gallery.selectedEntryRefs);
        pushToast("info", "選択した項目をフォルダから外しました");
        await refreshLibrary();
      });
    },
    moveSelectionToTrash: () => {
      if (gallery.selectedEntryRefs.length === 0) return;
      void safeMutate(async () => {
        await vaultApi.moveEntriesToTrash(gallery.selectedEntryRefs);
        pushToast("info", `${gallery.selectedEntryRefs.length}件をゴミ箱へ移動しました`);
        await refreshLibrary();
      });
    },
    restoreAllTrash: () => {
      const allRefs = entries.map((e) => ({ kind: e.kind, id: e.id }));
      if (allRefs.length === 0) return;
      void safeMutate(async () => {
        await vaultApi.restoreEntriesFromTrash(allRefs);
        pushToast(
          "success",
          hasActiveFilters
            ? `表示中の ${allRefs.length}件を復元しました`
            : "ゴミ箱の項目をすべて復元しました"
        );
        await refreshLibrary();
      });
    },
    emptyTrash: () => {
      void safeMutate(async () => {
        await vaultApi.emptyTrash();
        imageMemoryCache.clearAll();
        pushToast("info", "ゴミ箱を空にしました");
        await refreshLibrary();
      });
    },
  };

  const inspectorHandlers = {
    updateTitle: async (id: string, title: string) => {
      await safeMutate(async () => {
        if (gallery.inspectorMode === "group") {
          gallery.setGroupDetail(await vaultApi.setGroupTitle(id, title));
        } else {
          gallery.setItemDetail(await vaultApi.setItemTitle(id, title));
        }
        await refreshLibrary();
      });
    },
    updateRating: async (id: string, rating: number) => {
      await safeMutate(async () => {
        if (gallery.inspectorMode === "group") {
          gallery.setGroupDetail(await vaultApi.setGroupRating(id, rating));
        } else {
          gallery.setItemDetail(await vaultApi.setItemRating(id, rating));
        }
        await refreshLibrary();
      });
    },
    updateMemo: async (id: string, memo: string) => {
      await safeMutate(async () => {
        if (gallery.inspectorMode === "group") {
          gallery.setGroupDetail(await vaultApi.setGroupMemo(id, memo));
        } else {
          gallery.setItemDetail(await vaultApi.setItemMemo(id, memo));
        }
        await refreshLibrary();
      });
    },
    updateSourceUrl: async (id: string, sourceUrl: string) => {
      await safeMutate(async () => {
        gallery.setItemDetail(await vaultApi.setItemSourceUrl(id, sourceUrl));
        await refreshLibrary();
      });
    },
    addTag: async (id: string, tag: string) => {
      await safeMutate(async () => {
        if (gallery.inspectorMode === "group") {
          gallery.setGroupDetail(await vaultApi.addGroupTag(id, tag));
        } else {
          gallery.setItemDetail(await vaultApi.addItemTag(id, tag));
        }
        await refreshLibrary();
      });
    },
    removeTag: async (id: string, tag: string) => {
      await safeMutate(async () => {
        if (gallery.inspectorMode === "group") {
          gallery.setGroupDetail(await vaultApi.removeGroupTag(id, tag));
        } else {
          gallery.setItemDetail(await vaultApi.removeItemTag(id, tag));
        }
        await refreshLibrary();
      });
    },
    batchUpdateTags: async (addTags: string[], removeTags: string[]) => {
      try {
        await applyBatchTagUpdate(gallery.selectedEntryRefs, addTags, removeTags);
      } catch (err) {
        pushToast("error", formatErrorMessage(err));
        throw err;
      }
    },
    addToFolder: async (folderId: string) => {
      await safeMutate(async () => {
        await vaultApi.addEntriesToFolder(folderId, gallery.selectedEntryRefs);
        await refreshSingleSelectedDetail(gallery.selectedEntryRefs);
        await refreshLibrary();
      });
    },
    removeFromFolder: async (folderId: string) => {
      await safeMutate(async () => {
        await vaultApi.removeEntriesFromFolder(folderId, gallery.selectedEntryRefs);
        await refreshSingleSelectedDetail(gallery.selectedEntryRefs);
        await refreshLibrary();
      });
    },
    ungroup: async () => {
      if (!gallery.groupDetail) return;
      const gid = gallery.groupDetail.id;
      await safeMutate(async () => {
        await vaultApi.ungroup(gid);
        pushToast("info", "グループを解除して単体画像に戻しました");
        await refreshLibrary();
      });
    },
    moveToTrash: async () => {
      await safeMutate(async () => {
        await vaultApi.moveEntriesToTrash(gallery.selectedEntryRefs);
        pushToast("info", `${gallery.selectedEntryRefs.length}件をゴミ箱へ移動しました`);
        await refreshLibrary();
      });
    },
    restoreFromTrash: async () => {
      await safeMutate(async () => {
        await vaultApi.restoreEntriesFromTrash(gallery.selectedEntryRefs);
        pushToast("success", `${gallery.selectedEntryRefs.length}件を復元しました`);
        await refreshLibrary();
      });
    },
  };

  const lightboxHandlers = {
    updateTitle: async (id: string, title: string) => {
      await safeMutate(async () => {
        gallery.setItemDetail(await vaultApi.setItemTitle(id, title));
        await refreshLibrary();
      });
    },
    updateRating: async (id: string, rating: number) => {
      await safeMutate(async () => {
        gallery.setItemDetail(await vaultApi.setItemRating(id, rating));
        await refreshLibrary();
      });
    },
    updateMemo: async (id: string, memo: string) => {
      await safeMutate(async () => {
        gallery.setItemDetail(await vaultApi.setItemMemo(id, memo));
        await refreshLibrary();
      });
    },
    updateSourceUrl: async (id: string, sourceUrl: string) => {
      await safeMutate(async () => {
        gallery.setItemDetail(await vaultApi.setItemSourceUrl(id, sourceUrl));
        await refreshLibrary();
      });
    },
    addTag: async (id: string, tag: string) => {
      await safeMutate(async () => {
        gallery.setItemDetail(await vaultApi.addItemTag(id, tag));
        await refreshLibrary();
      });
    },
    removeTag: async (id: string, tag: string) => {
      await safeMutate(async () => {
        gallery.setItemDetail(await vaultApi.removeItemTag(id, tag));
        await refreshLibrary();
      });
    },
    addToFolder: async (fid: string, id: string) => {
      await safeMutate(async () => {
        await vaultApi.addEntriesToFolder(fid, [{ kind: "image", id }]);
        gallery.setItemDetail(await vaultApi.getItemDetail(id));
        await refreshLibrary();
      });
    },
    removeFromFolder: async (fid: string, id: string) => {
      await safeMutate(async () => {
        await vaultApi.removeEntriesFromFolder(fid, [{ kind: "image", id }]);
        gallery.setItemDetail(await vaultApi.getItemDetail(id));
        await refreshLibrary();
      });
    },
    moveToTrash: async (id: string) => {
      await safeMutate(async () => {
        await vaultApi.moveEntriesToTrash([{ kind: "image", id }]);
        gallery.setLightboxId(null);
        pushToast("info", "ゴミ箱へ移動しました");
        await refreshLibrary();
      });
    },
    restoreFromTrash: async (id: string) => {
      await safeMutate(async () => {
        await vaultApi.restoreEntriesFromTrash([{ kind: "image", id }]);
        gallery.setLightboxId(null);
        pushToast("success", "復元しました");
        await refreshLibrary();
      });
    },
  };

  const finalizeImportedGroupPages = async (
    gid: string,
    report: ImportReport,
    preSkippedVideos = 0
  ) => {
    const videoIdSet = new Set(report.videoPendingIds);
    const imageIds = report.importedIds.filter((id) => !videoIdSet.has(id));
    const totalSkippedVideos = preSkippedVideos + (report.importedIds.length - imageIds.length);
    if (imageIds.length > 0) {
      const updated = await vaultApi.addItemsToGroup(gid, imageIds);
      gallery.setActiveModalGroup(updated);
      if (gallery.groupDetail?.id === gid) gallery.setGroupDetail(updated);
      pushToast("success", `${imageIds.length}ページを追加しました`);
    } else if (report.duplicateCount > 0) {
      pushToast(
        "info",
        `${report.duplicateCount}件はすでに保管庫に登録済みのためスキップしました`
      );
    } else if (report.skippedUnsupportedCount > 0) {
      pushToast("warning", "対応していないファイル形式のため追加できませんでした");
    }
    if (totalSkippedVideos > 0) {
      pushToast(
        "warning",
        `動画 ${totalSkippedVideos}件は漫画ページに追加できないため除外しました`
      );
    }
    if (report.errors.length > 0) {
      pushToast("error", report.errors[0]);
    }
    await refreshLibrary();
    processPendingThumbnails(report.avifPendingIds, report.videoPendingIds);
  };

  const readerHandlers = {
    updateReaderPrefs: async (
      gid: string,
      readingDir: "rtl" | "ltr",
      spreadMode: "single" | "spread",
      coverAlone: boolean
    ) => {
      await safeMutate(async () => {
        const updated = await vaultApi.setGroupReaderPrefs(
          gid,
          readingDir,
          spreadMode,
          coverAlone
        );
        gallery.setActiveModalGroup(updated);
        if (gallery.groupDetail?.id === gid) gallery.setGroupDetail(updated);
      });
    },
    updateTitle: async (gid: string, title: string) => {
      await safeMutate(async () => {
        gallery.setActiveModalGroup(await vaultApi.setGroupTitle(gid, title));
        gallery.refreshSelectedDetail();
        await refreshLibrary();
      });
    },
    updateRating: async (gid: string, rating: number) => {
      await safeMutate(async () => {
        gallery.setActiveModalGroup(await vaultApi.setGroupRating(gid, rating));
        gallery.refreshSelectedDetail();
        await refreshLibrary();
      });
    },
    updateMemo: async (gid: string, memo: string) => {
      await safeMutate(async () => {
        gallery.setActiveModalGroup(await vaultApi.setGroupMemo(gid, memo));
        gallery.refreshSelectedDetail();
        await refreshLibrary();
      });
    },
    addTag: async (gid: string, tag: string) => {
      await safeMutate(async () => {
        gallery.setActiveModalGroup(await vaultApi.addGroupTag(gid, tag));
        gallery.refreshSelectedDetail();
        await refreshLibrary();
      });
    },
    removeTag: async (gid: string, tag: string) => {
      await safeMutate(async () => {
        gallery.setActiveModalGroup(await vaultApi.removeGroupTag(gid, tag));
        gallery.refreshSelectedDetail();
        await refreshLibrary();
      });
    },
    addToFolder: async (fid: string, gid: string) => {
      await safeMutate(async () => {
        await vaultApi.addEntriesToFolder(fid, [{ kind: "group", id: gid }]);
        gallery.setActiveModalGroup(await vaultApi.getGroupDetail(gid));
        gallery.refreshSelectedDetail();
        await refreshLibrary();
      });
    },
    removeFromFolder: async (fid: string, gid: string) => {
      await safeMutate(async () => {
        await vaultApi.removeEntriesFromFolder(fid, [{ kind: "group", id: gid }]);
        gallery.setActiveModalGroup(await vaultApi.getGroupDetail(gid));
        gallery.refreshSelectedDetail();
        await refreshLibrary();
      });
    },
    ungroup: async (gid: string) => {
      await safeMutate(async () => {
        await vaultApi.ungroup(gid);
        gallery.setReaderGroupId(null);
        pushToast("info", "グループを解除しました");
        await refreshLibrary();
      });
    },
    moveToTrash: async (gid: string) => {
      await safeMutate(async () => {
        await vaultApi.moveEntriesToTrash([{ kind: "group", id: gid }]);
        gallery.setReaderGroupId(null);
        pushToast("info", "ゴミ箱へ移動しました");
        await refreshLibrary();
      });
    },
    restoreFromTrash: async (gid: string) => {
      await safeMutate(async () => {
        await vaultApi.restoreEntriesFromTrash([{ kind: "group", id: gid }]);
        gallery.setReaderGroupId(null);
        pushToast("success", "復元しました");
        await refreshLibrary();
      });
    },
  };

  const editorHandlers = {
    reorderPages: async (gid: string, orderedIds: string[]) => {
      try {
        const updated = await vaultApi.reorderGroupPages(gid, orderedIds);
        gallery.setActiveModalGroup(updated);
        if (gallery.groupDetail?.id === gid) gallery.setGroupDetail(updated);
        await refreshLibrary();
      } catch (err) {
        pushToast("error", formatErrorMessage(err));
        throw err;
      }
    },
    removePage: async (gid: string, itemId: string) => {
      try {
        const next = await vaultApi.removePageFromGroup(gid, itemId);
        if (!next) {
          gallery.setEditorGroupId(null);
          gallery.setReaderGroupId(null);
          pushToast("info", "最後のページを外したためグループを解除しました");
        } else {
          gallery.setActiveModalGroup(next);
          if (gallery.groupDetail?.id === gid) gallery.setGroupDetail(next);
        }
        await refreshLibrary();
      } catch (err) {
        pushToast("error", formatErrorMessage(err));
        throw err;
      }
    },
    addFilesToGroup: async (gid: string, paths: string[]) => {
      await safeMutate(async () => {
        const report = await vaultApi.importPaths(paths, null);
        await finalizeImportedGroupPages(gid, report);
      });
    },
    addBrowserFilesToGroup: async (gid: string, files: File[]) => {
      await safeMutate(async () => {
        const imageFiles = files.filter((f) => !isVideoMime(f.type));
        const preSkippedVideos = files.length - imageFiles.length;
        if (imageFiles.length === 0) {
          if (preSkippedVideos > 0) {
            pushToast("warning", "動画ファイルは漫画グループのページに追加できません");
          }
          return;
        }
        const report = await uploadBrowserFiles(imageFiles, null);
        await finalizeImportedGroupPages(gid, report, preSkippedVideos);
      });
    },
  };

  return {
    batchRatingBusy,
    applyBatchTagUpdate,
    applyBatchRatingUpdate,
    triggerBatchSetRating,
    resetMutationState,
    toggleClipboardWatch,
    folderHandlers,
    dragDropHandlers,
    gridBulkHandlers,
    inspectorHandlers,
    lightboxHandlers,
    readerHandlers,
    editorHandlers,
  };
}
