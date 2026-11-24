import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import {
  BookOpen,
  Copy,
  Download,
  Folder,
  FolderMinus,
  ListOrdered,
  Maximize2,
  Play,
  Plus,
  RotateCcw,
  Star,
  Trash2,
  Unlink,
  X,
} from "lucide-react";
import { imageMemoryCache } from "../../services/imageCache";
import {
  formatDurationMs,
  isVideoMime,
  type FolderNode,
  type GroupDetail,
  type ItemDetail,
  type TagWithCount,
} from "../../types/vault";
import { formatBytes } from "../../utils/format";
import { isImeComposing, normalizeTagKey } from "../../utils/bulkTags";

export interface InspectorDetailViewProps {
  mode: "image" | "group";
  itemDetail: ItemDetail | null;
  groupDetail: GroupDetail | null;
  allTags: TagWithCount[];
  allFolders: FolderNode[];
  currentFolderId: string | null;
  inTrash: boolean;
  hidePreview: boolean;
  readOnly: boolean;
  isWebClient: boolean;
  onClose?: () => void;
  onUpdateTitle: (id: string, title: string) => Promise<void>;
  onUpdateRating: (id: string, rating: number) => Promise<void>;
  onUpdateMemo: (id: string, memo: string) => Promise<void>;
  onUpdateSourceUrl?: (id: string, sourceUrl: string) => Promise<void>;
  onCopyToast?: (message: string) => void;
  onAddTag: (id: string, tagName: string) => Promise<void>;
  onRemoveTag: (id: string, tagName: string) => Promise<void>;
  onAddToFolder: (folderId: string) => Promise<void>;
  onRemoveFromFolder: (folderId: string) => Promise<void>;
  onOpenLightbox?: (itemId: string) => void;
  onOpenReader?: (groupId: string) => void;
  onOpenEditor?: (groupId: string) => void;
  onUngroup?: () => Promise<void>;
  onPlainExport?: () => void;
  onMoveToTrash: () => Promise<void>;
  onRestoreFromTrash: () => Promise<void>;
  onRequestPermDelete: () => void;
}

function formatTimestamp(ms: number): string {
  const d = new Date(ms);
  const yyyy = d.getFullYear();
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  const hh = String(d.getHours()).padStart(2, "0");
  const min = String(d.getMinutes()).padStart(2, "0");
  return `${yyyy}/${mm}/${dd} ${hh}:${min}`;
}

export function InspectorDetailView({
  mode,
  itemDetail,
  groupDetail,
  allTags,
  allFolders,
  currentFolderId,
  inTrash,
  hidePreview,
  readOnly,
  isWebClient,
  onClose,
  onUpdateTitle,
  onUpdateRating,
  onUpdateMemo,
  onUpdateSourceUrl,
  onCopyToast,
  onAddTag,
  onRemoveTag,
  onAddToFolder,
  onRemoveFromFolder,
  onOpenLightbox,
  onOpenReader,
  onOpenEditor,
  onUngroup,
  onPlainExport,
  onMoveToTrash,
  onRestoreFromTrash,
  onRequestPermDelete,
}: InspectorDetailViewProps) {
  const activeEntity = mode === "image" ? itemDetail : groupDetail;
  const activeId = activeEntity?.id ?? null;

  const [titleDraft, setTitleDraft] = useState(activeEntity?.title ?? "");
  const [memoDraft, setMemoDraft] = useState(activeEntity?.memo ?? "");
  const [sourceUrlDraft, setSourceUrlDraft] = useState(
    mode === "image" && itemDetail ? itemDetail.sourceUrl : ""
  );
  const [tagDraft, setTagDraft] = useState("");
  const [tagInputOpen, setTagInputOpen] = useState(false);
  const [folderPickerOpen, setFolderPickerOpen] = useState(false);
  const [previewUrl, setPreviewUrl] = useState<string | undefined>(undefined);

  const memoTimerRef = useRef<number | null>(null);
  const memoFocusedRef = useRef(false);
  const pendingMemoRef = useRef<{ id: string; value: string } | null>(null);
  const onUpdateMemoRef = useRef(onUpdateMemo);
  onUpdateMemoRef.current = onUpdateMemo;

  useEffect(() => {
    setTitleDraft(activeEntity?.title ?? "");
    setMemoDraft(activeEntity?.memo ?? "");
    setSourceUrlDraft(mode === "image" && itemDetail ? itemDetail.sourceUrl : "");
    setTagDraft("");
    setTagInputOpen(false);
    setFolderPickerOpen(false);
    memoFocusedRef.current = false;
    return () => {
      if (memoTimerRef.current !== null) {
        window.clearTimeout(memoTimerRef.current);
        memoTimerRef.current = null;
      }
      const pending = pendingMemoRef.current;
      if (pending) {
        pendingMemoRef.current = null;
        void onUpdateMemoRef.current(pending.id, pending.value);
      }
    };
  }, [activeId]);

  useEffect(() => {
    if (activeEntity) {
      setTitleDraft(activeEntity.title);
      if (!memoFocusedRef.current) {
        setMemoDraft(activeEntity.memo);
      }
    }
  }, [activeEntity?.title, activeEntity?.memo]);

  useEffect(() => {
    if (mode === "image" && itemDetail) {
      setSourceUrlDraft(itemDetail.sourceUrl);
    }
  }, [mode, itemDetail?.sourceUrl]);

  const previewCoverItemId =
    mode === "image"
      ? itemDetail?.id
      : groupDetail?.pages[0]?.itemId;

  useEffect(() => {
    if (!previewCoverItemId || hidePreview) {
      setPreviewUrl(undefined);
      return;
    }
    let active = true;
    const cached = imageMemoryCache.getCachedThumbnailUrl(previewCoverItemId);
    if (cached) {
      setPreviewUrl(cached);
      return;
    }
    setPreviewUrl(undefined);
    void imageMemoryCache
      .getThumbnailUrl(previewCoverItemId)
      .then((u) => {
        if (active) setPreviewUrl(u);
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, [previewCoverItemId, hidePreview, itemDetail?.hasThumbnail, groupDetail?.pages[0]?.hasThumbnail]);

  const folderMap = useMemo(() => {
    const map = new Map<string, FolderNode>();
    for (const f of allFolders) map.set(f.id, f);
    return map;
  }, [allFolders]);

  const availableFolders = useMemo(() => {
    if (!activeEntity) return allFolders;
    const attached = new Set(activeEntity.folderIds);
    return allFolders.filter((f) => !attached.has(f.id));
  }, [allFolders, activeEntity]);

  const suggestions = useMemo(() => {
    if (!activeEntity || !tagInputOpen) return [];
    const existing = new Set(activeEntity.tags.map(normalizeTagKey));
    const q = normalizeTagKey(tagDraft);
    return allTags
      .filter((t) => !existing.has(normalizeTagKey(t.name)))
      .filter((t) => (q ? normalizeTagKey(t.name).includes(q) : true))
      .slice(0, 8);
  }, [activeEntity, allTags, tagDraft, tagInputOpen]);

  if (!activeEntity) {
    return null;
  }

  const commitTitle = () => {
    const trimmed = titleDraft.trim();
    if (trimmed !== activeEntity.title) {
      void onUpdateTitle(activeEntity.id, trimmed);
    }
  };

  const commitMemo = (nextMemo: string) => {
    if (nextMemo !== activeEntity.memo) {
      void onUpdateMemo(activeEntity.id, nextMemo);
    }
  };

  const commitSourceUrl = () => {
    if (mode !== "image" || !itemDetail || !onUpdateSourceUrl) return;
    const trimmed = sourceUrlDraft.trim();
    if (trimmed !== itemDetail.sourceUrl) {
      void onUpdateSourceUrl(itemDetail.id, trimmed);
    }
  };

  const isSingleVideo = mode === "image" && itemDetail !== null && isVideoMime(itemDetail.mimeType);
  const formatBadgeLabel =
    mode === "group" && groupDetail
      ? `${groupDetail.pageCount}P 漫画`
      : itemDetail?.mimeType === "video/quicktime"
        ? "MOV"
        : itemDetail?.mimeType.replace("image/", "").replace("video/", "").toUpperCase();

  const handleMemoChange = (val: string) => {
    setMemoDraft(val);
    if (memoTimerRef.current !== null) {
      window.clearTimeout(memoTimerRef.current);
    }
    const targetId = activeEntity.id;
    pendingMemoRef.current = { id: targetId, value: val };
    memoTimerRef.current = window.setTimeout(() => {
      memoTimerRef.current = null;
      pendingMemoRef.current = null;
      void onUpdateMemo(targetId, val);
    }, 450);
  };

  const handleTagSubmit = (tagName: string) => {
    const trimmed = tagName.trim();
    if (!trimmed) return;
    setTagDraft("");
    void onAddTag(activeEntity.id, trimmed);
  };

  const handleTagKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (isImeComposing(e)) return;
    if (e.key === "Enter") {
      e.preventDefault();
      handleTagSubmit(tagDraft);
    } else if (e.key === "Escape") {
      setTagInputOpen(false);
      setTagDraft("");
    }
  };

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
      {!hidePreview && (
        <div
          className="inspector-preview-box"
          title={
            mode === "group"
              ? "クリックして漫画を読む"
              : isSingleVideo
                ? "クリックして動画を再生"
                : "クリックして拡大表示"
          }
          onClick={() => {
            if (mode === "group" && groupDetail) onOpenReader?.(groupDetail.id);
            else if (itemDetail) onOpenLightbox?.(itemDetail.id);
          }}
        >
          {previewUrl ? (
            <img src={previewUrl} alt={activeEntity.title || "プレビュー"} draggable={false} />
          ) : (
            <div className="thumb-skeleton" />
          )}
          <span className="format-badge inspector-preview-badge">{formatBadgeLabel}</span>
        </div>
      )}

      {activeEntity.palette.length > 0 && (
        <div className="palette-row" aria-label="抽出カラーパレット">
          {activeEntity.palette.map((hex) => (
            <span
              key={hex}
              className="palette-swatch"
              style={{ backgroundColor: hex }}
              title={hex}
            />
          ))}
        </div>
      )}

      <div className="inspector-field-group">
        <input
          type="text"
          className="inspector-input"
          aria-label="タイトル"
          disabled={inTrash}
          readOnly={readOnly}
          placeholder={readOnly ? "タイトルなし" : "タイトルを追加..."}
          value={titleDraft}
          onChange={(e) => {
            if (!readOnly) setTitleDraft(e.target.value);
          }}
          onBlur={() => {
            if (!readOnly) commitTitle();
          }}
          onKeyDown={(e) => {
            if (!isImeComposing(e) && e.key === "Enter") e.currentTarget.blur();
          }}
        />
        <textarea
          className="inspector-textarea"
          aria-label="メモ"
          disabled={inTrash}
          readOnly={readOnly}
          placeholder={readOnly ? "メモなし" : "新しいメモ..."}
          value={memoDraft}
          onFocus={() => {
            memoFocusedRef.current = true;
          }}
          onChange={(e) => {
            if (!readOnly) handleMemoChange(e.target.value);
          }}
          onBlur={() => {
            memoFocusedRef.current = false;
            if (readOnly) return;
            if (memoTimerRef.current !== null) {
              window.clearTimeout(memoTimerRef.current);
              memoTimerRef.current = null;
            }
            pendingMemoRef.current = null;
            commitMemo(memoDraft);
          }}
        />
        {mode === "image" && itemDetail && (!readOnly || sourceUrlDraft.trim().length > 0) && (
          <div className="input-with-icon">
            <input
              type="url"
              className="inspector-input"
              style={{ paddingInlineEnd: 30 }}
              aria-label="出典URL"
              disabled={inTrash}
              readOnly={readOnly}
              placeholder="出典URL..."
              value={sourceUrlDraft}
              onChange={(e) => {
                if (!readOnly) setSourceUrlDraft(e.target.value);
              }}
              onBlur={() => {
                if (!readOnly) commitSourceUrl();
              }}
              onKeyDown={(e) => {
                if (!isImeComposing(e) && e.key === "Enter") e.currentTarget.blur();
              }}
            />
            {sourceUrlDraft.trim() && (
              <button
                type="button"
                className="input-trailing-btn"
                title="出典URLをコピー"
                aria-label="出典URLをコピー"
                onClick={() => {
                  void navigator.clipboard
                    ?.writeText(sourceUrlDraft.trim())
                    .then(() => onCopyToast?.("出典URLをコピーしました"))
                    .catch(() => {});
                }}
              >
                <Copy size={13} />
              </button>
            )}
          </div>
        )}
      </div>

      <div className="inspector-field-group">
        <span className="section-label">タグ</span>
        <div className="tag-chips-wrap">
          {activeEntity.tags.length === 0 && readOnly && (
            <span style={{ fontSize: 11, color: "var(--text-muted)" }}>タグなし</span>
          )}
          {activeEntity.tags.map((tag) => (
            <span key={tag} className="tag-chip">
              {tag}
              {!inTrash && !readOnly && (
                <button
                  type="button"
                  className="tag-chip-remove"
                  aria-label={`${tag} を削除`}
                  onClick={() => void onRemoveTag(activeEntity.id, tag)}
                >
                  <X size={11} />
                </button>
              )}
            </span>
          ))}
          {!inTrash && !readOnly && (
            <button
              type="button"
              className="icon-btn"
              style={{ inlineSize: 24, blockSize: 24, background: "var(--bg-elevated)" }}
              title="タグを追加"
              aria-label="タグを追加"
              onClick={() => setTagInputOpen((v) => !v)}
            >
              <Plus size={13} />
            </button>
          )}
        </div>

        {tagInputOpen && !inTrash && !readOnly && (
          <div className="tag-autocomplete">
            <input
              type="text"
              className="inspector-input"
              aria-label="新しいタグ名"
              placeholder="タグを入力して Enter..."
              value={tagDraft}
              autoFocus
              onChange={(e) => setTagDraft(e.target.value)}
              onKeyDown={handleTagKeyDown}
            />
            {suggestions.length > 0 && (
              <div className="tag-suggestions" role="listbox" aria-label="既存タグ候補">
                {suggestions.map((sug) => (
                  <button
                    key={sug.id}
                    type="button"
                    className="tag-suggestion-btn"
                    onClick={() => handleTagSubmit(sug.name)}
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

      <div className="inspector-field-group">
        <span className="section-label">フォルダ</span>
        <div className="tag-chips-wrap">
          {activeEntity.folderIds.length === 0 && readOnly && (
            <span style={{ fontSize: 11, color: "var(--text-muted)" }}>未所属</span>
          )}
          {activeEntity.folderIds.map((fid) => {
            const f = folderMap.get(fid);
            if (!f) return null;
            return (
              <span key={fid} className="tag-chip">
                <Folder size={11} style={{ color: f.color || "var(--text-secondary)" }} />
                {f.name}
                {!inTrash && !readOnly && (
                  <button
                    type="button"
                    className="tag-chip-remove"
                    aria-label={`${f.name} から外す`}
                    onClick={() => void onRemoveFromFolder(fid)}
                  >
                    <X size={11} />
                  </button>
                )}
              </span>
            );
          })}
          {!inTrash && !readOnly && allFolders.length > 0 && (
            <button
              type="button"
              className="icon-btn"
              style={{ inlineSize: 24, blockSize: 24, background: "var(--bg-elevated)" }}
              title="フォルダに追加"
              aria-label="フォルダに追加"
              onClick={() => setFolderPickerOpen((v) => !v)}
            >
              <Plus size={13} />
            </button>
          )}
        </div>

        {folderPickerOpen && !inTrash && !readOnly && (
          <div style={{ position: "relative" }}>
            <div className="tag-suggestions" style={{ position: "static", maxBlockSize: 130 }}>
              {availableFolders.length === 0 ? (
                <div style={{ padding: "4px 8px", fontSize: 11, color: "var(--text-muted)" }}>
                  追加できるフォルダがありません
                </div>
              ) : (
                availableFolders.map((f) => (
                  <button
                    key={f.id}
                    type="button"
                    className="tag-suggestion-btn"
                    onClick={() => {
                      setFolderPickerOpen(false);
                      void onAddToFolder(f.id);
                    }}
                  >
                    <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                      <Folder size={12} style={{ color: f.color || "var(--text-secondary)" }} />
                      {f.name}
                    </span>
                  </button>
                ))
              )}
            </div>
          </div>
        )}
      </div>

      <div className="inspector-field-group">
        <span className="section-label">インフォメーション</span>
        <div className="info-table">
          <div className="info-row">
            <span className="info-key">評価</span>
            <span className="star-row" role="group" aria-label="星評価">
              {[1, 2, 3, 4, 5].map((star) => {
                const filled = star <= activeEntity.rating;
                return (
                  <button
                    key={star}
                    type="button"
                    disabled={inTrash || readOnly}
                    className={`star-btn ${filled ? "filled" : ""}`}
                    aria-label={`星 ${star}`}
                    aria-pressed={activeEntity.rating === star}
                    onClick={() => {
                      if (readOnly) return;
                      void onUpdateRating(
                        activeEntity.id,
                        activeEntity.rating === star ? 0 : star
                      );
                    }}
                  >
                    <Star size={14} fill={filled ? "currentColor" : "none"} />
                  </button>
                );
              })}
            </span>
          </div>

          {mode === "group" && groupDetail ? (
            <>
              <div className="info-row">
                <span className="info-key">ページ数</span>
                <span className="info-val">{groupDetail.pageCount} ページ</span>
              </div>
              <div className="info-row">
                <span className="info-key">綴じ方向</span>
                <span className="info-val">
                  {groupDetail.readingDir === "rtl" ? "右綴じ" : "左綴じ"}
                </span>
              </div>
              <div className="info-row">
                <span className="info-key">合計サイズ</span>
                <span className="info-val">{formatBytes(groupDetail.totalBytes)}</span>
              </div>
            </>
          ) : itemDetail ? (
            <>
              <div className="info-row">
                <span className="info-key">寸法</span>
                <span className="info-val">
                  {itemDetail.width > 0 ? `${itemDetail.width}×${itemDetail.height}` : "取得中..."}
                </span>
              </div>
              {isSingleVideo && (
                <div className="info-row">
                  <span className="info-key">長さ</span>
                  <span className="info-val tabular-nums">
                    {itemDetail.durationMs > 0
                      ? formatDurationMs(itemDetail.durationMs)
                      : "取得中..."}
                  </span>
                </div>
              )}
              <div className="info-row">
                <span className="info-key">サイズ</span>
                <span className="info-val">{formatBytes(itemDetail.byteSize)}</span>
              </div>
              <div className="info-row">
                <span className="info-key">タイプ</span>
                <span className="info-val">{formatBadgeLabel}</span>
              </div>
            </>
          ) : null}

          <div className="info-row">
            <span className="info-key">追加日</span>
            <span className="info-val">{formatTimestamp(activeEntity.createdAt)}</span>
          </div>
          <div className="info-row">
            <span className="info-key">更新日</span>
            <span className="info-val">{formatTimestamp(activeEntity.updatedAt)}</span>
          </div>
        </div>
      </div>

      <div className="inspector-actions" style={{ flexDirection: "column" }}>
        {inTrash ? (
          !readOnly && (
            <div style={{ display: "flex", gap: 8 }}>
              <button
                type="button"
                className="btn btn-primary btn-block"
                onClick={() => void onRestoreFromTrash()}
              >
                <RotateCcw size={13} />
                復元
              </button>
              <button
                type="button"
                className="btn btn-danger"
                onClick={onRequestPermDelete}
              >
                <Trash2 size={13} />
                完全に削除
              </button>
            </div>
          )
        ) : (
          <>
            <div style={{ display: "flex", gap: 8 }}>
              {mode === "group" && groupDetail ? (
                <>
                  {onOpenReader && (
                    <button
                      type="button"
                      className="btn btn-primary btn-block"
                      onClick={() => onOpenReader(groupDetail.id)}
                    >
                      <BookOpen size={13} />
                      漫画を読む
                    </button>
                  )}
                  {!readOnly && onOpenEditor && (
                    <button
                      type="button"
                      className="btn"
                      title="ページ構成を編集"
                      aria-label="ページ構成を編集"
                      onClick={() => onOpenEditor(groupDetail.id)}
                    >
                      <ListOrdered size={13} />
                    </button>
                  )}
                </>
              ) : (
                itemDetail &&
                onOpenLightbox && (
                  <button
                    type="button"
                    className="btn btn-block"
                    onClick={() => onOpenLightbox(itemDetail.id)}
                  >
                    {isSingleVideo ? <Play size={13} /> : <Maximize2 size={13} />}
                    {isSingleVideo ? "動画を再生" : "拡大表示"}
                  </button>
                )
              )}

              {!readOnly && (
                <button
                  type="button"
                  className="btn btn-danger"
                  title="ゴミ箱へ移動"
                  aria-label="ゴミ箱へ移動"
                  onClick={() => void onMoveToTrash()}
                >
                  <Trash2 size={13} />
                </button>
              )}
            </div>

            {onPlainExport && (
              <button type="button" className="btn" onClick={onPlainExport}>
                <Download size={13} />
                {isWebClient
                  ? mode === "group"
                    ? "ZIP でダウンロード"
                    : "ダウンロード"
                  : mode === "group"
                    ? "フォルダとして書き出し..."
                    : "ファイルとして書き出し..."}
              </button>
            )}

            {!readOnly && mode === "group" && onUngroup && (
              <button type="button" className="btn" onClick={() => void onUngroup()}>
                <Unlink size={13} />
                グループを解除（単体画像に戻す）
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
          </>
        )}
      </div>
    </div>
  );
}
