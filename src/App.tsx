import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Folder } from "lucide-react";
import { ContextMenu } from "./components/ContextMenu";
import { GroupEditorDialog } from "./components/GroupEditorDialog";
import { Inspector } from "./components/Inspector";
import { LanAuthScreen } from "./components/LanAuthScreen";
import { LanShareDialog } from "./components/LanShareDialog";
import { LightboxDialog } from "./components/LightboxDialog";
import { LockSettingsDialog } from "./components/LockSettingsDialog";
import { MangaReaderDialog } from "./components/MangaReaderDialog";
import { PlainExportConfirmDialog } from "./components/PlainExportConfirmDialog";
import { Sidebar } from "./components/Sidebar";
import { ThumbnailGrid } from "./components/ThumbnailGrid";
import { ToastRegion } from "./components/ToastRegion";
import { Toolbar } from "./components/Toolbar";
import { UrlImportDialog } from "./components/UrlImportDialog";
import { VaultGate } from "./components/VaultGate";
import { VaultLockScreen } from "./components/VaultLockScreen";
import {
  VaultMaintenanceDialog,
  type MaintenanceTab,
} from "./components/VaultMaintenanceDialog";
import { useGallerySelection } from "./hooks/useGallerySelection";
import { useImportFlow } from "./hooks/useImportFlow";
import { useInternalDrag } from "./hooks/useInternalDrag";
import { usePlainExport } from "./hooks/usePlainExport";
import { useToasts } from "./hooks/useToasts";
import { useVaultSession } from "./hooks/useVaultSession";
import { imageMemoryCache } from "./services/imageCache";
import { formatErrorMessage, vaultApi } from "./services/vaultApi";
import {
  entryKey,
  type EntryRef,
  type FolderNode,
  type GalleryEntry,
  type LockSettings,
  type QueryScope,
  type SearchQuery,
  type SortBy,
  type TagWithCount,
  type ThumbFitMode,
  type UrlImportSettings,
  type ViewSelection,
} from "./types/vault";

const INITIAL_VIEW: ViewSelection = { kind: "all" };

export function App() {
  const { toasts, pushToast, dismissToast } = useToasts();

  const refreshLibraryRef = useRef<() => Promise<void>>(async () => {});
  const resetWorkspaceRef = useRef<() => void>(() => {});
  const readOnlyEnforcedRef = useRef<() => void>(() => {});
  const syncSelectionRef = useRef<(nextEntries: GalleryEntry[]) => void>(() => {});

  const session = useVaultSession({
    pushToast,
    refreshLibraryRef,
    onResetWorkspaceState: () => resetWorkspaceRef.current(),
    onReadOnlyEnforced: () => readOnlyEnforcedRef.current(),
  });
  const { isWebClient, status, setStatus, canEdit } = session;
  const plainExport = usePlainExport({ isWebClient, status, setStatus, pushToast });
  const { requestPlainExport, resetPlainExportState } = plainExport;

  const [isCompactViewport, setIsCompactViewport] = useState(() =>
    typeof window !== "undefined" ? window.matchMedia("(max-width: 1023px)").matches : false
  );
  const [isMobileViewport, setIsMobileViewport] = useState(() =>
    typeof window !== "undefined" ? window.matchMedia("(max-width: 767px)").matches : false
  );
  const [isTouchDevice, setIsTouchDevice] = useState(() =>
    typeof window !== "undefined" ? window.matchMedia("(pointer: coarse)").matches : false
  );
  const [mobileDrawerOpen, setMobileDrawerOpen] = useState(false);
  const [mobileSelectMode, setMobileSelectMode] = useState(false);

  const [entries, setEntries] = useState<GalleryEntry[]>([]);
  const [folders, setFolders] = useState<FolderNode[]>([]);
  const [tags, setTags] = useState<TagWithCount[]>([]);

  const [lockSettingsOpen, setLockSettingsOpen] = useState(false);
  const [maintenanceModal, setMaintenanceModal] = useState<{
    open: boolean;
    initialTab: MaintenanceTab;
    preselectedFolderIds: string[];
    preselectedEntries: EntryRef[];
  }>({
    open: false,
    initialTab: "maintenance",
    preselectedFolderIds: [],
    preselectedEntries: [],
  });

  const [history, setHistory] = useState<ViewSelection[]>([INITIAL_VIEW]);
  const [historyIndex, setHistoryIndex] = useState(0);
  const view = history[historyIndex] ?? INITIAL_VIEW;

  const [sidebarFilter, setSidebarFilter] = useState("");
  const [searchText, setSearchText] = useState("");
  const [sortBy, setSortBy] = useState<SortBy>("createdDesc");
  const [minRating, setMinRating] = useState<number | null>(null);
  const [mimeTypes, setMimeTypes] = useState<string[]>([]);
  const [filterOpen, setFilterOpen] = useState(false);

  const [thumbSize, setThumbSize] = useState(210);
  const [fitMode, setFitMode] = useState<ThumbFitMode>("ratio");
  const [inspectorOpen, setInspectorOpen] = useState(() =>
    typeof window !== "undefined" ? !window.matchMedia("(max-width: 1023px)").matches : true
  );

  const currentFolderId = view.kind === "folder" ? view.folderId : null;
  const isTrashView = view.kind === "trash";

  const currentQuery: SearchQuery = useMemo(() => {
    const scope: QueryScope = view.kind === "tags" ? { kind: "all" } : view;
    return {
      scope,
      tagNames: view.kind === "tags" ? view.tags : [],
      minRating,
      mimeTypes,
      text: searchText,
      sortBy,
    };
  }, [view, minRating, mimeTypes, searchText, sortBy]);

  const refreshLibrary = useCallback(async () => {
    if (!status.isOpen) return;
    try {
      const [nextStatus, nextFolders, nextTags, nextEntries] = await Promise.all([
        vaultApi.getVaultStatus(),
        vaultApi.listFolders(),
        vaultApi.listTags(),
        vaultApi.queryEntries(currentQuery),
      ]);
      setStatus(nextStatus);
      setFolders(nextFolders);
      setTags(nextTags);
      setEntries(nextEntries);
      syncSelectionRef.current(nextEntries);
    } catch (err) {
      pushToast("error", formatErrorMessage(err));
    }
  }, [status.isOpen, currentQuery, setStatus, pushToast]);

  useEffect(() => {
    if (!status.isOpen) return;
    void refreshLibrary();
  }, [status.isOpen, refreshLibrary]);

  refreshLibraryRef.current = refreshLibrary;
  const triggerRefreshLibrary = useCallback(() => refreshLibraryRef.current(), []);

  const pendingThumbRefreshTimerRef = useRef<number | null>(null);
  useEffect(() => {
    return () => {
      if (pendingThumbRefreshTimerRef.current !== null) {
        window.clearTimeout(pendingThumbRefreshTimerRef.current);
        pendingThumbRefreshTimerRef.current = null;
      }
    };
  }, []);

  const handlePendingThumbCompleted = useCallback(() => {
    if (pendingThumbRefreshTimerRef.current !== null) {
      window.clearTimeout(pendingThumbRefreshTimerRef.current);
    }
    pendingThumbRefreshTimerRef.current = window.setTimeout(() => {
      pendingThumbRefreshTimerRef.current = null;
      void refreshLibraryRef.current();
    }, 250);
  }, []);

  const handleOpenExportPackageForEntries = useCallback((selectedEntries: EntryRef[]) => {
    setMaintenanceModal({
      open: true,
      initialTab: "export",
      preselectedFolderIds: [],
      preselectedEntries: selectedEntries,
    });
  }, []);

  const gallery = useGallerySelection({
    isOpen: status.isOpen,
    entries,
    folders,
    currentFolderId,
    isTrashView,
    canEdit,
    isWebClient,
    mobileSelectMode,
    refreshLibrary: triggerRefreshLibrary,
    onOpenExportPackageForEntries: handleOpenExportPackageForEntries,
    onPlainExport: requestPlainExport,
    pushToast,
  });
  syncSelectionRef.current = gallery.syncSelectionWithEntries;

  const handlePlainExportSelection = useCallback(() => {
    if (gallery.inspectorMode === "image" && gallery.itemDetail) {
      requestPlainExport({
        kind: "item",
        itemId: gallery.itemDetail.id,
        title: gallery.itemDetail.title,
        mimeType: gallery.itemDetail.mimeType,
      });
      return;
    }
    if (gallery.inspectorMode === "group" && gallery.groupDetail) {
      requestPlainExport({
        kind: "group",
        groupId: gallery.groupDetail.id,
        title: gallery.groupDetail.title,
        pageCount: gallery.groupDetail.pageCount,
      });
      return;
    }
    if (gallery.selectedEntryRefs.length >= 2) {
      const imageCount = gallery.selectedEntryRefs.filter((r) => r.kind === "image").length;
      const groupCount = gallery.selectedEntryRefs.length - imageCount;
      requestPlainExport({
        kind: "entries",
        entries: [...gallery.selectedEntryRefs],
        imageCount,
        groupCount,
      });
      return;
    }
    if (gallery.selectedEntryRefs.length === 1 && gallery.selectedEntriesMeta[0]) {
      const entry = gallery.selectedEntriesMeta[0];
      if (entry.kind === "group") {
        requestPlainExport({
          kind: "group",
          groupId: entry.id,
          title: entry.title,
          pageCount: entry.pageCount,
        });
      } else {
        requestPlainExport({
          kind: "item",
          itemId: entry.id,
          title: entry.title,
          mimeType: entry.coverMimeType,
        });
      }
    }
  }, [
    gallery.inspectorMode,
    gallery.itemDetail,
    gallery.groupDetail,
    gallery.selectedEntryRefs,
    gallery.selectedEntriesMeta,
    requestPlainExport,
  ]);

  const handlePlainExportFolder = useCallback(
    (folderId: string) => {
      setMobileDrawerOpen(false);
      const folder = folders.find((f) => f.id === folderId);
      requestPlainExport({
        kind: "folder",
        folderId,
        folderName: folder?.name ?? "フォルダ",
      });
    },
    [folders, requestPlainExport]
  );

  const importFlow = useImportFlow({
    isOpen: status.isOpen,
    canEdit,
    isWebClient,
    currentFolderId,
    autoUrlImportEnabled: status.urlImportSettings.enabled,
    lockSettingsOpen,
    refreshLibrary: triggerRefreshLibrary,
    onSelectImportedKey: gallery.selectSingleKey,
    pushToast,
  });

  resetWorkspaceRef.current = () => {
    if (pendingThumbRefreshTimerRef.current !== null) {
      window.clearTimeout(pendingThumbRefreshTimerRef.current);
      pendingThumbRefreshTimerRef.current = null;
    }
    setEntries([]);
    setFolders([]);
    setTags([]);
    gallery.clearSelectionState();
    resetPlainExportState();
    setLockSettingsOpen(false);
    setMobileDrawerOpen(false);
    setMobileSelectMode(false);
    importFlow.closeUrlImportDialog();
    setMaintenanceModal({
      open: false,
      initialTab: "maintenance",
      preselectedFolderIds: [],
      preselectedEntries: [],
    });
    setSearchText("");
    setSidebarFilter("");
    setMinRating(null);
    setMimeTypes([]);
    setHistory([INITIAL_VIEW]);
    setHistoryIndex(0);
  };

  readOnlyEnforcedRef.current = () => {
    setMobileSelectMode(false);
    importFlow.closeUrlImportDialog();
    gallery.setEditorGroupId(null);
  };

  // Responsive viewport & touch-device detection
  useEffect(() => {
    const compactMq = window.matchMedia("(max-width: 1023px)");
    const mobileMq = window.matchMedia("(max-width: 767px)");
    const touchMq = window.matchMedia("(pointer: coarse)");

    const sync = () => {
      setIsCompactViewport(compactMq.matches);
      setIsMobileViewport(mobileMq.matches);
      setIsTouchDevice(touchMq.matches);
      if (!mobileMq.matches) setMobileDrawerOpen(false);
    };
    sync();
    compactMq.addEventListener("change", sync);
    mobileMq.addEventListener("change", sync);
    touchMq.addEventListener("change", sync);
    return () => {
      compactMq.removeEventListener("change", sync);
      mobileMq.removeEventListener("change", sync);
      touchMq.removeEventListener("change", sync);
    };
  }, []);

  // Close mobile sidebar drawer on Escape
  useEffect(() => {
    if (!mobileDrawerOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMobileDrawerOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [mobileDrawerOpen]);

  const navigateView = useCallback(
    (next: ViewSelection) => {
      setHistory((prev) => [...prev.slice(0, historyIndex + 1), next]);
      setHistoryIndex((idx) => idx + 1);
      setMobileDrawerOpen(false);
    },
    [historyIndex]
  );

  const toggleTagInView = useCallback(
    (tagName: string) => {
      const currentTags = view.kind === "tags" ? view.tags : [];
      const exists = currentTags.some((t) => t.toLowerCase() === tagName.toLowerCase());
      const nextTags = exists
        ? currentTags.filter((t) => t.toLowerCase() !== tagName.toLowerCase())
        : [...currentTags, tagName];
      if (nextTags.length === 0) {
        navigateView({ kind: "all" });
      } else {
        navigateView({ kind: "tags", tags: nextTags });
      }
    },
    [view, navigateView]
  );

  const internalDrag = useInternalDrag({
    onDropEntriesToFolder: (refs, folderId) => {
      void vaultApi
        .addEntriesToFolder(folderId, refs)
        .then(() => {
          const fname = folders.find((f) => f.id === folderId)?.name ?? "フォルダ";
          pushToast("success", `「${fname}」へ ${refs.length}件を追加しました`);
          return refreshLibrary();
        })
        .catch((err) => pushToast("error", formatErrorMessage(err)));
    },
    onDropEntriesToTrash: (refs) => {
      void vaultApi
        .moveEntriesToTrash(refs)
        .then(() => {
          pushToast("info", `${refs.length}件をゴミ箱へ移動しました`);
          return refreshLibrary();
        })
        .catch((err) => pushToast("error", formatErrorMessage(err)));
    },
    onDropImagesToGroup: (itemIds, groupId) => {
      void vaultApi
        .addItemsToGroup(groupId, itemIds)
        .then(() => {
          pushToast("success", `${itemIds.length}枚を漫画グループへ追加しました`);
          return refreshLibrary();
        })
        .catch((err) => pushToast("error", formatErrorMessage(err)));
    },
    onDropFolderToParent: (folderId, newParentId) => {
      void vaultApi
        .moveFolder(folderId, newParentId)
        .then(() => refreshLibrary())
        .catch((err) => pushToast("error", formatErrorMessage(err)));
    },
  });

  if (isWebClient && session.lanAuthPhase !== "authenticated") {
    return (
      <>
        <LanAuthScreen
          phase={session.lanAuthPhase}
          endReason={session.lanEndReason}
          error={session.lanAuthError}
          onSubmitPin={session.handleWebPinSubmit}
          onRetryConnection={() => void session.initWebClientAuth()}
        />
        <ToastRegion toasts={toasts} onDismiss={dismissToast} />
      </>
    );
  }

  if (!status.isOpen) {
    if (isWebClient) {
      return (
        <>
          <LanAuthScreen
            phase="ended"
            endReason="ホスト側の保管庫が開かれていないため接続を終了しました。"
            error={session.lanAuthError}
            onSubmitPin={session.handleWebPinSubmit}
            onRetryConnection={() => void session.initWebClientAuth()}
          />
          <ToastRegion toasts={toasts} onDismiss={dismissToast} />
        </>
      );
    }
    return (
      <>
        {status.canUnlock ? (
          <VaultLockScreen
            onUnlocked={(nextStatus) => setStatus(nextStatus)}
            onSwitchVault={() => void session.handleCloseCompletely()}
          />
        ) : (
          <VaultGate onOpened={(nextStatus) => setStatus(nextStatus)} />
        )}
        <ToastRegion toasts={toasts} onDismiss={dismissToast} />
      </>
    );
  }

  const sidebarElement = (
    <Sidebar
      status={status}
      folders={folders}
      tags={tags}
      view={view}
      sidebarFilter={sidebarFilter}
      hoverDropTarget={internalDrag.hoverTarget}
      isWebClient={isWebClient}
      canEdit={canEdit}
      lanShareStatus={session.lanShareStatus}
      isDrawerMode={isMobileViewport}
      onCloseDrawer={() => setMobileDrawerOpen(false)}
      onOpenLanShare={() => session.setLanDialogOpen(true)}
      onLogoutLan={() => void session.handleLogoutLan()}
      onSidebarFilterChange={setSidebarFilter}
      onSelectView={navigateView}
      onToggleTag={toggleTagInView}
      onImportFiles={() => void importFlow.handleImportFiles()}
      onOpenUrlImport={() => {
        setMobileDrawerOpen(false);
        importFlow.setUrlImportDialog({ open: true, initialUrl: "", autoStart: false });
      }}
      onImportFolderAsGroup={() => void importFlow.handleImportFolderAsGroup()}
      onToggleClipboardWatch={() => {
        void vaultApi
          .setClipboardWatch(!status.clipboardWatchEnabled)
          .then((nextStatus) => {
            setStatus(nextStatus);
            pushToast(
              "info",
              nextStatus.clipboardWatchEnabled
                ? "クリップボード監視モードをオンにしました"
                : "クリップボード監視モードをオフにしました"
            );
          })
          .catch((err) => pushToast("error", formatErrorMessage(err)));
      }}
      onOpenMaintenance={() =>
        setMaintenanceModal({
          open: true,
          initialTab: "maintenance",
          preselectedFolderIds: [],
          preselectedEntries: [],
        })
      }
      onExportFolder={(fid) =>
        setMaintenanceModal({
          open: true,
          initialTab: "export",
          preselectedFolderIds: [fid],
          preselectedEntries: [],
        })
      }
      onPlainExportFolder={handlePlainExportFolder}
      onOpenLockSettings={() => setLockSettingsOpen(true)}
      onLockNow={() => void session.performInstantLock(true)}
      onCreateFolder={async (parentId, name, color) => {
        try {
          await vaultApi.createFolder(parentId, name, color);
          await refreshLibrary();
        } catch (err) {
          pushToast("error", formatErrorMessage(err));
        }
      }}
      onRenameFolder={async (folderId, name, color) => {
        try {
          await vaultApi.renameFolder(folderId, name, color);
          await refreshLibrary();
        } catch (err) {
          pushToast("error", formatErrorMessage(err));
        }
      }}
      onMoveFolder={async (folderId, newParentId) => {
        try {
          await vaultApi.moveFolder(folderId, newParentId);
          await refreshLibrary();
        } catch (err) {
          pushToast("error", formatErrorMessage(err));
        }
      }}
      onDeleteFolder={async (folderId) => {
        try {
          await vaultApi.deleteFolder(folderId);
          if (view.kind === "folder" && view.folderId === folderId) {
            navigateView({ kind: "all" });
          }
          pushToast("info", "フォルダを削除しました（中の画像は保管庫に残ります）");
          await refreshLibrary();
        } catch (err) {
          pushToast("error", formatErrorMessage(err));
        }
      }}
      onBeginFolderDrag={(folderId, label, x, y) => {
        internalDrag.beginPendingDrag({ type: "folder", folderId, label }, x, y);
      }}
    />
  );

  return (
    <>
      {isWebClient && (
        <input
          ref={importFlow.browserUploadInputRef}
          type="file"
          multiple
          accept="image/*,video/*"
          style={{ display: "none" }}
          onChange={importFlow.handleBrowserUploadChange}
        />
      )}

      <div
        className={`workspace-shell ${inspectorOpen ? "" : "inspector-collapsed"} ${
          isCompactViewport ? "compact-shell" : ""
        } ${isMobileViewport ? "mobile-shell" : ""}`}
      >
        {isMobileViewport ? (
          <>
            {mobileDrawerOpen && (
              <div
                className="drawer-backdrop"
                aria-hidden="true"
                onClick={() => setMobileDrawerOpen(false)}
              />
            )}
            <div
              id="sidebar-drawer"
              className={`sidebar-drawer-host ${mobileDrawerOpen ? "open" : ""}`}
              aria-hidden={!mobileDrawerOpen}
            >
              {sidebarElement}
            </div>
          </>
        ) : (
          sidebarElement
        )}

        <main className="main-pane">
          <Toolbar
            view={view}
            folders={folders}
            itemCount={entries.length}
            canGoBack={historyIndex > 0}
            canGoForward={historyIndex < history.length - 1}
            onGoBack={() => setHistoryIndex((i) => Math.max(0, i - 1))}
            onGoForward={() => setHistoryIndex((i) => Math.min(history.length - 1, i + 1))}
            thumbSize={thumbSize}
            onThumbSizeChange={setThumbSize}
            fitMode={fitMode}
            onToggleFitMode={() => setFitMode((m) => (m === "ratio" ? "square" : "ratio"))}
            sortBy={sortBy}
            onSortByChange={setSortBy}
            searchText={searchText}
            onSearchTextChange={setSearchText}
            filterOpen={filterOpen}
            onToggleFilterOpen={() => setFilterOpen((v) => !v)}
            minRating={minRating}
            onMinRatingChange={setMinRating}
            mimeTypes={mimeTypes}
            onToggleMimeType={(mime) =>
              setMimeTypes((prev) =>
                prev.includes(mime) ? prev.filter((m) => m !== mime) : [...prev, mime]
              )
            }
            onRemoveTag={toggleTagInView}
            onClearAllFilters={() => {
              setMinRating(null);
              setMimeTypes([]);
              setSearchText("");
              navigateView({ kind: "all" });
            }}
            inspectorOpen={inspectorOpen}
            onToggleInspector={() => setInspectorOpen((v) => !v)}
            showMenuButton={isMobileViewport}
            mobileDrawerOpen={mobileDrawerOpen}
            onToggleMobileDrawer={() => setMobileDrawerOpen((v) => !v)}
            isTouchDevice={isTouchDevice}
            isWebClient={isWebClient}
            selectionMode={mobileSelectMode}
            onToggleSelectionMode={() => setMobileSelectMode((v) => !v)}
            onPlainExportFolder={handlePlainExportFolder}
          />

          <ThumbnailGrid
            entries={entries}
            selectedKeys={gallery.selectedKeys}
            primaryKey={gallery.primaryKey}
            thumbSize={thumbSize}
            fitMode={fitMode}
            isDraggingOsFiles={importFlow.isDraggingOsFiles}
            isTrashView={isTrashView}
            currentFolderId={currentFolderId}
            hoverDropTarget={internalDrag.hoverTarget}
            canEdit={canEdit}
            isWebClient={isWebClient}
            isTouchDevice={isTouchDevice}
            selectionMode={mobileSelectMode}
            onCardClick={gallery.handleCardClick}
            onSelectAll={gallery.selectAllEntries}
            onClearSelection={gallery.clearSelection}
            onSelectSingle={(entry) =>
              gallery.selectSingleKey(entryKey({ kind: entry.kind, id: entry.id }))
            }
            onOpenEntry={gallery.handleOpenEntry}
            onCardContextMenu={gallery.openCardContextMenu}
            onBeginEntryDrag={(entry, x, y) => {
              if (isTrashView) return;
              const k = entryKey({ kind: entry.kind, id: entry.id });
              const dragRefs = gallery.selectedKeys.has(k)
                ? gallery.selectedEntryRefs
                : [{ kind: entry.kind, id: entry.id }];
              const label =
                dragRefs.length === 1
                  ? entry.title || (entry.kind === "group" ? "漫画グループ" : "画像")
                  : `${dragRefs.length}件を追加`;
              internalDrag.beginPendingDrag(
                { type: "entries", entries: dragRefs, label },
                x,
                y
              );
            }}
            onImportFiles={() => void importFlow.handleImportFiles()}
            onAvifCompleted={handlePendingThumbCompleted}
            onCreateGroupFromSelection={() => void gallery.handleCreateGroupFromSelection()}
            onRemoveSelectionFromFolder={() => {
              if (!currentFolderId || gallery.selectedEntryRefs.length === 0) return;
              void vaultApi
                .removeEntriesFromFolder(currentFolderId, gallery.selectedEntryRefs)
                .then(() => {
                  pushToast("info", "選択した項目をフォルダから外しました");
                  return refreshLibrary();
                })
                .catch((err) => pushToast("error", formatErrorMessage(err)));
            }}
            onPlainExportSelection={handlePlainExportSelection}
            onMoveSelectionToTrash={() => {
              if (gallery.selectedEntryRefs.length === 0) return;
              void vaultApi
                .moveEntriesToTrash(gallery.selectedEntryRefs)
                .then(() => {
                  pushToast("info", `${gallery.selectedEntryRefs.length}件をゴミ箱へ移動しました`);
                  return refreshLibrary();
                })
                .catch((err) => pushToast("error", formatErrorMessage(err)));
            }}
            onRestoreAllTrash={() => {
              const allRefs = entries.map((e) => ({ kind: e.kind, id: e.id }));
              void vaultApi
                .restoreEntriesFromTrash(allRefs)
                .then(() => {
                  pushToast("success", "ゴミ箱の項目をすべて復元しました");
                  return refreshLibrary();
                })
                .catch((err) => pushToast("error", formatErrorMessage(err)));
            }}
            onEmptyTrash={() => {
              const idsToInvalidate = entries.map((e) => e.id);
              void vaultApi
                .emptyTrash()
                .then(() => {
                  imageMemoryCache.invalidateItems(idsToInvalidate);
                  pushToast("info", "ゴミ箱を空にしました");
                  return refreshLibrary();
                })
                .catch((err) => pushToast("error", formatErrorMessage(err)));
            }}
          />
        </main>

        {inspectorOpen && (
          <>
            {isCompactViewport && (
              <div
                className="drawer-backdrop"
                aria-hidden="true"
                onClick={() => setInspectorOpen(false)}
              />
            )}
            <aside
              className={`inspector ${isCompactViewport ? "drawer-open" : ""}`}
              aria-label="詳細パネル"
            >
              <Inspector
                mode={gallery.inspectorMode}
                itemDetail={gallery.itemDetail}
                groupDetail={gallery.groupDetail}
                selectedEntries={gallery.selectedEntryRefs}
                selectedEntriesMeta={gallery.selectedEntriesMeta}
                status={status}
                allTags={tags}
                allFolders={folders}
                currentFolderId={currentFolderId}
                inTrash={isTrashView}
                readOnly={!canEdit}
                isWebClient={isWebClient}
                onClose={isCompactViewport ? () => setInspectorOpen(false) : undefined}
                onUpdateTitle={async (id, title) => {
                  if (gallery.inspectorMode === "group") {
                    const updated = await vaultApi.setGroupTitle(id, title);
                    gallery.setGroupDetail(updated);
                  } else {
                    const updated = await vaultApi.setItemTitle(id, title);
                    gallery.setItemDetail(updated);
                  }
                  await refreshLibrary();
                }}
                onUpdateRating={async (id, rating) => {
                  if (gallery.inspectorMode === "group") {
                    const updated = await vaultApi.setGroupRating(id, rating);
                    gallery.setGroupDetail(updated);
                  } else {
                    const updated = await vaultApi.setItemRating(id, rating);
                    gallery.setItemDetail(updated);
                  }
                  await refreshLibrary();
                }}
                onUpdateMemo={async (id, memo) => {
                  if (gallery.inspectorMode === "group") {
                    const updated = await vaultApi.setGroupMemo(id, memo);
                    gallery.setGroupDetail(updated);
                  } else {
                    const updated = await vaultApi.setItemMemo(id, memo);
                    gallery.setItemDetail(updated);
                  }
                  await refreshLibrary();
                }}
                onUpdateSourceUrl={async (id, sourceUrl) => {
                  const updated = await vaultApi.setItemSourceUrl(id, sourceUrl);
                  gallery.setItemDetail(updated);
                  await refreshLibrary();
                }}
                onCopyToast={(msg) => pushToast("info", msg)}
                onAddTag={async (id, tag) => {
                  if (gallery.inspectorMode === "group") {
                    const updated = await vaultApi.addGroupTag(id, tag);
                    gallery.setGroupDetail(updated);
                  } else {
                    const updated = await vaultApi.addItemTag(id, tag);
                    gallery.setItemDetail(updated);
                  }
                  await refreshLibrary();
                }}
                onRemoveTag={async (id, tag) => {
                  if (gallery.inspectorMode === "group") {
                    const updated = await vaultApi.removeGroupTag(id, tag);
                    gallery.setGroupDetail(updated);
                  } else {
                    const updated = await vaultApi.removeItemTag(id, tag);
                    gallery.setItemDetail(updated);
                  }
                  await refreshLibrary();
                }}
                onAddToFolder={async (folderId) => {
                  await vaultApi.addEntriesToFolder(folderId, gallery.selectedEntryRefs);
                  if (gallery.selectedEntryRefs.length === 1) {
                    const ref = gallery.selectedEntryRefs[0];
                    if (ref.kind === "image") {
                      gallery.setItemDetail(await vaultApi.getItemDetail(ref.id));
                    } else {
                      gallery.setGroupDetail(await vaultApi.getGroupDetail(ref.id));
                    }
                  }
                  await refreshLibrary();
                }}
                onRemoveFromFolder={async (folderId) => {
                  await vaultApi.removeEntriesFromFolder(folderId, gallery.selectedEntryRefs);
                  if (gallery.selectedEntryRefs.length === 1) {
                    const ref = gallery.selectedEntryRefs[0];
                    if (ref.kind === "image") {
                      gallery.setItemDetail(await vaultApi.getItemDetail(ref.id));
                    } else {
                      gallery.setGroupDetail(await vaultApi.getGroupDetail(ref.id));
                    }
                  }
                  await refreshLibrary();
                }}
                onOpenLightbox={(id) => gallery.setLightboxId(id)}
                onOpenReader={(id) => gallery.setReaderGroupId(id)}
                onOpenEditor={(id) => gallery.setEditorGroupId(id)}
                onUngroup={async () => {
                  if (!gallery.groupDetail) return;
                  await vaultApi.ungroup(gallery.groupDetail.id);
                  pushToast("info", "グループを解除して単体画像に戻しました");
                  await refreshLibrary();
                }}
                onCreateGroupFromSelection={gallery.handleCreateGroupFromSelection}
                onPlainExport={handlePlainExportSelection}
                onMoveToTrash={async () => {
                  await vaultApi.moveEntriesToTrash(gallery.selectedEntryRefs);
                  pushToast("info", `${gallery.selectedEntryRefs.length}件をゴミ箱へ移動しました`);
                  await refreshLibrary();
                }}
                onRestoreFromTrash={async () => {
                  await vaultApi.restoreEntriesFromTrash(gallery.selectedEntryRefs);
                  pushToast("success", `${gallery.selectedEntryRefs.length}件を復元しました`);
                  await refreshLibrary();
                }}
                onPermanentlyDelete={() =>
                  gallery.executePermanentDelete(gallery.selectedEntryRefs)
                }
              />
            </aside>
          </>
        )}
      </div>

      <LightboxDialog
        open={gallery.lightboxId !== null}
        items={entries}
        currentId={gallery.lightboxId}
        detail={gallery.itemDetail}
        status={status}
        allTags={tags}
        allFolders={folders}
        currentFolderId={currentFolderId}
        readOnly={!canEdit}
        isWebClient={isWebClient}
        onClose={() => gallery.setLightboxId(null)}
        onNavigate={(id) => {
          const k = entryKey({ kind: "image", id });
          gallery.setSelectedKeys(new Set([k]));
          gallery.setPrimaryKey(k);
          gallery.setLightboxId(id);
        }}
        onUpdateTitle={async (id, title) => {
          gallery.setItemDetail(await vaultApi.setItemTitle(id, title));
          await refreshLibrary();
        }}
        onUpdateRating={async (id, rating) => {
          gallery.setItemDetail(await vaultApi.setItemRating(id, rating));
          await refreshLibrary();
        }}
        onUpdateMemo={async (id, memo) => {
          gallery.setItemDetail(await vaultApi.setItemMemo(id, memo));
          await refreshLibrary();
        }}
        onUpdateSourceUrl={async (id, sourceUrl) => {
          gallery.setItemDetail(await vaultApi.setItemSourceUrl(id, sourceUrl));
          await refreshLibrary();
        }}
        onCopyToast={(msg) => pushToast("info", msg)}
        onAddTag={async (id, tag) => {
          gallery.setItemDetail(await vaultApi.addItemTag(id, tag));
          await refreshLibrary();
        }}
        onRemoveTag={async (id, tag) => {
          gallery.setItemDetail(await vaultApi.removeItemTag(id, tag));
          await refreshLibrary();
        }}
        onAddToFolder={async (fid, id) => {
          await vaultApi.addEntriesToFolder(fid, [{ kind: "image", id }]);
          gallery.setItemDetail(await vaultApi.getItemDetail(id));
          await refreshLibrary();
        }}
        onRemoveFromFolder={async (fid, id) => {
          await vaultApi.removeEntriesFromFolder(fid, [{ kind: "image", id }]);
          gallery.setItemDetail(await vaultApi.getItemDetail(id));
          await refreshLibrary();
        }}
        onPlainExport={(item) =>
          requestPlainExport({
            kind: "item",
            itemId: item.id,
            title: item.title,
            mimeType: item.coverMimeType,
          })
        }
        onMoveToTrash={async (id) => {
          await vaultApi.moveEntriesToTrash([{ kind: "image", id }]);
          gallery.setLightboxId(null);
          pushToast("info", "ゴミ箱へ移動しました");
          await refreshLibrary();
        }}
        onRestoreFromTrash={async (id) => {
          await vaultApi.restoreEntriesFromTrash([{ kind: "image", id }]);
          gallery.setLightboxId(null);
          pushToast("success", "復元しました");
          await refreshLibrary();
        }}
        onPermanentlyDelete={async (id) => {
          await gallery.executePermanentDelete([{ kind: "image", id }]);
        }}
      />

      <MangaReaderDialog
        open={gallery.readerGroupId !== null && gallery.activeModalGroup !== null}
        group={gallery.activeModalGroup}
        status={status}
        allTags={tags}
        allFolders={folders}
        readOnly={!canEdit}
        isWebClient={isWebClient}
        onClose={() => gallery.setReaderGroupId(null)}
        onOpenEditor={(gid) => gallery.setEditorGroupId(gid)}
        onUpdateReaderPrefs={async (gid, readingDir, spreadMode, coverAlone) => {
          const updated = await vaultApi.setGroupReaderPrefs(
            gid,
            readingDir,
            spreadMode,
            coverAlone
          );
          gallery.setActiveModalGroup(updated);
          if (gallery.groupDetail?.id === gid) gallery.setGroupDetail(updated);
        }}
        onUpdateTitle={async (gid, title) => {
          const updated = await vaultApi.setGroupTitle(gid, title);
          gallery.setActiveModalGroup(updated);
          await refreshLibrary();
        }}
        onUpdateRating={async (gid, rating) => {
          const updated = await vaultApi.setGroupRating(gid, rating);
          gallery.setActiveModalGroup(updated);
          await refreshLibrary();
        }}
        onUpdateMemo={async (gid, memo) => {
          const updated = await vaultApi.setGroupMemo(gid, memo);
          gallery.setActiveModalGroup(updated);
          await refreshLibrary();
        }}
        onAddTag={async (gid, tag) => {
          const updated = await vaultApi.addGroupTag(gid, tag);
          gallery.setActiveModalGroup(updated);
          await refreshLibrary();
        }}
        onRemoveTag={async (gid, tag) => {
          const updated = await vaultApi.removeGroupTag(gid, tag);
          gallery.setActiveModalGroup(updated);
          await refreshLibrary();
        }}
        onAddToFolder={async (fid, gid) => {
          await vaultApi.addEntriesToFolder(fid, [{ kind: "group", id: gid }]);
          gallery.setActiveModalGroup(await vaultApi.getGroupDetail(gid));
          await refreshLibrary();
        }}
        onRemoveFromFolder={async (fid, gid) => {
          await vaultApi.removeEntriesFromFolder(fid, [{ kind: "group", id: gid }]);
          gallery.setActiveModalGroup(await vaultApi.getGroupDetail(gid));
          await refreshLibrary();
        }}
        onPlainExport={(grp) =>
          requestPlainExport({
            kind: "group",
            groupId: grp.id,
            title: grp.title,
            pageCount: grp.pageCount,
          })
        }
        onUngroup={async (gid) => {
          await vaultApi.ungroup(gid);
          gallery.setReaderGroupId(null);
          pushToast("info", "グループを解除しました");
          await refreshLibrary();
        }}
        onMoveToTrash={async (gid) => {
          await vaultApi.moveEntriesToTrash([{ kind: "group", id: gid }]);
          gallery.setReaderGroupId(null);
          pushToast("info", "ゴミ箱へ移動しました");
          await refreshLibrary();
        }}
      />

      <GroupEditorDialog
        open={gallery.editorGroupId !== null && gallery.activeModalGroup !== null}
        group={gallery.activeModalGroup}
        onClose={() => gallery.setEditorGroupId(null)}
        onReorder={async (gid, orderedIds) => {
          const updated = await vaultApi.reorderGroupPages(gid, orderedIds);
          gallery.setActiveModalGroup(updated);
          if (gallery.groupDetail?.id === gid) gallery.setGroupDetail(updated);
          await refreshLibrary();
        }}
        onRemovePage={async (gid, itemId) => {
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
        }}
        onAddFilesToGroup={async (gid, paths) => {
          const report = await vaultApi.importPaths(paths, null);
          if (report.importedIds.length > 0) {
            const updated = await vaultApi.addItemsToGroup(gid, report.importedIds);
            gallery.setActiveModalGroup(updated);
            if (gallery.groupDetail?.id === gid) gallery.setGroupDetail(updated);
            pushToast("success", `${report.importedIds.length}ページを追加しました`);
          }
          await refreshLibrary();
          importFlow.processPendingThumbnails(report.avifPendingIds, report.videoPendingIds);
        }}
        onAddBrowserFilesToGroup={async (gid, files) => {
          const report = await importFlow.uploadBrowserFiles(files, null);
          if (report.importedIds.length > 0) {
            const updated = await vaultApi.addItemsToGroup(gid, report.importedIds);
            gallery.setActiveModalGroup(updated);
            if (gallery.groupDetail?.id === gid) gallery.setGroupDetail(updated);
            pushToast("success", `${report.importedIds.length}ページを追加しました`);
          }
          await refreshLibrary();
          importFlow.processPendingThumbnails(report.avifPendingIds, report.videoPendingIds);
        }}
      />

      {!isWebClient && (
        <LockSettingsDialog
          open={lockSettingsOpen}
          settings={status.lockSettings}
          urlImportSettings={status.urlImportSettings}
          skipPlainExportConfirm={status.skipPlainExportConfirm}
          onClose={() => setLockSettingsOpen(false)}
          onSave={async (
            lockNext: LockSettings,
            urlNext: UrlImportSettings,
            skipConfirmNext: boolean
          ) => {
            try {
              const [savedLock, savedUrl, savedSkip] = await Promise.all([
                vaultApi.setLockSettings(lockNext),
                vaultApi.setUrlImportSettings(urlNext),
                vaultApi.setSkipPlainExportConfirm(skipConfirmNext),
              ]);
              setStatus((prev) => ({
                ...prev,
                lockSettings: savedLock,
                urlImportSettings: savedUrl,
                skipPlainExportConfirm: savedSkip,
              }));
              pushToast("success", "設定を保存しました");
            } catch (err) {
              pushToast("error", formatErrorMessage(err));
            }
          }}
        />
      )}

      <UrlImportDialog
        open={importFlow.urlImportDialog.open}
        initialUrl={importFlow.urlImportDialog.initialUrl}
        autoStart={importFlow.urlImportDialog.autoStart}
        urlImportSettings={status.urlImportSettings}
        onClose={importFlow.closeUrlImportDialog}
        onOpenSettings={() => setLockSettingsOpen(true)}
        onSaveUrlSettings={async (next: UrlImportSettings) => {
          const saved = await vaultApi.setUrlImportSettings(next);
          setStatus((prev) => ({ ...prev, urlImportSettings: saved }));
        }}
        onImportUrl={importFlow.handleImportUrl}
      />

      {!isWebClient && (
        <VaultMaintenanceDialog
          open={maintenanceModal.open}
          initialTab={maintenanceModal.initialTab}
          folders={folders}
          tags={tags}
          preselectedFolderIds={maintenanceModal.preselectedFolderIds}
          preselectedEntries={maintenanceModal.preselectedEntries}
          onClose={() =>
            setMaintenanceModal((prev) => ({ ...prev, open: false }))
          }
          onDataChanged={async (avifPendingIds, videoPendingIds) => {
            await refreshLibrary();
            importFlow.processPendingThumbnails(avifPendingIds ?? [], videoPendingIds ?? []);
          }}
          onToast={pushToast}
        />
      )}

      {!isWebClient && (
        <LanShareDialog
          open={session.lanDialogOpen}
          status={session.lanShareStatus}
          onStatusChange={session.setLanShareStatus}
          onClose={() => session.setLanDialogOpen(false)}
          onToast={pushToast}
        />
      )}

      <ContextMenu
        open={gallery.cardMenu !== null}
        x={gallery.cardMenu?.x ?? 0}
        y={gallery.cardMenu?.y ?? 0}
        items={gallery.cardContextItems}
        onClose={() => gallery.setCardMenu(null)}
      />

      <dialog
        ref={gallery.permDeleteDialogRef}
        className="confirm-dialog"
        aria-labelledby="ctx-perm-delete-title"
        onClose={(e) => {
          const dlg = e.currentTarget;
          const val = dlg.returnValue;
          dlg.returnValue = "";
          const targets = gallery.pendingDeleteEntries;
          gallery.setPendingDeleteEntries(null);
          if (val === "delete" && targets && targets.length > 0) {
            void gallery.executePermanentDelete(targets);
          }
        }}
      >
        <div className="confirm-dialog-inner">
          <h2 id="ctx-perm-delete-title">完全に削除しますか？</h2>
          <p>
            選択した {gallery.pendingDeleteEntries?.length ?? 0} 件を暗号化保管庫から復元できない形で完全に消去します。この操作は取り消せません。
          </p>
          <form method="dialog" className="confirm-dialog-actions">
            <button type="submit" value="cancel" className="btn" autoFocus>
              キャンセル
            </button>
            <button type="submit" value="delete" className="btn btn-danger">
              完全に削除
            </button>
          </form>
        </div>
      </dialog>

      <PlainExportConfirmDialog
        target={plainExport.pendingTarget}
        isWebClient={isWebClient}
        busy={plainExport.exporting}
        onCancel={plainExport.cancelPendingExport}
        onConfirm={plainExport.confirmPendingExport}
      />

      {internalDrag.activePayload && (
        <div
          className="drag-ghost"
          style={{
            insetInlineStart: internalDrag.cursor.x + 14,
            insetBlockStart: internalDrag.cursor.y + 12,
          }}
        >
          <Folder size={13} />
          <span>{internalDrag.activePayload.label}</span>
        </div>
      )}

      <ToastRegion toasts={toasts} onDismiss={dismissToast} />
    </>
  );
}
