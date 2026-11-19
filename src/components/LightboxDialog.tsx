import { useEffect, useRef, useState } from "react";
import {
  Camera,
  ChevronLeft,
  ChevronRight,
  Download,
  PanelRight,
  Repeat,
  RotateCcw,
  X,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import { useViewportPhysics } from "../hooks/useViewportPhysics";
import { imageMemoryCache } from "../services/imageCache";
import {
  isVideoMime,
  type EntryRef,
  type FolderNode,
  type GalleryEntry,
  type ItemDetail,
  type TagWithCount,
  type VaultStatus,
} from "../types/vault";
import { Inspector } from "./Inspector";

interface LightboxDialogProps {
  open: boolean;
  items: GalleryEntry[];
  currentId: string | null;
  detail: ItemDetail | null;
  status: VaultStatus;
  allTags: TagWithCount[];
  allFolders: FolderNode[];
  currentFolderId: string | null;
  readOnly?: boolean;
  isWebClient?: boolean;
  onClose: () => void;
  onNavigate: (itemId: string) => void;
  onUpdateTitle: (itemId: string, title: string) => Promise<void>;
  onUpdateRating: (itemId: string, rating: number) => Promise<void>;
  onUpdateMemo: (itemId: string, memo: string) => Promise<void>;
  onUpdateSourceUrl: (itemId: string, sourceUrl: string) => Promise<void>;
  onCopyToast?: (message: string) => void;
  onAddTag: (itemId: string, tagName: string) => Promise<void>;
  onRemoveTag: (itemId: string, tagName: string) => Promise<void>;
  onAddToFolder: (folderId: string, itemId: string) => Promise<void>;
  onRemoveFromFolder: (folderId: string, itemId: string) => Promise<void>;
  onPlainExport?: (item: GalleryEntry) => void;
  regeneratingThumbnail?: boolean;
  onRegenerateThumbnails?: (entries: EntryRef[]) => void;
  onCaptureVideoThumbnail?: (
    itemId: string,
    videoEl: HTMLVideoElement | null
  ) => Promise<void>;
  onMoveToTrash: (itemId: string) => Promise<void>;
  onRestoreFromTrash: (itemId: string) => Promise<void>;
  onPermanentlyDelete: (itemId: string) => Promise<void>;
}

const MAX_LAYOUT_EDGE = 4096;

export function LightboxDialog({
  open,
  items,
  currentId,
  detail,
  status,
  allTags,
  allFolders,
  currentFolderId,
  readOnly = false,
  isWebClient = false,
  onClose,
  onNavigate,
  onUpdateTitle,
  onUpdateRating,
  onUpdateMemo,
  onUpdateSourceUrl,
  onCopyToast,
  onAddTag,
  onRemoveTag,
  onAddToFolder,
  onRemoveFromFolder,
  onPlainExport,
  regeneratingThumbnail = false,
  onRegenerateThumbnails,
  onCaptureVideoThumbnail,
  onMoveToTrash,
  onRestoreFromTrash,
  onPermanentlyDelete,
}: LightboxDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const [imageUrl, setImageUrl] = useState<string | undefined>(undefined);
  const [videoUrl, setVideoUrl] = useState<string | undefined>(undefined);
  const [videoError, setVideoError] = useState(false);
  const [videoFallbackBlobUrl, setVideoFallbackBlobUrl] = useState<string | null>(null);
  const [videoLoop, setVideoLoop] = useState(false);
  const [naturalSize, setNaturalSize] = useState<{ width: number; height: number } | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [hudHidden, setHudHidden] = useState(false);

  const imageItems = items.filter((e) => e.kind === "image");
  const currentIndex = currentId ? imageItems.findIndex((i) => i.id === currentId) : -1;
  const currentItem = currentIndex >= 0 ? imageItems[currentIndex] : null;
  const prevItem = currentIndex > 0 ? imageItems[currentIndex - 1] : null;
  const nextItem =
    currentIndex >= 0 && currentIndex < imageItems.length - 1
      ? imageItems[currentIndex + 1]
      : null;
  const isVideo = Boolean(currentItem && isVideoMime(currentItem.coverMimeType));

  useEffect(() => {
    const dlg = dialogRef.current;
    if (!dlg) return;
    if (open && !dlg.open) {
      dlg.showModal();
    } else if (!open && dlg.open) {
      dlg.close();
    }
  }, [open]);

  useEffect(() => {
    if (currentItem && currentItem.width > 0 && currentItem.height > 0) {
      setNaturalSize({ width: currentItem.width, height: currentItem.height });
    } else {
      setNaturalSize(null);
    }
  }, [currentId, open]);

  useEffect(() => {
    return () => {
      if (videoFallbackBlobUrl) {
        URL.revokeObjectURL(videoFallbackBlobUrl);
      }
    };
  }, [videoFallbackBlobUrl]);

  useEffect(() => {
    setVideoError(false);
    if (!open || !currentItem) {
      setImageUrl(undefined);
      setVideoUrl(undefined);
      setVideoFallbackBlobUrl((prev) => {
        if (prev) URL.revokeObjectURL(prev);
        return null;
      });
      return;
    }

    if (prevItem && !isVideoMime(prevItem.coverMimeType)) {
      imageMemoryCache.prefetchFullImage(prevItem.id, prevItem.coverMimeType);
    }
    if (nextItem && !isVideoMime(nextItem.coverMimeType)) {
      imageMemoryCache.prefetchFullImage(nextItem.id, nextItem.coverMimeType);
    }

    if (isVideoMime(currentItem.coverMimeType)) {
      setImageUrl(undefined);
      setVideoFallbackBlobUrl((prev) => {
        if (prev) URL.revokeObjectURL(prev);
        return null;
      });
      setVideoLoop(currentItem.durationMs > 0 && currentItem.durationMs <= 15_000);
      setVideoUrl(imageMemoryCache.getVaultMediaStreamUrl(currentItem.id));
      return;
    }

    setVideoUrl(undefined);
    let active = true;
    setImageUrl(undefined);
    void imageMemoryCache
      .getFullImageUrl(currentItem.id, currentItem.coverMimeType)
      .then((url) => {
        if (active) setImageUrl(url);
      })
      .catch(() => {});

    return () => {
      active = false;
    };
  }, [open, currentItem?.id, currentItem?.coverMimeType, prevItem?.id, nextItem?.id]);

  useEffect(() => {
    const v = videoRef.current;
    return () => {
      if (v) {
        v.pause();
        v.removeAttribute("src");
        v.load();
      }
    };
  }, [open, currentItem?.id]);

  const layoutScale = naturalSize
    ? Math.min(1, MAX_LAYOUT_EDGE / Math.max(naturalSize.width, naturalSize.height, 1))
    : 1;
  const contentWidth = naturalSize ? Math.max(1, Math.round(naturalSize.width * layoutScale)) : 1;
  const contentHeight = naturalSize ? Math.max(1, Math.round(naturalSize.height * layoutScale)) : 1;

  useEffect(() => {
    if (!open) setHudHidden(false);
  }, [open]);

  const physics = useViewportPhysics({
    viewportRef,
    stageRef,
    contentWidth,
    contentHeight,
    maxFitScale: 1,
    enabled: open && !isVideo && Boolean(imageUrl && naturalSize),
    resetKey: `${currentId ?? ""}:${open}`,
    onSwipeLeft: () => nextItem && onNavigate(nextItem.id),
    onSwipeRight: () => prevItem && onNavigate(prevItem.id),
    onTapZone: (zone) => {
      if (zone === "left") {
        if (prevItem) onNavigate(prevItem.id);
      } else if (zone === "right") {
        if (nextItem) onNavigate(nextItem.id);
      } else {
        setHudHidden((v) => !v);
      }
    },
  });

  useEffect(() => {
    if (!open) return;
    const handleKey = (e: globalThis.KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable)
      ) {
        return;
      }

      if (e.key === "ArrowLeft" && prevItem) {
        e.preventDefault();
        onNavigate(prevItem.id);
      } else if (e.key === "ArrowRight" && nextItem) {
        e.preventDefault();
        onNavigate(nextItem.id);
      } else if (e.key === " " && isVideo && videoRef.current) {
        if (target?.tagName === "BUTTON") return;
        e.preventDefault();
        if (videoRef.current.paused) void videoRef.current.play();
        else videoRef.current.pause();
      } else if (!readOnly && e.key >= "1" && e.key <= "5" && currentItem) {
        e.preventDefault();
        void onUpdateRating(currentItem.id, Number(e.key));
      } else if (!readOnly && e.key === "0" && currentItem) {
        e.preventDefault();
        void onUpdateRating(currentItem.id, 0);
      } else if (e.key === "i" || e.key === "I") {
        e.preventDefault();
        setDrawerOpen((v) => !v);
      } else if (!isVideo && (e.key === "+" || e.key === "=")) {
        e.preventDefault();
        physics.zoomIn();
      } else if (!isVideo && e.key === "-") {
        e.preventDefault();
        physics.zoomOut();
      } else if (!isVideo && (e.key === "f" || e.key === "F")) {
        e.preventDefault();
        physics.resetFit();
      }
    };

    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [
    open,
    isVideo,
    readOnly,
    prevItem,
    nextItem,
    currentItem,
    onNavigate,
    onUpdateRating,
    physics.zoomIn,
    physics.zoomOut,
    physics.resetFit,
  ]);

  const effectivePercent = Math.round(physics.hud.scale * layoutScale * 100);

  return (
    <dialog
      ref={dialogRef}
      className="lightbox-dialog"
      aria-label={currentItem?.title || (isVideo ? "動画プレーヤー" : "拡大画像ビューア")}
      onClose={(e) => {
        e.currentTarget.returnValue = "";
        if (videoRef.current) {
          videoRef.current.pause();
        }
        onClose();
      }}
    >
      <div className={`lightbox-stage ${!isVideo && hudHidden ? "hud-hidden" : ""}`}>
        <div className="lightbox-hud">
          <div className="lightbox-hud-left">
            <span className="lightbox-title">
              {currentItem?.title || (isVideo ? "無題の動画" : "無題の画像")}
            </span>
            {currentIndex >= 0 && (
              <span className="toolbar-count tabular-nums">
                {currentIndex + 1} / {imageItems.length}
              </span>
            )}
          </div>

          <div className="lightbox-controls">
            {isVideo ? (
              <>
                <button
                  type="button"
                  className={`pill-btn ${videoLoop ? "active" : ""}`}
                  style={{ display: "inline-flex", alignItems: "center", gap: 4 }}
                  title="ループ再生を切替"
                  aria-pressed={videoLoop}
                  onClick={() => setVideoLoop((v) => !v)}
                >
                  <Repeat size={12} />
                  ループ再生
                </button>
                {!readOnly &&
                  currentItem &&
                  currentItem.deletedAt === null &&
                  onCaptureVideoThumbnail && (
                    <button
                      type="button"
                      className="pill-btn"
                      style={{ display: "inline-flex", alignItems: "center", gap: 4 }}
                      title="現在表示している動画フレームをサムネイルに設定"
                      disabled={regeneratingThumbnail || videoError || !videoUrl}
                      onClick={() =>
                        void onCaptureVideoThumbnail(currentItem.id, videoRef.current)
                      }
                    >
                      <Camera size={12} />
                      {regeneratingThumbnail ? "生成中..." : "現在のフレームをサムネイルに"}
                    </button>
                  )}
              </>
            ) : (
              <>
                <button
                  type="button"
                  className="icon-btn"
                  title="縮小 (-)"
                  aria-label="縮小"
                  onClick={physics.zoomOut}
                >
                  <ZoomOut size={15} />
                </button>

                <button
                  type="button"
                  className="pill-btn tabular-nums"
                  style={{ minInlineSize: 64 }}
                  title="クリックでフィット ↔ 原寸切替"
                  onClick={physics.toggleActualSize}
                >
                  {physics.hud.isFit ? "フィット" : `${effectivePercent}%`}
                </button>

                <button
                  type="button"
                  className="icon-btn"
                  title="拡大 (+)"
                  aria-label="拡大"
                  onClick={physics.zoomIn}
                >
                  <ZoomIn size={15} />
                </button>

                <button
                  type="button"
                  className="icon-btn"
                  title="画面に合わせる (F)"
                  aria-label="画面に合わせる"
                  onClick={physics.resetFit}
                >
                  <RotateCcw size={14} />
                </button>
              </>
            )}

            {currentItem && currentItem.deletedAt === null && onPlainExport && (
              <button
                type="button"
                className="icon-btn"
                title={isWebClient ? "ダウンロード" : "通常のファイルとして書き出し..."}
                aria-label={isWebClient ? "ダウンロード" : "通常のファイルとして書き出し"}
                onClick={() => onPlainExport(currentItem)}
              >
                <Download size={15} />
              </button>
            )}

            <button
              type="button"
              className={`icon-btn ${drawerOpen ? "active" : ""}`}
              title="詳細パネル (I)"
              aria-label="詳細パネルを開閉"
              onClick={() => setDrawerOpen((v) => !v)}
            >
              <PanelRight size={15} />
            </button>

            <button
              type="button"
              className="icon-btn"
              title="閉じる (Esc)"
              aria-label="拡大表示を閉じる"
              onClick={() => dialogRef.current?.close()}
            >
              <X size={16} />
            </button>
          </div>
        </div>

        <div
          ref={viewportRef}
          className={`lightbox-viewport ${!isVideo ? "image-mode" : ""} ${
            !isVideo && physics.hud.canPan ? "pannable" : ""
          } ${!isVideo && physics.isDragging ? "dragging" : ""}`}
          {...(isVideo ? {} : physics.handlers)}
        >
          {isVideo && currentItem ? (
            videoError ? (
              <div className="video-thumb-placeholder" style={{ padding: 24 }}>
                <span>この動画を再生できませんでした</span>
              </div>
            ) : videoUrl ? (
              <video
                ref={videoRef}
                key={currentItem.id}
                src={videoUrl}
                controls
                playsInline
                preload="metadata"
                crossOrigin={isWebClient ? undefined : "anonymous"}
                loop={videoLoop}
                width={currentItem.width > 0 ? currentItem.width : undefined}
                height={currentItem.height > 0 ? currentItem.height : undefined}
                className="lightbox-video"
                onError={() => {
                  if (
                    !imageMemoryCache.usesHttpMediaStream() &&
                    !videoUrl.startsWith("blob:") &&
                    currentItem.byteSize <= 32 * 1024 * 1024
                  ) {
                    const targetId = currentItem.id;
                    const targetMime = currentItem.coverMimeType;
                    void imageMemoryCache
                      .getFullImageUrl(targetId, targetMime)
                      .then((blobUrl) => {
                        setVideoUrl(blobUrl);
                      })
                      .catch(() => {
                        setVideoError(true);
                      });
                    return;
                  }
                  setVideoError(true);
                }}
              />
            ) : (
              <span style={{ color: "var(--text-secondary)" }}>動画を読み込み中...</span>
            )
          ) : imageUrl ? (
            <div
              ref={stageRef}
              className="physics-stage"
              style={{
                inlineSize: naturalSize ? contentWidth : undefined,
                blockSize: naturalSize ? contentHeight : undefined,
                visibility: naturalSize ? "visible" : "hidden",
              }}
            >
              <img
                src={imageUrl}
                alt={currentItem?.title || "拡大画像"}
                draggable={false}
                className="lightbox-image"
                onLoad={(e) => {
                  const nw = e.currentTarget.naturalWidth;
                  const nh = e.currentTarget.naturalHeight;
                  if (nw > 0 && nh > 0) {
                    setNaturalSize((prev) =>
                      prev && prev.width === nw && prev.height === nh
                        ? prev
                        : { width: nw, height: nh }
                    );
                  }
                }}
                style={
                  naturalSize
                    ? { inlineSize: contentWidth, blockSize: contentHeight }
                    : undefined
                }
              />
            </div>
          ) : (
            <span style={{ color: "var(--text-secondary)" }}>読み込み中...</span>
          )}

          <button
            type="button"
            className="lightbox-nav-btn prev"
            disabled={!prevItem}
            aria-label="前のアイテム"
            onClick={() => prevItem && onNavigate(prevItem.id)}
          >
            <ChevronLeft size={22} />
          </button>

          <button
            type="button"
            className="lightbox-nav-btn next"
            disabled={!nextItem}
            aria-label="次のアイテム"
            onClick={() => nextItem && onNavigate(nextItem.id)}
          >
            <ChevronRight size={22} />
          </button>
        </div>
      </div>

      {drawerOpen && currentItem && (
        <aside className="lightbox-drawer" aria-label="詳細パネル">
          <Inspector
            mode="image"
            itemDetail={detail}
            groupDetail={null}
            selectedEntries={[{ kind: "image", id: currentItem.id }]}
            status={status}
            allTags={allTags}
            allFolders={allFolders}
            currentFolderId={currentFolderId}
            inTrash={currentItem.deletedAt !== null}
            hidePreview
            readOnly={readOnly}
            isWebClient={isWebClient}
            onClose={() => setDrawerOpen(false)}
            onUpdateTitle={onUpdateTitle}
            onUpdateRating={onUpdateRating}
            onUpdateMemo={onUpdateMemo}
            onUpdateSourceUrl={onUpdateSourceUrl}
            onCopyToast={onCopyToast}
            onAddTag={onAddTag}
            onRemoveTag={onRemoveTag}
            onAddToFolder={(fid) => onAddToFolder(fid, currentItem.id)}
            onRemoveFromFolder={(fid) => onRemoveFromFolder(fid, currentItem.id)}
            onPlainExport={onPlainExport ? () => onPlainExport(currentItem) : undefined}
            regeneratingThumbnails={regeneratingThumbnail}
            onRegenerateThumbnails={
              onRegenerateThumbnails
                ? () => onRegenerateThumbnails([{ kind: "image", id: currentItem.id }])
                : undefined
            }
            onMoveToTrash={() => onMoveToTrash(currentItem.id)}
            onRestoreFromTrash={() => onRestoreFromTrash(currentItem.id)}
            onPermanentlyDelete={() => onPermanentlyDelete(currentItem.id)}
          />
        </aside>
      )}
    </dialog>
  );
}
