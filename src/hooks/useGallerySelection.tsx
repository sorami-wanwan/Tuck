import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent,
} from "react";
import {
  Archive,
  BookOpen,
  Download,
  Folder,
  FolderMinus,
  FolderPlus,
  Layers,
  ListOrdered,
  Maximize2,
  Play,
  RotateCcw,
  Star,
  StarOff,
  Tags,
  Trash2,
  Unlink,
} from "lucide-react";
import type { ContextMenuItem } from "../components/ContextMenu";
import { imageMemoryCache } from "../services/imageCache";
import { formatErrorMessage, vaultApi } from "../services/vaultApi";
import { summarizeSelectionRating } from "../utils/bulkRating";
import { sortImagesOldestFirst } from "../utils/groupOrder";
import {
  entryKey,
  isVideoMime,
  parseEntryKey,
  type EntryRef,
  type FolderNode,
  type GalleryEntry,
  type GroupDetail,
  type ItemDetail,
  type PlainExportTarget,
  type ToastItem,
} from "../types/vault";

interface UseGallerySelectionOptions {
  isOpen: boolean;
  entries: GalleryEntry[];
  folders: FolderNode[];
  currentFolderId: string | null;
  isTrashView: boolean;
  canEdit: boolean;
  isWebClient: boolean;
  mobileSelectMode: boolean;
  refreshLibrary: () => Promise<void>;
  onOpenExportPackageForEntries: (entries: EntryRef[]) => void;
  onPlainExport: (target: PlainExportTarget) => void;
  onOpenBulkTagDialog: () => void;
  onBatchSetRating: (entries: EntryRef[], rating: number) => Promise<void>;
  pushToast: (tone: ToastItem["tone"], message: string) => void;
}

/**
 * サムネイル選択・詳細データ取得・拡大表示 / 漫画リーダー / グループ編集モーダル・右クリックメニュー・完全削除を管理するフック。
 */
export function useGallerySelection({
  isOpen,
  entries,
  folders,
  currentFolderId,
  isTrashView,
  canEdit,
  isWebClient,
  mobileSelectMode,
  refreshLibrary,
  onOpenExportPackageForEntries,
  onPlainExport,
  onOpenBulkTagDialog,
  onBatchSetRating,
  pushToast,
}: UseGallerySelectionOptions) {
  const [selectedKeys, setSelectedKeys] = useState<Set<string>>(() => new Set());
  const selectedKeysRef = useRef<Set<string>>(selectedKeys);
  selectedKeysRef.current = selectedKeys;
  const [primaryKey, setPrimaryKey] = useState<string | null>(null);
  const primaryKeyRef = useRef<string | null>(primaryKey);
  primaryKeyRef.current = primaryKey;
  const anchorKeyRef = useRef<string | null>(null);
  const [detailNonce, setDetailNonce] = useState(0);

  const [itemDetail, setItemDetail] = useState<ItemDetail | null>(null);
  const [groupDetail, setGroupDetail] = useState<GroupDetail | null>(null);
  const [lightboxId, setLightboxId] = useState<string | null>(null);
  const [readerGroupId, setReaderGroupId] = useState<string | null>(null);
  const [editorGroupId, setEditorGroupId] = useState<string | null>(null);
  const [activeModalGroup, setActiveModalGroup] = useState<GroupDetail | null>(null);

  const [cardMenu, setCardMenu] = useState<{ x: number; y: number } | null>(null);
  const [pendingDeleteEntries, setPendingDeleteEntries] = useState<EntryRef[] | null>(null);
  const permDeleteDialogRef = useRef<HTMLDialogElement>(null);

  const refreshSelectedDetail = useCallback(() => {
    setDetailNonce((n) => n + 1);
  }, []);

  const selectSingleKey = useCallback((key: string) => {
    const next = new Set([key]);
    selectedKeysRef.current = next;
    primaryKeyRef.current = key;
    setSelectedKeys(next);
    setPrimaryKey(key);
    anchorKeyRef.current = key;
  }, []);

  const selectAllEntries = useCallback(() => {
    const all = new Set(entries.map((e) => entryKey({ kind: e.kind, id: e.id })));
    const firstKey = entries[0] ? entryKey({ kind: entries[0].kind, id: entries[0].id }) : null;
    selectedKeysRef.current = all;
    primaryKeyRef.current = firstKey;
    setSelectedKeys(all);
    setPrimaryKey(firstKey);
  }, [entries]);

  const clearSelection = useCallback(() => {
    const empty = new Set<string>();
    selectedKeysRef.current = empty;
    primaryKeyRef.current = null;
    anchorKeyRef.current = null;
    setSelectedKeys(empty);
    setPrimaryKey(null);
  }, []);

  const openCardContextMenu = useCallback(
    (entry: GalleryEntry, x: number, y: number) => {
      const k = entryKey({ kind: entry.kind, id: entry.id });
      if (!selectedKeys.has(k)) {
        selectSingleKey(k);
      }
      setCardMenu({ x, y });
    },
    [selectedKeys, selectSingleKey]
  );

  const clearSelectionState = useCallback(() => {
    const empty = new Set<string>();
    selectedKeysRef.current = empty;
    primaryKeyRef.current = null;
    setSelectedKeys(empty);
    setPrimaryKey(null);
    anchorKeyRef.current = null;
    setItemDetail(null);
    setGroupDetail(null);
    setLightboxId(null);
    setReaderGroupId(null);
    setEditorGroupId(null);
    setActiveModalGroup(null);
    setCardMenu(null);
    setPendingDeleteEntries(null);
  }, []);

  const syncSelectionWithEntries = useCallback((nextEntries: GalleryEntry[]) => {
    const validKeys = new Set(nextEntries.map((e) => entryKey({ kind: e.kind, id: e.id })));
    const prev = selectedKeysRef.current;
    const filtered = new Set<string>();
    for (const k of prev) {
      if (validKeys.has(k)) filtered.add(k);
    }
    if (prev.size > 0 && filtered.size === 0 && nextEntries.length > 0) {
      filtered.add(entryKey({ kind: nextEntries[0].kind, id: nextEntries[0].id }));
    }

    const unchanged =
      filtered.size === prev.size && Array.from(filtered).every((k) => prev.has(k));
    if (!unchanged) {
      selectedKeysRef.current = filtered;
      setSelectedKeys(filtered);
    }

    const prevPrimary = primaryKeyRef.current;
    const nextPrimary =
      filtered.size === 0
        ? null
        : prevPrimary && filtered.has(prevPrimary)
          ? prevPrimary
          : (Array.from(filtered)[0] ?? null);
    if (nextPrimary !== prevPrimary) {
      primaryKeyRef.current = nextPrimary;
      setPrimaryKey(nextPrimary);
    }
    if (anchorKeyRef.current && !filtered.has(anchorKeyRef.current)) {
      anchorKeyRef.current = nextPrimary;
    }
  }, []);

  // Load Inspector detail for single selection
  useEffect(() => {
    if (!isOpen || selectedKeys.size !== 1 || !primaryKey) {
      setItemDetail(null);
      setGroupDetail(null);
      return;
    }
    const ref = parseEntryKey(primaryKey);
    let active = true;
    if (ref.kind === "image") {
      setGroupDetail(null);
      void vaultApi
        .getItemDetail(ref.id)
        .then((d) => {
          if (active) setItemDetail(d);
        })
        .catch(() => {
          if (active) setItemDetail(null);
        });
    } else {
      setItemDetail(null);
      void vaultApi
        .getGroupDetail(ref.id)
        .then((g) => {
          if (active) setGroupDetail(g);
        })
        .catch(() => {
          if (active) setGroupDetail(null);
        });
    }
    return () => {
      active = false;
    };
  }, [isOpen, selectedKeys, primaryKey, detailNonce]);

  // Keep active modal group synced when reader or editor open
  const modalGroupTargetId = editorGroupId ?? readerGroupId;
  useEffect(() => {
    if (!isOpen || !modalGroupTargetId) {
      setActiveModalGroup(null);
      return;
    }
    let active = true;
    void vaultApi
      .getGroupDetail(modalGroupTargetId)
      .then((g) => {
        if (active) setActiveModalGroup(g);
      })
      .catch(() => {
        if (active) {
          setActiveModalGroup(null);
          setReaderGroupId(null);
          setEditorGroupId(null);
        }
      });
    return () => {
      active = false;
    };
  }, [isOpen, modalGroupTargetId, entries, detailNonce]);

  const selectedEntryRefs = useMemo(
    () => Array.from(selectedKeys).map(parseEntryKey),
    [selectedKeys]
  );

  const handleCardClick = useCallback(
    (entry: GalleryEntry, e: MouseEvent) => {
      const key = entryKey({ kind: entry.kind, id: entry.id });
      if (e.metaKey || e.ctrlKey || mobileSelectMode) {
        const next = new Set(selectedKeysRef.current);
        const removed = next.delete(key);
        if (!removed) {
          next.add(key);
        }
        selectedKeysRef.current = next;
        setSelectedKeys(next);

        if (!removed) {
          primaryKeyRef.current = key;
          setPrimaryKey(key);
          anchorKeyRef.current = key;
        } else {
          const remaining = Array.from(next);
          const curPrimary = primaryKeyRef.current;
          const nextPrimary =
            curPrimary && next.has(curPrimary)
              ? curPrimary
              : (remaining[remaining.length - 1] ?? null);
          primaryKeyRef.current = nextPrimary;
          setPrimaryKey(nextPrimary);
          if (anchorKeyRef.current === key) {
            anchorKeyRef.current = nextPrimary;
          }
        }
        return;
      }

      if (e.shiftKey && anchorKeyRef.current) {
        const aIdx = entries.findIndex(
          (x) => entryKey({ kind: x.kind, id: x.id }) === anchorKeyRef.current
        );
        const bIdx = entries.findIndex((x) => entryKey({ kind: x.kind, id: x.id }) === key);
        if (aIdx >= 0 && bIdx >= 0) {
          const lo = Math.min(aIdx, bIdx);
          const hi = Math.max(aIdx, bIdx);
          const rangeKeys = new Set<string>();
          for (let i = lo; i <= hi; i++) {
            rangeKeys.add(entryKey({ kind: entries[i].kind, id: entries[i].id }));
          }
          selectedKeysRef.current = rangeKeys;
          primaryKeyRef.current = key;
          setSelectedKeys(rangeKeys);
          setPrimaryKey(key);
          return;
        }
      }

      selectSingleKey(key);
    },
    [entries, mobileSelectMode, selectSingleKey]
  );

  const handleOpenEntry = useCallback((entry: GalleryEntry) => {
    const key = entryKey({ kind: entry.kind, id: entry.id });
    setSelectedKeys(new Set([key]));
    setPrimaryKey(key);
    if (entry.kind === "group") {
      setReaderGroupId(entry.id);
    } else {
      setLightboxId(entry.id);
    }
  }, []);

  const entryMap = useMemo(
    () => new Map(entries.map((e) => [entryKey({ kind: e.kind, id: e.id }), e])),
    [entries]
  );

  const selectedEntriesMeta = useMemo(
    () =>
      selectedEntryRefs
        .map((r) => entryMap.get(entryKey(r)))
        .filter((e): e is GalleryEntry => Boolean(e)),
    [selectedEntryRefs, entryMap]
  );

  const handleCreateGroupFromSelection = useCallback(async () => {
    const itemIds = sortImagesOldestFirst(selectedEntriesMeta).map((e) => e.id);
    if (itemIds.length < 2) return;
    try {
      const created = await vaultApi.createGroupFromItems(itemIds, null, currentFolderId);
      const gk = entryKey({ kind: "group", id: created.id });
      setSelectedKeys(new Set([gk]));
      setPrimaryKey(gk);
      pushToast("success", `${created.pageCount}ページを漫画グループにまとめました`);
      await refreshLibrary();
    } catch (err) {
      pushToast("error", formatErrorMessage(err));
    }
  }, [selectedEntriesMeta, currentFolderId, pushToast, refreshLibrary]);

  const requestPermanentDelete = useCallback((refs: EntryRef[]) => {
    setPendingDeleteEntries(refs);
    permDeleteDialogRef.current?.showModal();
  }, []);

  const executePermanentDelete = useCallback(
    async (refs: EntryRef[]) => {
      try {
        const idsToInvalidate = new Set<string>();
        const groupFetchPromises: Array<Promise<string[]>> = [];
        for (const r of refs) {
          idsToInvalidate.add(r.id);
          const entryMeta = entries.find((e) => e.kind === r.kind && e.id === r.id);
          if (entryMeta?.coverItemId) {
            idsToInvalidate.add(entryMeta.coverItemId);
          }
          if (r.kind === "group") {
            const knownGroup =
              groupDetail?.id === r.id
                ? groupDetail
                : activeModalGroup?.id === r.id
                  ? activeModalGroup
                  : null;
            if (knownGroup) {
              for (const p of knownGroup.pages) idsToInvalidate.add(p.itemId);
            } else {
              groupFetchPromises.push(
                vaultApi
                  .getGroupDetail(r.id)
                  .then((g) => g.pages.map((p) => p.itemId))
                  .catch(() => [])
              );
            }
          }
        }
        if (groupFetchPromises.length > 0) {
          const pageLists = await Promise.all(groupFetchPromises);
          for (const pageIds of pageLists) {
            for (const pid of pageIds) idsToInvalidate.add(pid);
          }
        }

        await vaultApi.permanentlyDeleteEntries(refs);
        const invalidList = Array.from(idsToInvalidate);
        imageMemoryCache.invalidateItems(invalidList);
        if (lightboxId && idsToInvalidate.has(lightboxId)) setLightboxId(null);
        if (readerGroupId && idsToInvalidate.has(readerGroupId)) setReaderGroupId(null);
        if (editorGroupId && idsToInvalidate.has(editorGroupId)) setEditorGroupId(null);
        pushToast("info", `${refs.length}件を完全に削除しました`);
        await refreshLibrary();
      } catch (err) {
        pushToast("error", formatErrorMessage(err));
      }
    },
    [
      entries,
      groupDetail,
      activeModalGroup,
      lightboxId,
      readerGroupId,
      editorGroupId,
      pushToast,
      refreshLibrary,
    ]
  );

  const inspectorMode: "empty" | "image" | "group" | "multi" =
    selectedKeys.size >= 2
      ? "multi"
      : selectedKeys.size === 1 && primaryKey
        ? parseEntryKey(primaryKey).kind === "group"
          ? groupDetail
            ? "group"
            : "empty"
          : itemDetail
            ? "image"
            : "empty"
        : "empty";

  const standaloneSelectedCount = selectedEntriesMeta.filter(
    (e) => e.kind === "image" && !isVideoMime(e.coverMimeType)
  ).length;
  const singleSelectedEntry =
    selectedEntryRefs.length === 1 ? entryMap.get(entryKey(selectedEntryRefs[0])) : undefined;
  const singleSelectedGroupId =
    selectedEntryRefs.length === 1 && selectedEntryRefs[0].kind === "group"
      ? selectedEntryRefs[0].id
      : null;

  const cardContextItems: ContextMenuItem[] = useMemo(() => {
    if (!cardMenu || selectedEntryRefs.length === 0) return [];

    if (isTrashView) {
      if (!canEdit) return [];
      return [
        {
          id: "restore",
          label: "元の場所へ復元",
          icon: <RotateCcw size={13} />,
          onClick: () => {
            void vaultApi
              .restoreEntriesFromTrash(selectedEntryRefs)
              .then(() => {
                pushToast("success", `${selectedEntryRefs.length}件を復元しました`);
                return refreshLibrary();
              })
              .catch((err) => pushToast("error", formatErrorMessage(err)));
          },
        },
        {
          id: "perm-del",
          label: "完全に削除",
          icon: <Trash2 size={13} />,
          danger: true,
          separatorBefore: true,
          onClick: () => requestPermanentDelete(selectedEntryRefs),
        },
      ];
    }

    const firstRef = selectedEntryRefs[0];
    const itemsList: ContextMenuItem[] = [];

    if (selectedEntryRefs.length === 1) {
      const isSingleVideo = Boolean(
        singleSelectedEntry &&
          singleSelectedEntry.kind === "image" &&
          isVideoMime(singleSelectedEntry.coverMimeType)
      );
      itemsList.push({
        id: "open",
        label:
          firstRef.kind === "group" ? "漫画を読む" : isSingleVideo ? "動画を再生" : "拡大表示",
        icon:
          firstRef.kind === "group" ? (
            <BookOpen size={13} />
          ) : isSingleVideo ? (
            <Play size={13} />
          ) : (
            <Maximize2 size={13} />
          ),
        onClick: () => {
          if (firstRef.kind === "group") setReaderGroupId(firstRef.id);
          else setLightboxId(firstRef.id);
        },
      });
    }

    if (selectedEntryRefs.length === 1) {
      const target: PlainExportTarget =
        firstRef.kind === "group"
          ? {
              kind: "group",
              groupId: firstRef.id,
              title: singleSelectedEntry?.title ?? "",
              pageCount: singleSelectedEntry?.pageCount,
            }
          : {
              kind: "item",
              itemId: firstRef.id,
              title: singleSelectedEntry?.title ?? "",
              mimeType: singleSelectedEntry?.coverMimeType ?? "image/png",
            };
      itemsList.push({
        id: "plain-export",
        label:
          firstRef.kind === "group"
            ? isWebClient
              ? "ZIP でダウンロード"
              : "フォルダとして書き出し..."
            : isWebClient
              ? "ダウンロード"
              : "ファイルとして書き出し...",
        icon: <Download size={13} />,
        onClick: () => onPlainExport(target),
      });
    } else {
      const imageCount = selectedEntryRefs.filter((r) => r.kind === "image").length;
      const groupCount = selectedEntryRefs.length - imageCount;
      itemsList.push({
        id: "plain-export",
        label: isWebClient
          ? "選択項目を ZIP でダウンロード"
          : "選択項目をファイルとして書き出し...",
        icon: <Download size={13} />,
        onClick: () =>
          onPlainExport({
            kind: "entries",
            entries: [...selectedEntryRefs],
            imageCount,
            groupCount,
          }),
      });
    }

    if (!canEdit) {
      return itemsList;
    }

    itemsList.push({
      id: "add-folder",
      label: "フォルダに追加...",
      icon: <FolderPlus size={13} />,
      children:
        folders.length === 0
          ? [{ id: "no-folders", label: "フォルダがありません", disabled: true }]
          : folders.map((f) => ({
              id: `f-${f.id}`,
              label: f.name,
              icon: <Folder size={12} style={{ color: f.color || "var(--text-secondary)" }} />,
              onClick: () => {
                void vaultApi
                  .addEntriesToFolder(f.id, selectedEntryRefs)
                  .then(() => {
                    pushToast("success", `「${f.name}」に追加しました`);
                    return refreshLibrary();
                  })
                  .catch((err) => pushToast("error", formatErrorMessage(err)));
              },
            })),
    });

    itemsList.push({
      id: "edit-tags",
      label: selectedEntryRefs.length >= 2 ? "タグを一括編集..." : "タグを編集...",
      icon: <Tags size={13} />,
      onClick: onOpenBulkTagDialog,
    });

    const ratingSummary = summarizeSelectionRating(selectedEntriesMeta);
    itemsList.push({
      id: "set-rating",
      label: "評価を設定...",
      icon: <Star size={13} />,
      children: [
        ...[5, 4, 3, 2, 1].map((star) => ({
          id: `rating-${star}`,
          label: `${"★".repeat(star)}${"☆".repeat(5 - star)} (${star})`,
          icon: (
            <Star
              size={12}
              fill="currentColor"
              style={{ color: "var(--star-gold)", flexShrink: 0 }}
            />
          ),
          checked: ratingSummary.uniformRating === star,
          onClick: () => void onBatchSetRating([...selectedEntryRefs], star),
        })),
        {
          id: "rating-clear",
          label: "評価を解除",
          icon: <StarOff size={12} style={{ flexShrink: 0 }} />,
          separatorBefore: true,
          disabled: ratingSummary.ratedCount === 0,
          onClick: () => void onBatchSetRating([...selectedEntryRefs], 0),
        },
      ],
    });

    if (standaloneSelectedCount >= 2) {
      itemsList.push({
        id: "bundle",
        label: `漫画グループにまとめる (${standaloneSelectedCount}枚)`,
        icon: <Layers size={13} />,
        onClick: () => void handleCreateGroupFromSelection(),
      });
    }

    if (singleSelectedGroupId) {
      itemsList.push(
        {
          id: "edit-pages",
          label: "ページ構成を編集",
          icon: <ListOrdered size={13} />,
          onClick: () => setEditorGroupId(singleSelectedGroupId),
        },
        {
          id: "ungroup",
          label: "グループを解除（単体画像に戻す）",
          icon: <Unlink size={13} />,
          onClick: () => {
            void vaultApi
              .ungroup(singleSelectedGroupId)
              .then(() => {
                pushToast("info", "グループを解除して単体画像に戻しました");
                return refreshLibrary();
              })
              .catch((err) => pushToast("error", formatErrorMessage(err)));
          },
        }
      );
    }

    if (currentFolderId) {
      itemsList.push({
        id: "remove-folder",
        label: "このフォルダから外す（本体は残る）",
        icon: <FolderMinus size={13} />,
        separatorBefore: true,
        onClick: () => {
          void vaultApi
            .removeEntriesFromFolder(currentFolderId, selectedEntryRefs)
            .then(() => {
              pushToast("info", "このフォルダから外しました（本体は保管庫に残ります）");
              return refreshLibrary();
            })
            .catch((err) => pushToast("error", formatErrorMessage(err)));
        },
      });
    }

    if (!isWebClient) {
      itemsList.push({
        id: "export-pkg",
        label: "選択項目をパッケージに書き出し...",
        icon: <Archive size={13} />,
        separatorBefore: true,
        onClick: () => onOpenExportPackageForEntries([...selectedEntryRefs]),
      });
    }

    itemsList.push({
      id: "trash",
      label: "ゴミ箱へ移動",
      icon: <Trash2 size={13} />,
      danger: true,
      separatorBefore: true,
      onClick: () => {
        void vaultApi
          .moveEntriesToTrash(selectedEntryRefs)
          .then(() => {
            pushToast("info", `${selectedEntryRefs.length}件をゴミ箱へ移動しました`);
            return refreshLibrary();
          })
          .catch((err) => pushToast("error", formatErrorMessage(err)));
      },
    });

    return itemsList;
  }, [
    cardMenu,
    canEdit,
    isWebClient,
    selectedEntryRefs,
    selectedEntriesMeta,
    singleSelectedEntry,
    isTrashView,
    folders,
    standaloneSelectedCount,
    singleSelectedGroupId,
    currentFolderId,
    pushToast,
    refreshLibrary,
    handleCreateGroupFromSelection,
    requestPermanentDelete,
    onOpenExportPackageForEntries,
    onPlainExport,
    onOpenBulkTagDialog,
    onBatchSetRating,
  ]);

  return {
    selectedKeys,
    setSelectedKeys,
    primaryKey,
    setPrimaryKey,
    anchorKeyRef,
    selectSingleKey,
    selectAllEntries,
    clearSelection,
    openCardContextMenu,
    clearSelectionState,
    syncSelectionWithEntries,
    refreshSelectedDetail,
    itemDetail,
    setItemDetail,
    groupDetail,
    setGroupDetail,
    lightboxId,
    setLightboxId,
    readerGroupId,
    setReaderGroupId,
    editorGroupId,
    setEditorGroupId,
    activeModalGroup,
    setActiveModalGroup,
    cardMenu,
    setCardMenu,
    pendingDeleteEntries,
    setPendingDeleteEntries,
    permDeleteDialogRef,
    selectedEntryRefs,
    selectedEntriesMeta,
    inspectorMode,
    handleCardClick,
    handleOpenEntry,
    handleCreateGroupFromSelection,
    executePermanentDelete,
    cardContextItems,
  };
}
