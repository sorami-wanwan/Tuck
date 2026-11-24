import { useMemo, useState } from "react";
import { Download, Folder, FolderMinus, Layers, Plus, RotateCcw, Trash2, X } from "lucide-react";
import {
  isVideoMime,
  type EntryRef,
  type FolderNode,
  type GalleryEntry,
  type TagWithCount,
} from "../../types/vault";
import { formatSelectionRatingStatus, summarizeSelectionRating } from "../../utils/bulkRating";
import { isImeComposing, normalizeTagKey, summarizeSelectionTags } from "../../utils/bulkTags";
import { BatchRatingControl } from "../BatchRatingControl";

export interface InspectorMultiViewProps {
  selectedEntries: EntryRef[];
  selectedEntriesMeta?: GalleryEntry[];
  allTags: TagWithCount[];
  allFolders: FolderNode[];
  currentFolderId: string | null;
  inTrash: boolean;
  readOnly: boolean;
  isWebClient: boolean;
  onClose?: () => void;
  onBatchSetRating?: (rating: number) => Promise<void>;
  batchRatingBusy?: boolean;
  onBatchUpdateTags?: (addTags: string[], removeTags: string[]) => Promise<void>;
  onOpenBulkTagDialog?: () => void;
  onAddToFolder: (folderId: string) => Promise<void>;
  onRemoveFromFolder: (folderId: string) => Promise<void>;
  onCreateGroupFromSelection?: () => Promise<void>;
  onPlainExport?: () => void;
  onMoveToTrash: () => Promise<void>;
  onRestoreFromTrash: () => Promise<void>;
  onRequestPermDelete: () => void;
}

export function InspectorMultiView({
  selectedEntries,
  selectedEntriesMeta,
  allTags,
  allFolders,
  currentFolderId,
  inTrash,
  readOnly,
  isWebClient,
  onClose,
  onBatchSetRating,
  batchRatingBusy = false,
  onBatchUpdateTags,
  onOpenBulkTagDialog,
  onAddToFolder,
  onRemoveFromFolder,
  onCreateGroupFromSelection,
  onPlainExport,
  onMoveToTrash,
  onRestoreFromTrash,
  onRequestPermDelete,
}: InspectorMultiViewProps) {
  const [multiTagDraft, setMultiTagDraft] = useState("");
  const [multiTagFocused, setMultiTagFocused] = useState(false);
  const [multiTagBusy, setMultiTagBusy] = useState(false);

  const multiTagSummary = useMemo(
    () => summarizeSelectionTags(selectedEntriesMeta ?? []),
    [selectedEntriesMeta]
  );

  const multiRatingSummary = useMemo(
    () => summarizeSelectionRating(selectedEntriesMeta ?? []),
    [selectedEntriesMeta]
  );

  const multiSuggestions = useMemo(() => {
    if (!multiTagFocused) return [];
    const allAttached = new Set(
      multiTagSummary.filter((s) => s.state === "all").map((s) => s.key)
    );
    const q = normalizeTagKey(multiTagDraft);
    return allTags
      .filter((t) => !allAttached.has(normalizeTagKey(t.name)))
      .filter((t) => (q ? normalizeTagKey(t.name).includes(q) : true))
      .slice(0, 8);
  }, [multiTagFocused, multiTagSummary, allTags, multiTagDraft]);

  const runMultiTagUpdate = async (
    addTags: string[],
    removeTags: string[],
    clearDraft = false
  ) => {
    if (!onBatchUpdateTags || multiTagBusy) return;
    setMultiTagBusy(true);
    try {
      await onBatchUpdateTags(addTags, removeTags);
      if (clearDraft) setMultiTagDraft("");
    } catch {
      // Failure toast is handled by the caller; keep multiTagDraft intact for retry.
    } finally {
      setMultiTagBusy(false);
    }
  };

  const standaloneImageCount = selectedEntriesMeta
    ? selectedEntriesMeta.filter((e) => e.kind === "image" && !isVideoMime(e.coverMimeType)).length
    : selectedEntries.filter((e) => e.kind === "image").length;
  const standaloneVideoCount = selectedEntriesMeta
    ? selectedEntriesMeta.filter((e) => e.kind === "image" && isVideoMime(e.coverMimeType)).length
    : 0;
  const selectedGroupCount = selectedEntries.filter((e) => e.kind === "group").length;

  const summaryParts = [
    `単体画像 ${standaloneImageCount}件`,
    ...(standaloneVideoCount > 0 ? [`動画 ${standaloneVideoCount}件`] : []),
    `漫画グループ ${selectedGroupCount}件`,
  ];

  return (
    <div className="inspector-scroll">
      {onClose && (
        <div className="inspector-drawer-bar">
          <span>詳細情報</span>
          <button
            type="button"
            className="icon-btn"
            aria-label="詳細パネルを閉じる"
            onClick={onClose}
          >
            <X size={15} />
          </button>
        </div>
      )}
      <div
        style={{
          padding: "16px 14px",
          borderRadius: "var(--radius-md)",
          background: "var(--bg-elevated)",
          border: "1px solid var(--border-subtle)",
          display: "flex",
          flexDirection: "column",
          gap: 10,
        }}
      >
        <div style={{ fontWeight: 600 }}>{selectedEntries.length}件を選択中</div>
        <div style={{ fontSize: 12, color: "var(--text-secondary)" }}>
          {summaryParts.join(" / ")}
        </div>
      </div>

      {!readOnly && !inTrash && standaloneImageCount >= 2 && onCreateGroupFromSelection && (
        <button
          type="button"
          className="btn btn-primary"
          onClick={() => void onCreateGroupFromSelection()}
        >
          <Layers size={13} />
          漫画グループにまとめる ({standaloneImageCount}枚)
        </button>
      )}

      <div className="inspector-field-group">
        <div className="inspector-section-header">
          <span className="section-label">評価</span>
          <div style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
            <span className="multi-rating-status tabular-nums">
              {formatSelectionRatingStatus(multiRatingSummary)}
            </span>
            {!readOnly && !inTrash && onBatchSetRating && multiRatingSummary.ratedCount > 0 && (
              <button
                type="button"
                className="inspector-inline-link"
                disabled={batchRatingBusy}
                onClick={() => void onBatchSetRating(0)}
              >
                解除
              </button>
            )}
          </div>
        </div>
        <div className="multi-rating-panel">
          <BatchRatingControl
            summary={multiRatingSummary}
            disabled={readOnly || inTrash || !onBatchSetRating || batchRatingBusy}
            onSelectRating={(rating) => void onBatchSetRating?.(rating)}
          />
        </div>
      </div>

      <div className="inspector-field-group">
        <div className="inspector-section-header">
          <span className="section-label">タグ</span>
          {!readOnly && !inTrash && onOpenBulkTagDialog && (
            <button
              type="button"
              className="inspector-inline-link"
              onClick={onOpenBulkTagDialog}
            >
              まとめて編集...
            </button>
          )}
        </div>
        <div className="tag-chips-wrap">
          {multiTagSummary.length === 0 && (readOnly || inTrash) && (
            <span style={{ fontSize: 11, color: "var(--text-muted)" }}>タグなし</span>
          )}
          {multiTagSummary.map((tag) =>
            tag.state === "all" ? (
              <span key={tag.key} className="tag-chip">
                <span>{tag.name}</span>
                {!readOnly && !inTrash && onBatchUpdateTags && (
                  <button
                    type="button"
                    className="tag-chip-remove"
                    disabled={multiTagBusy}
                    title={`${tag.name} を全項目から削除`}
                    aria-label={`${tag.name} を全項目から削除`}
                    onClick={() => void runMultiTagUpdate([], [tag.name])}
                  >
                    <X size={11} />
                  </button>
                )}
              </span>
            ) : (
              <span
                key={tag.key}
                className="tag-chip tag-chip-partial"
                title={`${tag.total}件中 ${tag.count}件に付与中`}
              >
                <span>{tag.name}</span>
                <span className="tag-chip-ratio tabular-nums">
                  {tag.count}/{tag.total}
                </span>
                {!readOnly && !inTrash && onBatchUpdateTags && (
                  <>
                    <button
                      type="button"
                      className="tag-chip-apply"
                      disabled={multiTagBusy}
                      title="全項目に揃える"
                      aria-label={`${tag.name} を全項目に揃える`}
                      onClick={() => void runMultiTagUpdate([tag.name], [])}
                    >
                      <Plus size={11} />
                    </button>
                    <button
                      type="button"
                      className="tag-chip-remove"
                      disabled={multiTagBusy}
                      title="全項目から外す"
                      aria-label={`${tag.name} を全項目から削除`}
                      onClick={() => void runMultiTagUpdate([], [tag.name])}
                    >
                      <X size={11} />
                    </button>
                  </>
                )}
              </span>
            )
          )}
        </div>

        {!readOnly && !inTrash && onBatchUpdateTags && (
          <div
            className="tag-autocomplete"
            onBlur={(e) => {
              if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
                setMultiTagFocused(false);
              }
            }}
          >
            <input
              type="text"
              className="inspector-input"
              aria-label="選択中の全項目にタグを追加"
              placeholder={`全 ${selectedEntries.length}件にタグを追加...`}
              value={multiTagDraft}
              disabled={multiTagBusy}
              onFocus={() => setMultiTagFocused(true)}
              onChange={(e) => setMultiTagDraft(e.target.value)}
              onKeyDown={(e) => {
                if (isImeComposing(e)) return;
                if (e.key === "Enter") {
                  e.preventDefault();
                  const trimmed = multiTagDraft.normalize("NFC").trim();
                  if (trimmed) {
                    void runMultiTagUpdate([trimmed], [], true);
                  }
                } else if (e.key === "Escape") {
                  setMultiTagDraft("");
                  setMultiTagFocused(false);
                  e.currentTarget.blur();
                }
              }}
            />
            {multiSuggestions.length > 0 && (
              <div className="tag-suggestions" role="listbox" aria-label="既存タグ候補">
                {multiSuggestions.map((sug) => (
                  <button
                    key={sug.id}
                    type="button"
                    className="tag-suggestion-btn"
                    disabled={multiTagBusy}
                    onClick={() => void runMultiTagUpdate([sug.name], [], true)}
                  >
                    <span>{sug.name}</span>
                    <span className="nav-item-count tabular-nums">{sug.count}</span>
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      {!readOnly && !inTrash && allFolders.length > 0 && (
        <div className="inspector-field-group">
          <span className="section-label">フォルダへ一括追加</span>
          <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            {allFolders.map((f) => (
              <button
                key={f.id}
                type="button"
                className="nav-item"
                onClick={() => void onAddToFolder(f.id)}
              >
                <Folder size={13} style={{ color: f.color || "var(--text-secondary)" }} />
                <span className="nav-item-label">{f.name}</span>
              </button>
            ))}
          </div>
        </div>
      )}

      {(!readOnly || (!inTrash && Boolean(onPlainExport))) && (
        <div className="inspector-actions" style={{ flexDirection: "column" }}>
          {inTrash ? (
            !readOnly && (
              <>
                <button
                  type="button"
                  className="btn btn-primary"
                  onClick={() => void onRestoreFromTrash()}
                >
                  <RotateCcw size={13} />
                  選択項目を復元
                </button>
                <button
                  type="button"
                  className="btn btn-danger"
                  onClick={onRequestPermDelete}
                >
                  <Trash2 size={13} />
                  完全に削除
                </button>
              </>
            )
          ) : (
            <>
              {onPlainExport && (
                <button type="button" className="btn" onClick={onPlainExport}>
                  <Download size={13} />
                  {isWebClient
                    ? "選択項目を ZIP でダウンロード"
                    : "選択項目をファイルとして書き出し..."}
                </button>
              )}
              {!readOnly && currentFolderId && (
                <button
                  type="button"
                  className="btn"
                  onClick={() => void onRemoveFromFolder(currentFolderId)}
                >
                  <FolderMinus size={13} />
                  このフォルダから外す（本体は残る）
                </button>
              )}
              {!readOnly && (
                <button type="button" className="btn btn-danger" onClick={() => void onMoveToTrash()}>
                  <Trash2 size={13} />
                  ゴミ箱へ移動
                </button>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}
