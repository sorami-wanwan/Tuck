import { useRef } from "react";
import type {
  EntryRef,
  FolderNode,
  GalleryEntry,
  GroupDetail,
  ItemDetail,
  TagWithCount,
  VaultStatus,
} from "../types/vault";
import { InspectorDetailView } from "./inspector/InspectorDetailView";
import { InspectorEmptyView } from "./inspector/InspectorEmptyView";
import { InspectorMultiView } from "./inspector/InspectorMultiView";

export interface InspectorProps {
  mode: "empty" | "image" | "group" | "multi";
  itemDetail: ItemDetail | null;
  groupDetail: GroupDetail | null;
  selectedEntries: EntryRef[];
  selectedEntriesMeta?: GalleryEntry[];
  status: VaultStatus;
  allTags: TagWithCount[];
  allFolders: FolderNode[];
  currentFolderId: string | null;
  inTrash: boolean;
  hidePreview?: boolean;
  readOnly?: boolean;
  isWebClient?: boolean;
  onClose?: () => void;
  onUpdateTitle: (id: string, title: string) => Promise<void>;
  onUpdateRating: (id: string, rating: number) => Promise<void>;
  onUpdateMemo: (id: string, memo: string) => Promise<void>;
  onUpdateSourceUrl?: (id: string, sourceUrl: string) => Promise<void>;
  onCopyToast?: (message: string) => void;
  onAddTag: (id: string, tagName: string) => Promise<void>;
  onRemoveTag: (id: string, tagName: string) => Promise<void>;
  onBatchSetRating?: (rating: number) => Promise<void>;
  batchRatingBusy?: boolean;
  onBatchUpdateTags?: (addTags: string[], removeTags: string[]) => Promise<void>;
  onOpenBulkTagDialog?: () => void;
  onAddToFolder: (folderId: string) => Promise<void>;
  onRemoveFromFolder: (folderId: string) => Promise<void>;
  onOpenLightbox?: (itemId: string) => void;
  onOpenReader?: (groupId: string) => void;
  onOpenEditor?: (groupId: string) => void;
  onUngroup?: () => Promise<void>;
  onCreateGroupFromSelection?: () => Promise<void>;
  onPlainExport?: () => void;
  onMoveToTrash: () => Promise<void>;
  onRestoreFromTrash: () => Promise<void>;
  onPermanentlyDelete: () => Promise<void>;
}

export function Inspector(props: InspectorProps) {
  const {
    mode,
    itemDetail,
    groupDetail,
    selectedEntries,
    selectedEntriesMeta,
    status,
    allTags,
    allFolders,
    currentFolderId,
    inTrash,
    hidePreview = false,
    readOnly = false,
    isWebClient = false,
    onClose,
    onUpdateTitle,
    onUpdateRating,
    onUpdateMemo,
    onUpdateSourceUrl,
    onCopyToast,
    onAddTag,
    onRemoveTag,
    onBatchSetRating,
    batchRatingBusy = false,
    onBatchUpdateTags,
    onOpenBulkTagDialog,
    onAddToFolder,
    onRemoveFromFolder,
    onOpenLightbox,
    onOpenReader,
    onOpenEditor,
    onUngroup,
    onCreateGroupFromSelection,
    onPlainExport,
    onMoveToTrash,
    onRestoreFromTrash,
    onPermanentlyDelete,
  } = props;

  const confirmDialogRef = useRef<HTMLDialogElement>(null);
  const openConfirmPermDelete = () => confirmDialogRef.current?.showModal();

  const renderPermDeleteDialog = () => (
    <dialog
      ref={confirmDialogRef}
      className="confirm-dialog"
      aria-labelledby="perm-delete-title"
      onClose={(e) => {
        const dlg = e.currentTarget;
        const val = dlg.returnValue;
        dlg.returnValue = "";
        if (val === "delete") {
          void onPermanentlyDelete();
        }
      }}
    >
      <div className="confirm-dialog-inner">
        <h2 id="perm-delete-title">完全に削除しますか？</h2>
        <p>
          暗号化保管庫から対象の画像とサムネイルを復元できない形で完全に消去します。この操作は取り消せません。
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
  );

  if (mode === "multi") {
    return (
      <>
        <InspectorMultiView
          selectedEntries={selectedEntries}
          selectedEntriesMeta={selectedEntriesMeta}
          allTags={allTags}
          allFolders={allFolders}
          currentFolderId={currentFolderId}
          inTrash={inTrash}
          readOnly={readOnly}
          isWebClient={isWebClient}
          onClose={onClose}
          onBatchSetRating={onBatchSetRating}
          batchRatingBusy={batchRatingBusy}
          onBatchUpdateTags={onBatchUpdateTags}
          onOpenBulkTagDialog={onOpenBulkTagDialog}
          onAddToFolder={onAddToFolder}
          onRemoveFromFolder={onRemoveFromFolder}
          onCreateGroupFromSelection={onCreateGroupFromSelection}
          onPlainExport={onPlainExport}
          onMoveToTrash={onMoveToTrash}
          onRestoreFromTrash={onRestoreFromTrash}
          onRequestPermDelete={openConfirmPermDelete}
        />
        {renderPermDeleteDialog()}
      </>
    );
  }

  const activeEntity = mode === "image" ? itemDetail : mode === "group" ? groupDetail : null;
  if (!activeEntity || (mode !== "image" && mode !== "group")) {
    return (
      <InspectorEmptyView
        status={status}
        readOnly={readOnly}
        isWebClient={isWebClient}
        onClose={onClose}
      />
    );
  }

  return (
    <>
      <InspectorDetailView
        mode={mode}
        itemDetail={itemDetail}
        groupDetail={groupDetail}
        allTags={allTags}
        allFolders={allFolders}
        currentFolderId={currentFolderId}
        inTrash={inTrash}
        hidePreview={hidePreview}
        readOnly={readOnly}
        isWebClient={isWebClient}
        onClose={onClose}
        onUpdateTitle={onUpdateTitle}
        onUpdateRating={onUpdateRating}
        onUpdateMemo={onUpdateMemo}
        onUpdateSourceUrl={onUpdateSourceUrl}
        onCopyToast={onCopyToast}
        onAddTag={onAddTag}
        onRemoveTag={onRemoveTag}
        onAddToFolder={onAddToFolder}
        onRemoveFromFolder={onRemoveFromFolder}
        onOpenLightbox={onOpenLightbox}
        onOpenReader={onOpenReader}
        onOpenEditor={onOpenEditor}
        onUngroup={onUngroup}
        onPlainExport={onPlainExport}
        onMoveToTrash={onMoveToTrash}
        onRestoreFromTrash={onRestoreFromTrash}
        onRequestPermDelete={openConfirmPermDelete}
      />
      {renderPermDeleteDialog()}
    </>
  );
}
