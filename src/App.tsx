import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { ContextMenu } from "./components/ContextMenu";
import { DragGhost } from "./components/DragGhost";
import { Inspector } from "./components/Inspector";
import { LanAuthScreen } from "./components/LanAuthScreen";
import { PlainExportConfirmDialog } from "./components/PlainExportConfirmDialog";
import { Sidebar } from "./components/Sidebar";
import { ThumbnailGrid } from "./components/ThumbnailGrid";
import { ToastRegion } from "./components/ToastRegion";
import { Toolbar } from "./components/Toolbar";
import { VaultGate } from "./components/VaultGate";
import { VaultLockScreen } from "./components/VaultLockScreen";
import type { MaintenanceTab } from "./components/VaultMaintenanceDialog";
import { useGallerySelection } from "./hooks/useGallerySelection";
import { useImportFlow } from "./hooks/useImportFlow";
import { useInternalDrag } from "./hooks/useInternalDrag";
import { useLibraryMutations } from "./hooks/useLibraryMutations";
import { useLibraryView } from "./hooks/useLibraryView";
import { usePlainExport } from "./hooks/usePlainExport";
import { useToasts } from "./hooks/useToasts";
import { useVaultSession } from "./hooks/useVaultSession";
import { formatErrorMessage, vaultApi } from "./services/vaultApi";
import {
  entryKey,
  isVideoMime,
  type EntryRef,
  type GalleryEntry,
  type LockSettings,
  type UrlImportSettings,
} from "./types/vault";

const BulkTagDialog = lazy(() =>
  import("./components/BulkTagDialog").then((m) => ({ default: m.BulkTagDialog }))
);
const GroupEditorDialog = lazy(() =>
  import("./components/GroupEditorDialog").then((m) => ({ default: m.GroupEditorDialog }))
);
const LanShareDialog = lazy(() =>
  import("./components/LanShareDialog").then((m) => ({ default: m.LanShareDialog }))
);
const LightboxDialog = lazy(() =>
  import("./components/LightboxDialog").then((m) => ({ default: m.LightboxDialog }))
);
const LockSettingsDialog = lazy(() =>
  import("./components/LockSettingsDialog").then((m) => ({ default: m.LockSettingsDialog }))
);
const MangaReaderDialog = lazy(() =>
  import("./components/MangaReaderDialog").then((m) => ({ default: m.MangaReaderDialog }))
);
const UrlImportDialog = lazy(() =>
  import("./components/UrlImportDialog").then((m) => ({ default: m.UrlImportDialog }))
);
const VaultMaintenanceDialog = lazy(() =>
  import("./components/VaultMaintenanceDialog").then((m) => ({
    default: m.VaultMaintenanceDialog,
  }))
);

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

  const [lockSettingsOpen, setLockSettingsOpen] = useState(false);
  const [bulkTagDialogOpen, setBulkTagDialogOpen] = useState(false);
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

  const [thumbSize, setThumbSize] = useState(210);
  const [fitMode, setFitMode] = useState<"ratio" | "square">("ratio");
  const [inspectorOpen, setInspectorOpen] = useState(() =>
    typeof window !== "undefined" ? !window.matchMedia("(max-width: 1023px)").matches : true
  );

  const libraryView = useLibraryView({
    isOpen: status.isOpen,
    setStatus,
    pushToast,
    onSyncSelection: (nextEntries) => syncSelectionRef.current(nextEntries),
    onNavigate: () => {
      setMobileDrawerOpen(false);
      setBulkTagDialogOpen(false);
    },
  });
  const { entries, folders, tags, view, currentFolderId, isTrashView, refreshLibrary } =
    libraryView;

  refreshLibraryRef.current = refreshLibrary;
  const triggerRefreshLibrary = useCallback(() => refreshLibraryRef.current(), []);
  const triggerBatchSetRatingRef = useRef<(targetEntries: EntryRef[], rating: number) => Promise<void>>(
    async () => {}
  );

  const handleOpenExportPackageForEntries = useCallback((selectedEntries: EntryRef[]) => {
    setMaintenanceModal({
      open: true,
      initialTab: "export",
      preselectedFolderIds: [],
      preselectedEntries: selectedEntries,
    });
  }, []);

  const handleOpenBulkTagDialog = useCallback(() => {
    setBulkTagDialogOpen(true);
  }, []);

  const triggerBatchSetRating = useCallback(
    (targetEntries: EntryRef[], rating: number) =>
      triggerBatchSetRatingRef.current(targetEntries, rating),
    []
  );

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
    onOpenBulkTagDialog: handleOpenBulkTagDialog,
    onBatchSetRating: triggerBatchSetRating,
    pushToast,
  });
  syncSelectionRef.current = gallery.syncSelectionWithEntries;

  const importFlow = useImportFlow({
    isOpen: status.isOpen,
    canEdit,
    isWebClient,
    isTrashView,
    currentFolderId,
    autoUrlImportEnabled: status.urlImportSettings.enabled,
    lockSettingsOpen,
    refreshLibrary: triggerRefreshLibrary,
    onSelectImportedKey: gallery.selectSingleKey,
    pushToast,
  });

  const mutations = useLibraryMutations({
    entries,
    folders,
    view,
    currentFolderId,
    hasActiveFilters: libraryView.hasActiveFilters,
    navigateView: libraryView.navigateView,
    refreshLibrary,
    setStatus,
    pushToast,
    uploadBrowserFiles: importFlow.uploadBrowserFiles,
    processPendingThumbnails: importFlow.processPendingThumbnails,
    gallery,
  });
  triggerBatchSetRatingRef.current = mutations.triggerBatchSetRating;

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

  resetWorkspaceRef.current = () => {
    gallery.clearSelectionState();
    resetPlainExportState();
    setLockSettingsOpen(false);
    setBulkTagDialogOpen(false);
    mutations.resetMutationState();
    setMobileDrawerOpen(false);
    setMobileSelectMode(false);
    importFlow.closeUrlImportDialog();
    setMaintenanceModal({
      open: false,
      initialTab: "maintenance",
      preselectedFolderIds: [],
      preselectedEntries: [],
    });
    libraryView.resetLibraryViewState();
  };

  readOnlyEnforcedRef.current = () => {
    setMobileSelectMode(false);
    setBulkTagDialogOpen(false);
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

  const internalDrag = useInternalDrag(mutations.dragDropHandlers);

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
      sidebarFilter={libraryView.sidebarFilter}
      hoverDropTarget={internalDrag.hoverTarget}
      isWebClient={isWebClient}
      canEdit={canEdit}
      lanShareStatus={session.lanShareStatus}
      isDrawerMode={isMobileViewport}
      onCloseDrawer={() => setMobileDrawerOpen(false)}
      onOpenLanShare={() => session.setLanDialogOpen(true)}
      onLogoutLan={() => void session.handleLogoutLan()}
      onSidebarFilterChange={libraryView.setSidebarFilter}
      onSelectView={libraryView.navigateView}
      onToggleTag={libraryView.toggleTagInView}
      onImportFiles={() => void importFlow.handleImportFiles()}
      onOpenUrlImport={() => {
        setMobileDrawerOpen(false);
        importFlow.setUrlImportDialog({ open: true, initialUrl: "", autoStart: false });
      }}
      onImportFolderAsGroup={() => void importFlow.handleImportFolderAsGroup()}
      onToggleClipboardWatch={() =>
        mutations.toggleClipboardWatch(status.clipboardWatchEnabled)
      }
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
      onCreateFolder={mutations.folderHandlers.createFolder}
      onRenameFolder={mutations.folderHandlers.renameFolder}
      onMoveFolder={mutations.folderHandlers.moveFolder}
      onDeleteFolder={mutations.folderHandlers.deleteFolder}
      onBeginFolderDrag={(folderId, label, x, y, pointerId) => {
        internalDrag.beginPendingDrag({ type: "folder", folderId, label }, x, y, pointerId);
      }}
    />
  );

  const lightboxOpen = gallery.lightboxId !== null;
  const readerOpen = gallery.readerGroupId !== null && gallery.activeModalGroup !== null;
  const editorOpen = gallery.editorGroupId !== null && gallery.activeModalGroup !== null;
  const bulkTagOpen = bulkTagDialogOpen && canEdit && !isTrashView;

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
            canGoBack={libraryView.canGoBack}
            canGoForward={libraryView.canGoForward}
            onGoBack={libraryView.goBack}
            onGoForward={libraryView.goForward}
            thumbSize={thumbSize}
            onThumbSizeChange={setThumbSize}
            fitMode={fitMode}
            onToggleFitMode={() => setFitMode((m) => (m === "ratio" ? "square" : "ratio"))}
            sortBy={libraryView.sortBy}
            onSortByChange={libraryView.setSortBy}
            searchText={libraryView.searchText}
            onSearchTextChange={libraryView.setSearchText}
            mediaType={libraryView.mediaType}
            onMediaTypeChange={libraryView.handleMediaTypeChange}
            filterOpen={libraryView.filterOpen}
            onToggleFilterOpen={() => libraryView.setFilterOpen((v) => !v)}
            minRating={libraryView.minRating}
            onMinRatingChange={libraryView.setMinRating}
            mimeTypes={libraryView.mimeTypes}
            onToggleMimeType={libraryView.toggleMimeType}
            onRemoveTag={libraryView.toggleTagInView}
            onClearAllFilters={libraryView.clearAllFiltersAndView}
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
            selectedEntriesMeta={gallery.selectedEntriesMeta}
            primaryKey={gallery.primaryKey}
            thumbSize={thumbSize}
            fitMode={fitMode}
            isDraggingOsFiles={importFlow.isDraggingOsFiles}
            isTrashView={isTrashView}
            trashCount={status.trashCount}
            hasActiveFilters={libraryView.hasActiveFilters}
            onResetFilters={libraryView.handleResetFilters}
            currentFolderId={currentFolderId}
            hoverDropTarget={internalDrag.hoverTarget}
            canEdit={canEdit}
            isWebClient={isWebClient}
            isTouchDevice={isTouchDevice}
            selectionMode={mobileSelectMode}
            batchRatingBusy={mutations.batchRatingBusy}
            onCardClick={gallery.handleCardClick}
            onSelectAll={gallery.selectAllEntries}
            onClearSelection={gallery.clearSelection}
            onSelectSingle={(entry) =>
              gallery.selectSingleKey(entryKey({ kind: entry.kind, id: entry.id }))
            }
            onOpenEntry={gallery.handleOpenEntry}
            onCardContextMenu={gallery.openCardContextMenu}
            onBeginEntryDrag={(entry, x, y, pointerId) => {
              if (isTrashView) return;
              const k = entryKey({ kind: entry.kind, id: entry.id });
              const inSel = gallery.selectedKeys.has(k);
              const dragRefs = inSel
                ? gallery.selectedEntryRefs
                : [{ kind: entry.kind, id: entry.id }];
              const dragEntriesMeta = inSel ? gallery.selectedEntriesMeta : [entry];
              const hasImageForGroup = dragEntriesMeta.some(
                (item) => item.kind === "image" && !isVideoMime(item.coverMimeType)
              );
              const label =
                dragRefs.length === 1
                  ? entry.title || (entry.kind === "group" ? "漫画グループ" : "画像")
                  : `${dragRefs.length}件を追加`;
              internalDrag.beginPendingDrag(
                { type: "entries", entries: dragRefs, hasImageForGroup, label },
                x,
                y,
                pointerId
              );
            }}
            onImportFiles={() => void importFlow.handleImportFiles()}
            onAvifCompleted={triggerRefreshLibrary}
            onCreateGroupFromSelection={() => void gallery.handleCreateGroupFromSelection()}
            onBatchSetRating={(rating) =>
              void mutations.applyBatchRatingUpdate(gallery.selectedEntryRefs, rating)
            }
            onOpenBulkTagDialog={() => setBulkTagDialogOpen(true)}
            onRemoveSelectionFromFolder={mutations.gridBulkHandlers.removeSelectionFromFolder}
            onPlainExportSelection={handlePlainExportSelection}
            onMoveSelectionToTrash={mutations.gridBulkHandlers.moveSelectionToTrash}
            onRestoreAllTrash={mutations.gridBulkHandlers.restoreAllTrash}
            onEmptyTrash={mutations.gridBulkHandlers.emptyTrash}
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
                onUpdateTitle={mutations.inspectorHandlers.updateTitle}
                onUpdateRating={mutations.inspectorHandlers.updateRating}
                onUpdateMemo={mutations.inspectorHandlers.updateMemo}
                onUpdateSourceUrl={mutations.inspectorHandlers.updateSourceUrl}
                onCopyToast={(msg) => pushToast("info", msg)}
                onAddTag={mutations.inspectorHandlers.addTag}
                onRemoveTag={mutations.inspectorHandlers.removeTag}
                onBatchSetRating={(rating) =>
                  mutations.applyBatchRatingUpdate(gallery.selectedEntryRefs, rating)
                }
                batchRatingBusy={mutations.batchRatingBusy}
                onBatchUpdateTags={mutations.inspectorHandlers.batchUpdateTags}
                onOpenBulkTagDialog={() => setBulkTagDialogOpen(true)}
                onAddToFolder={mutations.inspectorHandlers.addToFolder}
                onRemoveFromFolder={mutations.inspectorHandlers.removeFromFolder}
                onOpenLightbox={(id) => gallery.setLightboxId(id)}
                onOpenReader={(id) => gallery.setReaderGroupId(id)}
                onOpenEditor={(id) => gallery.setEditorGroupId(id)}
                onUngroup={mutations.inspectorHandlers.ungroup}
                onCreateGroupFromSelection={gallery.handleCreateGroupFromSelection}
                onPlainExport={handlePlainExportSelection}
                onMoveToTrash={mutations.inspectorHandlers.moveToTrash}
                onRestoreFromTrash={mutations.inspectorHandlers.restoreFromTrash}
                onPermanentlyDelete={() =>
                  gallery.executePermanentDelete(gallery.selectedEntryRefs)
                }
              />
            </aside>
          </>
        )}
      </div>

      {lightboxOpen && (
        <Suspense fallback={null}>
          <LightboxDialog
            open
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
            onUpdateTitle={mutations.lightboxHandlers.updateTitle}
            onUpdateRating={mutations.lightboxHandlers.updateRating}
            onUpdateMemo={mutations.lightboxHandlers.updateMemo}
            onUpdateSourceUrl={mutations.lightboxHandlers.updateSourceUrl}
            onCopyToast={(msg) => pushToast("info", msg)}
            onAddTag={mutations.lightboxHandlers.addTag}
            onRemoveTag={mutations.lightboxHandlers.removeTag}
            onAddToFolder={mutations.lightboxHandlers.addToFolder}
            onRemoveFromFolder={mutations.lightboxHandlers.removeFromFolder}
            onPlainExport={(item) =>
              requestPlainExport({
                kind: "item",
                itemId: item.id,
                title: item.title,
                mimeType: item.coverMimeType,
              })
            }
            onMoveToTrash={mutations.lightboxHandlers.moveToTrash}
            onRestoreFromTrash={mutations.lightboxHandlers.restoreFromTrash}
            onPermanentlyDelete={async (id) => {
              await gallery.executePermanentDelete([{ kind: "image", id }]);
            }}
          />
        </Suspense>
      )}

      {readerOpen && (
        <Suspense fallback={null}>
          <MangaReaderDialog
            open
            group={gallery.activeModalGroup}
            status={status}
            allTags={tags}
            allFolders={folders}
            readOnly={!canEdit}
            isWebClient={isWebClient}
            onClose={() => gallery.setReaderGroupId(null)}
            onOpenEditor={(gid) => gallery.setEditorGroupId(gid)}
            onUpdateReaderPrefs={mutations.readerHandlers.updateReaderPrefs}
            onUpdateTitle={mutations.readerHandlers.updateTitle}
            onUpdateRating={mutations.readerHandlers.updateRating}
            onUpdateMemo={mutations.readerHandlers.updateMemo}
            onAddTag={mutations.readerHandlers.addTag}
            onRemoveTag={mutations.readerHandlers.removeTag}
            onAddToFolder={mutations.readerHandlers.addToFolder}
            onRemoveFromFolder={mutations.readerHandlers.removeFromFolder}
            onPlainExport={(grp) =>
              requestPlainExport({
                kind: "group",
                groupId: grp.id,
                title: grp.title,
                pageCount: grp.pageCount,
              })
            }
            onUngroup={mutations.readerHandlers.ungroup}
            onMoveToTrash={mutations.readerHandlers.moveToTrash}
            onRestoreFromTrash={mutations.readerHandlers.restoreFromTrash}
            onPermanentlyDelete={async (gid) => {
              await gallery.executePermanentDelete([{ kind: "group", id: gid }]);
            }}
          />
        </Suspense>
      )}

      {editorOpen && (
        <Suspense fallback={null}>
          <GroupEditorDialog
            open
            group={gallery.activeModalGroup}
            onClose={() => gallery.setEditorGroupId(null)}
            onReorder={mutations.editorHandlers.reorderPages}
            onRemovePage={mutations.editorHandlers.removePage}
            onAddFilesToGroup={mutations.editorHandlers.addFilesToGroup}
            onAddBrowserFilesToGroup={mutations.editorHandlers.addBrowserFilesToGroup}
          />
        </Suspense>
      )}

      {!isWebClient && lockSettingsOpen && (
        <Suspense fallback={null}>
          <LockSettingsDialog
            open
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
        </Suspense>
      )}

      {importFlow.urlImportDialog.open && (
        <Suspense fallback={null}>
          <UrlImportDialog
            open
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
        </Suspense>
      )}

      {!isWebClient && maintenanceModal.open && (
        <Suspense fallback={null}>
          <VaultMaintenanceDialog
            open
            initialTab={maintenanceModal.initialTab}
            folders={folders}
            tags={tags}
            preselectedFolderIds={maintenanceModal.preselectedFolderIds}
            preselectedEntries={maintenanceModal.preselectedEntries}
            onClose={() => setMaintenanceModal((prev) => ({ ...prev, open: false }))}
            onDataChanged={async (avifPendingIds, videoPendingIds) => {
              await refreshLibrary();
              importFlow.processPendingThumbnails(avifPendingIds ?? [], videoPendingIds ?? []);
            }}
            onToast={pushToast}
          />
        </Suspense>
      )}

      {!isWebClient && session.lanDialogOpen && (
        <Suspense fallback={null}>
          <LanShareDialog
            open
            status={session.lanShareStatus}
            onStatusChange={session.setLanShareStatus}
            onClose={() => session.setLanDialogOpen(false)}
            onToast={pushToast}
          />
        </Suspense>
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
            選択した {gallery.pendingDeleteEntries?.length ?? 0}{" "}
            件を暗号化保管庫から復元できない形で完全に消去します。この操作は取り消せません。
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

      {bulkTagOpen && (
        <Suspense fallback={null}>
          <BulkTagDialog
            open
            entries={gallery.selectedEntriesMeta}
            allTags={tags}
            onClose={() => setBulkTagDialogOpen(false)}
            onApply={mutations.applyBatchTagUpdate}
          />
        </Suspense>
      )}

      <DragGhost
        activePayload={internalDrag.activePayload}
        hoverTarget={internalDrag.hoverTarget}
        ghostRef={internalDrag.ghostRef}
        folders={folders}
      />

      <ToastRegion toasts={toasts} onDismiss={dismissToast} />
    </>
  );
}
