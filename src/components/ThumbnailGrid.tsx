import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  BookOpen,
  Download,
  Film,
  FolderMinus,
  ImagePlus,
  Layers,
  Maximize2,
  Play,
  RefreshCw,
  RotateCcw,
  Star,
  Trash2,
  X,
} from "lucide-react";
import type { DropTarget } from "../hooks/useInternalDrag";
import { imageMemoryCache } from "../services/imageCache";
import {
  entryKey,
  formatDurationMs,
  isVideoMime,
  type GalleryEntry,
  type ThumbFitMode,
} from "../types/vault";

interface ThumbnailGridProps {
  entries: GalleryEntry[];
  selectedKeys: Set<string>;
  primaryKey: string | null;
  thumbSize: number;
  fitMode: ThumbFitMode;
  isDraggingOsFiles: boolean;
  isTrashView: boolean;
  currentFolderId: string | null;
  hoverDropTarget: DropTarget;
  canEdit?: boolean;
  isWebClient?: boolean;
  isTouchDevice?: boolean;
  selectionMode?: boolean;
  onCardClick: (entry: GalleryEntry, e: MouseEvent) => void;
  onSelectAll: () => void;
  onClearSelection: () => void;
  onSelectSingle: (entry: GalleryEntry) => void;
  onOpenEntry: (entry: GalleryEntry) => void;
  onCardContextMenu: (entry: GalleryEntry, x: number, y: number) => void;
  onBeginEntryDrag: (entry: GalleryEntry, x: number, y: number) => void;
  onImportFiles: () => void;
  onAvifCompleted: () => void;
  onCreateGroupFromSelection: () => void;
  onRemoveSelectionFromFolder: () => void;
  onPlainExportSelection?: () => void;
  regeneratingThumbnails?: boolean;
  onRegenerateSelectionThumbnails?: () => void;
  onMoveSelectionToTrash: () => void;
  onRestoreAllTrash: () => void;
  onEmptyTrash: () => void;
}

const GRID_GAP = 12;
const GRID_PADDING = 28;

function shortFormatLabel(mimeType: string): string {
  switch (mimeType) {
    case "image/png":
      return "PNG";
    case "image/jpeg":
      return "JPG";
    case "image/webp":
      return "WEBP";
    case "image/gif":
      return "GIF";
    case "image/avif":
      return "AVIF";
    case "video/mp4":
      return "MP4";
    case "video/webm":
      return "WEBM";
    case "video/quicktime":
      return "MOV";
    default:
      return isVideoMime(mimeType) ? "VIDEO" : "IMG";
  }
}

interface ThumbCardProps {
  entry: GalleryEntry;
  selected: boolean;
  dropHighlight: boolean;
  fitMode: ThumbFitMode;
  cardHeight: number;
  canEdit: boolean;
  isTouchDevice: boolean;
  selectionMode: boolean;
  onClick: (e: MouseEvent) => void;
  onOpen: () => void;
  onContextMenu: (e: MouseEvent) => void;
  onPointerDown: (clientX: number, clientY: number) => void;
  onAvifCompleted: () => void;
}

function ThumbCard({
  entry,
  selected,
  dropHighlight,
  fitMode,
  cardHeight,
  canEdit,
  isTouchDevice,
  selectionMode,
  onClick,
  onOpen,
  onContextMenu,
  onPointerDown,
  onAvifCompleted,
}: ThumbCardProps) {
  const coverId = entry.coverItemId;
  const [thumbVer, setThumbVer] = useState<number>(() =>
    imageMemoryCache.getThumbnailVersion(coverId)
  );
  const [url, setUrl] = useState<string | undefined>(() =>
    imageMemoryCache.getCachedThumbnailUrl(coverId)
  );
  const [failed, setFailed] = useState(false);
  const onAvifCompletedRef = useRef(onAvifCompleted);
  onAvifCompletedRef.current = onAvifCompleted;

  useEffect(() => {
    setThumbVer(imageMemoryCache.getThumbnailVersion(coverId));
    return imageMemoryCache.subscribeThumbnailChange(coverId, () => {
      setThumbVer(imageMemoryCache.getThumbnailVersion(coverId));
    });
  }, [coverId]);

  useEffect(() => {
    let active = true;
    setFailed(false);
    const cached = imageMemoryCache.getCachedThumbnailUrl(coverId);
    if (cached) {
      setUrl(cached);
      return;
    }
    setUrl(undefined);

    const load = async () => {
      try {
        const hasThumb =
          entry.coverHasThumbnail || imageMemoryCache.hasKnownThumbnail(coverId);
        if (!hasThumb) {
          if (!canEdit) {
            if (active) setFailed(true);
            return;
          }
          if (
            (entry.coverMimeType === "image/avif" || isVideoMime(entry.coverMimeType)) &&
            imageMemoryCache.hasPendingThumbnailFailed(coverId)
          ) {
            if (active) setFailed(true);
            return;
          }
          if (entry.coverMimeType === "image/avif") {
            await imageMemoryCache.processPendingAvifThumbnail(coverId);
            if (active) onAvifCompletedRef.current();
          } else if (isVideoMime(entry.coverMimeType)) {
            await imageMemoryCache.processPendingVideoThumbnail(coverId, entry.coverMimeType);
            if (active) onAvifCompletedRef.current();
          }
        }
        const nextUrl = await imageMemoryCache.getThumbnailUrl(coverId);
        if (active) setUrl(nextUrl);
      } catch {
        if (active) setFailed(true);
      }
    };

    void load();
    return () => {
      active = false;
    };
  }, [coverId, entry.coverHasThumbnail, entry.coverMimeType, canEdit, thumbVer]);

  const isVideo = entry.kind === "image" && isVideoMime(entry.coverMimeType);
  const durationLabel = isVideo ? formatDurationMs(entry.durationMs) : "";
  const showFormatBadge =
    entry.kind === "image" &&
    (entry.coverMimeType === "image/gif" || entry.coverMimeType === "image/avif" || isVideo);

  return (
    <div
      role="option"
      aria-selected={selected}
      data-drop-group-id={entry.kind === "group" && entry.deletedAt === null ? entry.id : undefined}
      className={`thumb-card ${entry.kind === "group" ? "is-group" : ""} ${
        selected ? "selected" : ""
      } ${dropHighlight ? "drop-target-active" : ""}`}
      style={{ blockSize: cardHeight }}
      onClick={(e) => {
        if (isTouchDevice && !selectionMode) {
          onOpen();
          return;
        }
        onClick(e);
      }}
      onDoubleClick={onOpen}
      onContextMenu={onContextMenu}
      onPointerDown={(e) => {
        if (!canEdit || e.pointerType === "touch" || e.button !== 0) return;
        if ((e.target as HTMLElement).closest("button")) return;
        onPointerDown(e.clientX, e.clientY);
      }}
    >
      <div className="thumb-media-box">
        {url ? (
          <img
            src={url}
            alt={entry.title || (isVideo ? "動画" : "画像")}
            draggable={false}
            className={`thumb-img ${fitMode === "square" ? "fit-square" : "fit-ratio"}`}
          />
        ) : failed ? (
          <div className="video-thumb-placeholder">
            {isVideo && <Film size={22} />}
            <span>{shortFormatLabel(entry.coverMimeType)}</span>
          </div>
        ) : (
          <div className="thumb-skeleton" />
        )}
      </div>

      <div className="thumb-overlay-top">
        {entry.kind === "group" ? (
          <span className="manga-badge">
            <BookOpen size={10} />
            {entry.pageCount}P
          </span>
        ) : showFormatBadge ? (
          <span className="format-badge" style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
            {isVideo && <Film size={10} />}
            {shortFormatLabel(entry.coverMimeType)}
          </span>
        ) : (
          <span />
        )}

        <button
          type="button"
          className="thumb-expand-btn"
          title={entry.kind === "group" ? "漫画を読む" : isVideo ? "動画を再生" : "拡大表示"}
          aria-label={entry.kind === "group" ? "漫画を読む" : isVideo ? "動画を再生" : "拡大表示"}
          onClick={(e) => {
            e.stopPropagation();
            onOpen();
          }}
        >
          {entry.kind === "group" ? (
            <BookOpen size={13} />
          ) : isVideo ? (
            <Play size={13} />
          ) : (
            <Maximize2 size={13} />
          )}
        </button>
      </div>

      {(entry.title || entry.rating > 0 || durationLabel) && (
        <div className="thumb-overlay-bottom">
          <span className="thumb-title">{entry.title}</span>
          <div style={{ display: "inline-flex", alignItems: "center", gap: 4, flexShrink: 0 }}>
            {durationLabel && (
              <span className="thumb-duration-badge tabular-nums">{durationLabel}</span>
            )}
            {entry.rating > 0 && (
              <span className="thumb-stars" aria-label={`評価 ${entry.rating}`}>
                <Star size={10} fill="currentColor" />
                <span className="tabular-nums">{entry.rating}</span>
              </span>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

export function ThumbnailGrid(props: ThumbnailGridProps) {
  const {
    entries,
    selectedKeys,
    primaryKey,
    thumbSize,
    fitMode,
    isDraggingOsFiles,
    isTrashView,
    currentFolderId,
    hoverDropTarget,
    canEdit = true,
    isWebClient = false,
    isTouchDevice = false,
    selectionMode = false,
    onCardClick,
    onSelectAll,
    onClearSelection,
    onSelectSingle,
    onOpenEntry,
    onCardContextMenu,
    onBeginEntryDrag,
    onImportFiles,
    onAvifCompleted,
    onCreateGroupFromSelection,
    onRemoveSelectionFromFolder,
    onPlainExportSelection,
    regeneratingThumbnails = false,
    onRegenerateSelectionThumbnails,
    onMoveSelectionToTrash,
    onRestoreAllTrash,
    onEmptyTrash,
  } = props;

  const scrollRef = useRef<HTMLDivElement>(null);
  const emptyTrashDialogRef = useRef<HTMLDialogElement>(null);
  const [containerWidth, setContainerWidth] = useState(900);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const observer = new ResizeObserver((items) => {
      const entry = items[0];
      if (entry) setContainerWidth(entry.contentRect.width);
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const { columns, cardWidth, cardHeight, rowCount } = useMemo(() => {
    const usable = Math.max(200, containerWidth - GRID_PADDING);
    const effectiveThumb =
      containerWidth < 520
        ? Math.min(thumbSize, Math.max(120, Math.floor((usable - GRID_GAP) / 2)))
        : thumbSize;
    const cols = Math.max(1, Math.floor((usable + GRID_GAP) / (effectiveThumb + GRID_GAP)));
    const w = Math.floor((usable - GRID_GAP * (cols - 1)) / cols);
    const h = fitMode === "square" ? w : Math.round(w * 0.82);
    const rows = Math.ceil(entries.length / cols);
    return { columns: cols, cardWidth: w, cardHeight: h, rowCount: rows };
  }, [containerWidth, thumbSize, fitMode, entries.length]);

  const rowVirtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => cardHeight + GRID_GAP,
    overscan: 3,
  });

  useEffect(() => {
    rowVirtualizer.measure();
  }, [cardHeight, columns, rowVirtualizer]);

  const standaloneSelectedCount = useMemo(
    () =>
      entries.filter(
        (e) =>
          e.kind === "image" &&
          !isVideoMime(e.coverMimeType) &&
          selectedKeys.has(entryKey({ kind: e.kind, id: e.id }))
      ).length,
    [entries, selectedKeys]
  );

  const handleKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a") {
      e.preventDefault();
      onSelectAll();
      return;
    }
    if (entries.length === 0) return;

    const curIndex = primaryKey
      ? entries.findIndex((item) => entryKey({ kind: item.kind, id: item.id }) === primaryKey)
      : -1;

    if ((e.key === "Enter" || e.key === " ") && curIndex >= 0) {
      e.preventDefault();
      onOpenEntry(entries[curIndex]);
      return;
    }

    let nextIndex = curIndex;
    if (e.key === "ArrowRight")
      nextIndex = curIndex < 0 ? 0 : Math.min(entries.length - 1, curIndex + 1);
    else if (e.key === "ArrowLeft") nextIndex = curIndex < 0 ? 0 : Math.max(0, curIndex - 1);
    else if (e.key === "ArrowDown")
      nextIndex = curIndex < 0 ? 0 : Math.min(entries.length - 1, curIndex + columns);
    else if (e.key === "ArrowUp")
      nextIndex = curIndex < 0 ? 0 : Math.max(0, curIndex - columns);
    else return;

    e.preventDefault();
    const target = entries[nextIndex];
    if (target) {
      onSelectSingle(target);
      rowVirtualizer.scrollToIndex(Math.floor(nextIndex / columns), { align: "auto" });
    }
  };

  return (
    <div style={{ position: "relative", flex: 1, minBlockSize: 0, display: "flex", flexDirection: "column" }}>
      {isTrashView && entries.length > 0 && (
        <div className="trash-banner">
          <span style={{ color: "var(--text-secondary)" }}>
            ゴミ箱のアイテムは通常の一覧に表示されません。完全に削除するまで復元できます。
          </span>
          {canEdit && (
            <div style={{ display: "flex", gap: 8 }}>
              <button type="button" className="btn" onClick={onRestoreAllTrash}>
                <RotateCcw size={13} />
                すべて復元
              </button>
              <button
                type="button"
                className="btn btn-danger"
                onClick={() => emptyTrashDialogRef.current?.showModal()}
              >
                <Trash2 size={13} />
                ゴミ箱を空にする
              </button>
            </div>
          )}
        </div>
      )}

      <div
        ref={scrollRef}
        className="grid-scroll"
        tabIndex={0}
        role="listbox"
        aria-multiselectable="true"
        aria-label="画像・漫画一覧"
        onKeyDown={handleKeyDown}
      >
        {entries.length === 0 ? (
          <div className="empty-state">
            <div className="empty-card">
              <ImagePlus size={30} style={{ color: "var(--accent-blue)" }} />
              <h3>{isTrashView ? "ゴミ箱は空です" : "表示するアイテムがありません"}</h3>
              {!isTrashView && canEdit && (
                <>
                  <p>
                    {isWebClient
                      ? "ファイルをドラッグ＆ドロップするか、下のボタンから取り込めます。"
                      : (
                        <>
                          画像やフォルダをドラッグ＆ドロップするか、
                          <br />
                          <code>Cmd+V</code> / <code>Ctrl+V</code> でクリップボードから貼り付けられます。
                        </>
                      )}
                  </p>
                  <button
                    type="button"
                    className="btn btn-primary"
                    style={{ marginBlockStart: 6 }}
                    onClick={onImportFiles}
                  >
                    ファイルを選択して取り込む
                  </button>
                </>
              )}
            </div>
          </div>
        ) : (
          <div
            className="grid-virtual-inner"
            style={{ blockSize: `${rowVirtualizer.getTotalSize()}px` }}
          >
            {rowVirtualizer.getVirtualItems().map((virtualRow) => {
              const startIdx = virtualRow.index * columns;
              const rowItems = entries.slice(startIdx, startIdx + columns);
              return (
                <div
                  key={virtualRow.key}
                  className="grid-row"
                  style={{
                    transform: `translateY(${virtualRow.start}px)`,
                    gridTemplateColumns: `repeat(${columns}, ${cardWidth}px)`,
                  }}
                >
                  {rowItems.map((entry) => {
                    const key = entryKey({ kind: entry.kind, id: entry.id });
                    const isGroupDrop =
                      hoverDropTarget?.type === "group" && hoverDropTarget.groupId === entry.id;
                    return (
                      <ThumbCard
                        key={key}
                        entry={entry}
                        selected={selectedKeys.has(key)}
                        dropHighlight={isGroupDrop}
                        fitMode={fitMode}
                        cardHeight={cardHeight}
                        canEdit={canEdit}
                        isTouchDevice={isTouchDevice}
                        selectionMode={selectionMode}
                        onClick={(e) => onCardClick(entry, e)}
                        onOpen={() => onOpenEntry(entry)}
                        onContextMenu={(e) => {
                          e.preventDefault();
                          onCardContextMenu(entry, e.clientX, e.clientY);
                        }}
                        onPointerDown={(x, y) => onBeginEntryDrag(entry, x, y)}
                        onAvifCompleted={onAvifCompleted}
                      />
                    );
                  })}
                </div>
              );
            })}
          </div>
        )}
      </div>

      {selectedKeys.size >= 2 && !isTrashView && (canEdit || Boolean(onPlainExportSelection)) && (
        <div className="multi-action-bar">
          <span className="tabular-nums" style={{ fontWeight: 600, fontSize: 12, paddingInline: 4 }}>
            {selectedKeys.size}件選択中
          </span>
          {canEdit && standaloneSelectedCount >= 2 && (
            <button type="button" className="btn btn-primary" onClick={onCreateGroupFromSelection}>
              <Layers size={13} />
              漫画グループにまとめる ({standaloneSelectedCount})
            </button>
          )}
          {onPlainExportSelection && (
            <button type="button" className="btn" onClick={onPlainExportSelection}>
              <Download size={13} />
              {isWebClient ? "ZIP でダウンロード" : "ファイルとして書き出し..."}
            </button>
          )}
          {canEdit && onRegenerateSelectionThumbnails && (
            <button
              type="button"
              className="btn"
              disabled={regeneratingThumbnails}
              onClick={onRegenerateSelectionThumbnails}
            >
              <RefreshCw size={13} className={regeneratingThumbnails ? "spin" : ""} />
              サムネイル再生成
            </button>
          )}
          {canEdit && currentFolderId && (
            <button type="button" className="btn" onClick={onRemoveSelectionFromFolder}>
              <FolderMinus size={13} />
              フォルダから外す
            </button>
          )}
          {canEdit && (
            <button type="button" className="btn btn-danger" onClick={onMoveSelectionToTrash}>
              <Trash2 size={13} />
              ゴミ箱へ
            </button>
          )}
          <button
            type="button"
            className="icon-btn"
            aria-label="選択を解除"
            onClick={onClearSelection}
          >
            <X size={14} />
          </button>
        </div>
      )}

      {isDraggingOsFiles && (
        <div className="drop-overlay" aria-hidden="true">
          <div className="drop-overlay-inner">
            <ImagePlus size={36} style={{ color: "var(--accent-blue)" }} />
            <span>ドロップして暗号化保管庫へ取り込む</span>
          </div>
        </div>
      )}

      <dialog
        ref={emptyTrashDialogRef}
        className="confirm-dialog"
        aria-labelledby="empty-trash-title"
        onClose={(e) => {
          const dlg = e.currentTarget;
          const val = dlg.returnValue;
          dlg.returnValue = "";
          if (val === "empty") {
            onEmptyTrash();
          }
        }}
      >
        <div className="confirm-dialog-inner">
          <h2 id="empty-trash-title">ゴミ箱を空にしますか？</h2>
          <p>
            ゴミ箱内のすべての単体画像と漫画グループを保管庫から復元できない形で完全に消去します。この操作は取り消せません。
          </p>
          <form method="dialog" className="confirm-dialog-actions">
            <button type="submit" value="cancel" className="btn" autoFocus>
              キャンセル
            </button>
            <button type="submit" value="empty" className="btn btn-danger">
              完全に削除
            </button>
          </form>
        </div>
      </dialog>
    </div>
  );
}
