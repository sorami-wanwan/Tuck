import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import {
  BookOpen,
  Copy,
  Download,
  Folder,
  FolderMinus,
  Layers,
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
import { imageMemoryCache } from "../services/imageCache";
import {
  formatDurationMs,
  isVideoMime,
  type EntryRef,
  type FolderNode,
  type GalleryEntry,
  type GroupDetail,
  type ItemDetail,
  type TagWithCount,
  type VaultStatus,
} from "../types/vault";
import { formatBytes } from "../utils/format";
import { formatShortcutLabel } from "../utils/formatShortcut";

interface InspectorProps {
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

function formatTimestamp(ms: number): string {
  const d = new Date(ms);
  const yyyy = d.getFullYear();
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  const hh = String(d.getHours()).padStart(2, "0");
  const min = String(d.getMinutes()).padStart(2, "0");
  return `${yyyy}/${mm}/${dd} ${hh}:${min}`;
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

  const activeEntity = mode === "image" ? itemDetail : mode === "group" ? groupDetail : null;
  const activeId = activeEntity?.id ?? null;

  const [titleDraft, setTitleDraft] = useState("");
  const [memoDraft, setMemoDraft] = useState("");
  const [sourceUrlDraft, setSourceUrlDraft] = useState("");
  const [tagDraft, setTagDraft] = useState("");
  const [tagInputOpen, setTagInputOpen] = useState(false);
  const [folderPickerOpen, setFolderPickerOpen] = useState(false);
  const [previewUrl, setPreviewUrl] = useState<string | undefined>(undefined);
  const confirmDialogRef = useRef<HTMLDialogElement>(null);
  const memoTimerRef = useRef<number | null>(null);

  useEffect(() => {
    setTitleDraft(activeEntity?.title ?? "");
    setMemoDraft(activeEntity?.memo ?? "");
    setSourceUrlDraft(mode === "image" && itemDetail ? itemDetail.sourceUrl : "");
    setTagDraft("");
    setTagInputOpen(false);
    setFolderPickerOpen(false);
    if (memoTimerRef.current !== null) {
      window.clearTimeout(memoTimerRef.current);
      memoTimerRef.current = null;
    }
  }, [activeId]);

  useEffect(() => {
    if (activeEntity) {
      setTitleDraft(activeEntity.title);
      setMemoDraft(activeEntity.memo);
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
      : mode === "group"
        ? groupDetail?.pages[0]?.itemId
        : undefined;

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
  }, [previewCoverItemId, hidePreview]);

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
    const existing = new Set(activeEntity.tags.map((t) => t.toLowerCase()));
    const q = tagDraft.trim().toLowerCase();
    return allTags
      .filter((t) => !existing.has(t.name.toLowerCase()))
      .filter((t) => (q ? t.name.toLowerCase().includes(q) : true))
      .slice(0, 8);
  }, [activeEntity, allTags, tagDraft, tagInputOpen]);

  const standaloneImageCount = selectedEntriesMeta
    ? selectedEntriesMeta.filter((e) => e.kind === "image" && !isVideoMime(e.coverMimeType)).length
    : selectedEntries.filter((e) => e.kind === "image").length;
  const standaloneVideoCount = selectedEntriesMeta
    ? selectedEntriesMeta.filter((e) => e.kind === "image" && isVideoMime(e.coverMimeType)).length
    : 0;
  const selectedGroupCount = selectedEntries.filter((e) => e.kind === "group").length;

  if (mode === "multi") {
    const summaryParts = [
      `単体画像 ${standaloneImageCount}件`,
      ...(standaloneVideoCount > 0 ? [`動画 ${standaloneVideoCount}件`] : []),
      `漫画グループ ${selectedGroupCount}件`,
    ];
    return (
      <>
        <div className="inspector-scroll">
          {onClose && (
            <div className="inspector-drawer-bar">
              <span>詳細情報</span>
              <button type="button" className="icon-btn" aria-label="詳細パネルを閉じる" onClick={onClose}>
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

          {!readOnly && !inTrash && standaloneSelectedActions()}

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
                    <button type="button" className="btn btn-primary" onClick={() => void onRestoreFromTrash()}>
                      <RotateCcw size={13} />
                      すべて復元
                    </button>
                    <button
                      type="button"
                      className="btn btn-danger"
                      onClick={() => confirmDialogRef.current?.showModal()}
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
        {renderPermDeleteDialog()}
      </>
    );
  }

  function standaloneSelectedActions() {
    if (standaloneImageCount < 2 || !onCreateGroupFromSelection) return null;
    return (
      <button
        type="button"
        className="btn btn-primary"
        onClick={() => void onCreateGroupFromSelection()}
      >
        <Layers size={13} />
        漫画グループにまとめる ({standaloneImageCount}枚)
      </button>
    );
  }

  function renderPermDeleteDialog() {
    return (
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
  }

  if (!activeEntity) {
    return (
      <div className="inspector-scroll">
        {onClose && (
          <div className="inspector-drawer-bar">
            <span>保管庫の概要</span>
            <button type="button" className="icon-btn" aria-label="詳細パネルを閉じる" onClick={onClose}>
              <X size={15} />
            </button>
          </div>
        )}
        <div
          style={{
            padding: "20px 14px",
            borderRadius: "var(--radius-md)",
            background: "var(--bg-elevated)",
            border: "1px solid var(--border-subtle)",
            display: "flex",
            flexDirection: "column",
            gap: 8,
          }}
        >
          <div style={{ fontWeight: 600 }}>保管庫の概要</div>
          <div className="info-table">
            <div className="info-row">
              <span className="info-key">登録数</span>
              <span className="info-val">{status.itemCount} 件</span>
            </div>
            <div className="info-row">
              <span className="info-key">未分類</span>
              <span className="info-val">{status.uncategorizedCount} 件</span>
            </div>
            <div className="info-row">
              <span className="info-key">合計容量</span>
              <span className="info-val">{formatBytes(status.totalBytes)}</span>
            </div>
          </div>
        </div>

        <div style={{ color: "var(--text-muted)", fontSize: 12, lineHeight: 1.7 }}>
          <div style={{ fontWeight: 600, color: "var(--text-secondary)", marginBottom: 4 }}>
            操作ヒント
          </div>
          {isWebClient ? (
            <>
              <div>・タップ / クリック：画像・動画・漫画を開く</div>
              <div>・2本指ピンチ：拡大・縮小</div>
              <div>・左右スワイプ：前後のページ / 画像へ移動</div>
              <div>・{readOnly ? "現在は閲覧専用モードです" : "LAN 編集モードで整理・アップロード可"}</div>
            </>
          ) : (
            <>
              <div>・Cmd/Ctrl + クリック：複数選択</div>
              <div>・フォルダへドラッグ：フォルダに追加</div>
              <div>・{formatShortcutLabel(status.lockSettings.shortcut)}：即時ロック</div>
              <div>・Cmd/Ctrl + V：クリップボードから取り込み</div>
            </>
          )}
        </div>
      </div>
    );
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
    memoTimerRef.current = window.setTimeout(() => {
      memoTimerRef.current = null;
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
    if (e.key === "Enter") {
      e.preventDefault();
      handleTagSubmit(tagDraft);
    } else if (e.key === "Escape") {
      setTagInputOpen(false);
      setTagDraft("");
    }
  };

  return (
    <>
      <div className="inspector-scroll">
        {onClose && (
          <div className="inspector-drawer-bar">
            <span>詳細情報</span>
            <button type="button" className="icon-btn" aria-label="詳細パネルを閉じる" onClick={onClose}>
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
              if (e.key === "Enter") e.currentTarget.blur();
            }}
          />
          <textarea
            className="inspector-textarea"
            aria-label="メモ"
            disabled={inTrash}
            readOnly={readOnly}
            placeholder={readOnly ? "メモなし" : "新しいメモ..."}
            value={memoDraft}
            onChange={(e) => {
              if (!readOnly) handleMemoChange(e.target.value);
            }}
            onBlur={() => {
              if (readOnly) return;
              if (memoTimerRef.current !== null) {
                window.clearTimeout(memoTimerRef.current);
                memoTimerRef.current = null;
              }
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
                  if (e.key === "Enter") e.currentTarget.blur();
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
                  onClick={() => confirmDialogRef.current?.showModal()}
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

      {renderPermDeleteDialog()}
    </>
  );
}
