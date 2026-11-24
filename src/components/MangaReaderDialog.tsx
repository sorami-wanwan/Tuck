import { useEffect, useMemo, useRef, useState } from "react";
import {
  BookOpen,
  ChevronLeft,
  ChevronRight,
  Columns2,
  Download,
  FileText,
  ListOrdered,
  PanelRight,
  RotateCcw,
  X,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import { useViewportPhysics } from "../hooks/useViewportPhysics";
import { imageMemoryCache } from "../services/imageCache";
import type { FolderNode, GroupDetail, TagWithCount, VaultStatus } from "../types/vault";
import { Inspector } from "./Inspector";

interface MangaReaderDialogProps {
  open: boolean;
  group: GroupDetail | null;
  status: VaultStatus;
  allTags: TagWithCount[];
  allFolders: FolderNode[];
  readOnly?: boolean;
  isWebClient?: boolean;
  onClose: () => void;
  onOpenEditor: (groupId: string) => void;
  onUpdateReaderPrefs: (
    groupId: string,
    readingDir: "rtl" | "ltr",
    spreadMode: "single" | "spread",
    coverAlone: boolean
  ) => Promise<void>;
  onUpdateTitle: (groupId: string, title: string) => Promise<void>;
  onUpdateRating: (groupId: string, rating: number) => Promise<void>;
  onUpdateMemo: (groupId: string, memo: string) => Promise<void>;
  onAddTag: (groupId: string, tagName: string) => Promise<void>;
  onRemoveTag: (groupId: string, tagName: string) => Promise<void>;
  onAddToFolder: (folderId: string, groupId: string) => Promise<void>;
  onRemoveFromFolder: (folderId: string, groupId: string) => Promise<void>;
  onPlainExport?: (group: GroupDetail) => void;
  onUngroup: (groupId: string) => Promise<void>;
  onMoveToTrash: (groupId: string) => Promise<void>;
  onRestoreFromTrash: (groupId: string) => Promise<void>;
  onPermanentlyDelete: (groupId: string) => Promise<void>;
}

const MAX_SPREAD_HEIGHT = 2800;
const MAX_SINGLE_EDGE = 3600;

function buildSpreads(
  totalPages: number,
  spreadMode: "single" | "spread",
  coverAlone: boolean
): number[][] {
  if (totalPages <= 0) return [];
  if (spreadMode === "single") {
    return Array.from({ length: totalPages }, (_, i) => [i]);
  }
  const spreads: number[][] = [];
  let idx = 0;
  if (coverAlone && totalPages > 0) {
    spreads.push([0]);
    idx = 1;
  }
  while (idx < totalPages) {
    if (idx + 1 < totalPages) {
      spreads.push([idx, idx + 1]);
      idx += 2;
    } else {
      spreads.push([idx]);
      idx += 1;
    }
  }
  return spreads;
}

export function MangaReaderDialog({
  open,
  group,
  status,
  allTags,
  allFolders,
  readOnly = false,
  isWebClient = false,
  onClose,
  onOpenEditor,
  onUpdateReaderPrefs,
  onUpdateTitle,
  onUpdateRating,
  onUpdateMemo,
  onAddTag,
  onRemoveTag,
  onAddToFolder,
  onRemoveFromFolder,
  onPlainExport,
  onUngroup,
  onMoveToTrash,
  onRestoreFromTrash,
  onPermanentlyDelete,
}: MangaReaderDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const [pageIdx, setPageIdx] = useState(0);
  const [pageUrls, setPageUrls] = useState<Record<string, string>>({});
  const [naturalDims, setNaturalDims] = useState<Record<string, { w: number; h: number }>>({});
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [hudHidden, setHudHidden] = useState(false);
  const [localPrefs, setLocalPrefs] = useState<{
    readingDir: "rtl" | "ltr";
    spreadMode: "single" | "spread";
    coverAlone: boolean;
  }>({
    readingDir: "rtl",
    spreadMode: "single",
    coverAlone: true,
  });

  useEffect(() => {
    const dlg = dialogRef.current;
    if (!dlg) return;
    if (open && group && !dlg.open) {
      dlg.showModal();
    } else if ((!open || !group) && dlg.open) {
      dlg.close();
    }
  }, [open, group]);

  useEffect(() => {
    setPageIdx(0);
    setPageUrls({});
    setNaturalDims({});
    setHudHidden(false);
    if (group) {
      setLocalPrefs({
        readingDir: group.readingDir,
        spreadMode: group.spreadMode,
        coverAlone: group.coverAlone,
      });
    }
  }, [group?.id, open]);

  const useLocalPrefsOnly = readOnly || group?.deletedAt !== null;
  const spreadMode = useLocalPrefsOnly ? localPrefs.spreadMode : (group?.spreadMode ?? "single");
  const readingDir = useLocalPrefsOnly ? localPrefs.readingDir : (group?.readingDir ?? "rtl");
  const coverAlone = useLocalPrefsOnly ? localPrefs.coverAlone : (group?.coverAlone ?? true);
  const pages = group?.pages ?? [];

  useEffect(() => {
    setPageIdx((prev) => {
      const maxIdx = Math.max(0, pages.length - 1);
      return prev > maxIdx ? maxIdx : prev;
    });
  }, [pages.length]);

  const updateReaderPrefs = (
    nextDir: "rtl" | "ltr",
    nextSpread: "single" | "spread",
    nextCover: boolean
  ) => {
    setLocalPrefs({ readingDir: nextDir, spreadMode: nextSpread, coverAlone: nextCover });
    if (!readOnly && group && group.deletedAt === null) {
      void onUpdateReaderPrefs(group.id, nextDir, nextSpread, nextCover);
    }
  };

  const spreads = useMemo(
    () => buildSpreads(pages.length, spreadMode, coverAlone),
    [pages.length, spreadMode, coverAlone]
  );

  const currentSpreadIndex = useMemo(() => {
    if (spreads.length === 0) return 0;
    const found = spreads.findIndex((s) => s.includes(pageIdx));
    return found >= 0 ? found : 0;
  }, [spreads, pageIdx]);

  const activeSpread = spreads[currentSpreadIndex] ?? [];

  const orderedVisiblePages = useMemo(() => {
    const items = activeSpread.map((i) => pages[i]).filter(Boolean);
    if (items.length === 2 && readingDir === "rtl") {
      return [items[1], items[0]];
    }
    return items;
  }, [activeSpread, pages, readingDir]);

  // Compute high-res Scale-Down layout dimensions so zooming stays razor-sharp AND left/right heights match!
  const stageLayout = useMemo(() => {
    if (orderedVisiblePages.length === 0) {
      return {
        contentWidth: 1,
        contentHeight: 1,
        layoutScale: 1,
        sizes: new Map<string, { width: number; height: number }>(),
      };
    }

    const getDims = (p: (typeof orderedVisiblePages)[number]) => {
      const loaded = naturalDims[p.itemId];
      if (loaded && loaded.w > 0 && loaded.h > 0) return loaded;
      if (p.width > 0 && p.height > 0) return { w: p.width, h: p.height };
      return { w: 700, h: 1000 };
    };

    const sizes = new Map<string, { width: number; height: number }>();

    if (orderedVisiblePages.length === 2) {
      const p1 = orderedVisiblePages[0];
      const p2 = orderedVisiblePages[1];
      const d1 = getDims(p1);
      const d2 = getDims(p2);
      const rawMaxH = Math.max(d1.h, d2.h);
      const baseH = Math.min(rawMaxH, MAX_SPREAD_HEIGHT);
      const w1 = Math.max(1, Math.round(baseH * (d1.w / d1.h)));
      const w2 = Math.max(1, Math.round(baseH * (d2.w / d2.h)));
      sizes.set(p1.itemId, { width: w1, height: baseH });
      sizes.set(p2.itemId, { width: w2, height: baseH });
      return {
        contentWidth: w1 + w2,
        contentHeight: baseH,
        layoutScale: baseH / Math.max(rawMaxH, 1),
        sizes,
      };
    }

    const p = orderedVisiblePages[0];
    const d = getDims(p);
    const ls = Math.min(1, MAX_SINGLE_EDGE / Math.max(d.w, d.h, 1));
    const w = Math.max(1, Math.round(d.w * ls));
    const h = Math.max(1, Math.round(d.h * ls));
    sizes.set(p.itemId, { width: w, height: h });
    return {
      contentWidth: w,
      contentHeight: h,
      layoutScale: ls,
      sizes,
    };
  }, [orderedVisiblePages, naturalDims]);

  const hasPrevSpread = currentSpreadIndex > 0;
  const hasNextSpread = currentSpreadIndex < spreads.length - 1;

  const goPrevSpread = () => {
    if (!hasPrevSpread) return;
    setPageIdx(spreads[currentSpreadIndex - 1][0]);
  };

  const goNextSpread = () => {
    if (!hasNextSpread) return;
    setPageIdx(spreads[currentSpreadIndex + 1][0]);
  };

  const handleLeftSideAction = () => {
    if (readingDir === "rtl") goNextSpread();
    else goPrevSpread();
  };

  const handleRightSideAction = () => {
    if (readingDir === "rtl") goPrevSpread();
    else goNextSpread();
  };

  const physics = useViewportPhysics({
    viewportRef,
    stageRef,
    contentWidth: stageLayout.contentWidth,
    contentHeight: stageLayout.contentHeight,
    maxFitScale: 2,
    enabled: open && orderedVisiblePages.length > 0,
    resetKey: `${group?.id ?? ""}:${currentSpreadIndex}:${spreadMode}:${coverAlone}:${open}`,
    onSwipeLeft: () => {
      if (readingDir === "rtl") goPrevSpread();
      else goNextSpread();
    },
    onSwipeRight: () => {
      if (readingDir === "rtl") goNextSpread();
      else goPrevSpread();
    },
    onTapZone: (zone) => {
      if (zone === "left") handleLeftSideAction();
      else if (zone === "right") handleRightSideAction();
      else setHudHidden((v) => !v);
    },
  });

  // Fetch active pages + prefetch next 2 pages
  useEffect(() => {
    if (!open || !group || activeSpread.length === 0) return;
    let active = true;

    const loadVisible = async () => {
      const updates: Record<string, string> = {};
      await Promise.all(
        activeSpread.map(async (idx) => {
          const p = pages[idx];
          if (!p) return;
          try {
            const url = await imageMemoryCache.getFullImageUrl(p.itemId, p.mimeType);
            updates[p.itemId] = url;
          } catch {
            // ignore
          }
        })
      );
      if (active) {
        setPageUrls((prev) => ({ ...prev, ...updates }));
      }
    };

    void loadVisible();

    const maxVisible = activeSpread[activeSpread.length - 1] ?? 0;
    for (let offset = 1; offset <= 2; offset++) {
      const nextP = pages[maxVisible + offset];
      if (nextP) imageMemoryCache.prefetchFullImage(nextP.itemId, nextP.mimeType);
    }

    return () => {
      active = false;
    };
  }, [open, group?.id, activeSpread, pages]);

  useEffect(() => {
    if (!open || !group) return;
    const handleKey = (e: globalThis.KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const openDialogs = document.querySelectorAll("dialog[open]");
      if (openDialogs.length > 0 && openDialogs[openDialogs.length - 1] !== dialogRef.current) {
        return;
      }
      const target = e.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable)
      ) {
        return;
      }

      if (e.key === "ArrowLeft") {
        e.preventDefault();
        if (readingDir === "rtl") goNextSpread();
        else goPrevSpread();
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        if (readingDir === "rtl") goPrevSpread();
        else goNextSpread();
      } else if (e.key === " ") {
        e.preventDefault();
        if (e.shiftKey) goPrevSpread();
        else goNextSpread();
      } else if (e.key === "s" || e.key === "S") {
        e.preventDefault();
        updateReaderPrefs(
          readingDir,
          spreadMode === "single" ? "spread" : "single",
          coverAlone
        );
      } else if (e.key === "d" || e.key === "D") {
        e.preventDefault();
        updateReaderPrefs(
          readingDir === "rtl" ? "ltr" : "rtl",
          spreadMode,
          coverAlone
        );
      } else if (e.key === "c" || e.key === "C") {
        e.preventDefault();
        updateReaderPrefs(readingDir, spreadMode, !coverAlone);
      } else if (!readOnly && group.deletedAt === null && e.key >= "1" && e.key <= "5") {
        e.preventDefault();
        void onUpdateRating(group.id, Number(e.key));
      } else if (!readOnly && group.deletedAt === null && e.key === "0") {
        e.preventDefault();
        void onUpdateRating(group.id, 0);
      } else if (e.key === "i" || e.key === "I") {
        e.preventDefault();
        setDrawerOpen((v) => !v);
      } else if (e.key === "+" || e.key === "=") {
        e.preventDefault();
        physics.zoomIn();
      } else if (e.key === "-") {
        e.preventDefault();
        physics.zoomOut();
      } else if (e.key === "f" || e.key === "F") {
        e.preventDefault();
        physics.resetFit();
      }
    };
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [
    open,
    group,
    readOnly,
    readingDir,
    spreadMode,
    coverAlone,
    currentSpreadIndex,
    spreads,
    onUpdateReaderPrefs,
    onUpdateRating,
    physics.zoomIn,
    physics.zoomOut,
    physics.resetFit,
  ]);

  const spreadLabel =
    activeSpread.length === 2
      ? `P.${activeSpread[0] + 1}-${activeSpread[1] + 1} / ${pages.length}`
      : `P.${(activeSpread[0] ?? 0) + 1} / ${pages.length}`;

  const leftDisabled = readingDir === "rtl" ? !hasNextSpread : !hasPrevSpread;
  const rightDisabled = readingDir === "rtl" ? !hasPrevSpread : !hasNextSpread;
  const effectivePercent = Math.round(physics.hud.scale * stageLayout.layoutScale * 100);

  return (
    <dialog
      ref={dialogRef}
      className="manga-reader-dialog"
      aria-label={group?.title || "漫画リーダー"}
      onClose={(e) => {
        e.currentTarget.returnValue = "";
        onClose();
      }}
    >
      <div className={`reader-main ${hudHidden ? "hud-hidden" : ""}`}>
        <div className="lightbox-hud">
          <div className="lightbox-hud-left">
            <BookOpen size={15} style={{ color: "var(--accent-blue)", flexShrink: 0 }} />
            <span className="lightbox-title">{group?.title || "無題のグループ"}</span>
            <span className="toolbar-count tabular-nums">{spreadLabel}</span>
          </div>

          <div className="lightbox-controls">
            <div className="mode-pill-group" role="group" aria-label="ページ表示形式">
              <button
                type="button"
                className={`mode-pill-btn ${spreadMode === "single" ? "active" : ""}`}
                onClick={() => updateReaderPrefs(readingDir, "single", coverAlone)}
              >
                <FileText size={11} style={{ marginInlineEnd: 3, verticalAlign: "-1px" }} />
                単ページ
              </button>
              <button
                type="button"
                className={`mode-pill-btn ${spreadMode === "spread" ? "active" : ""}`}
                onClick={() => updateReaderPrefs(readingDir, "spread", coverAlone)}
              >
                <Columns2 size={11} style={{ marginInlineEnd: 3, verticalAlign: "-1px" }} />
                見開き
              </button>
            </div>

            <div className="mode-pill-group" role="group" aria-label="綴じ方向">
              <button
                type="button"
                className={`mode-pill-btn ${readingDir === "rtl" ? "active" : ""}`}
                title="日本の漫画向け：左方向へ進む"
                onClick={() => updateReaderPrefs("rtl", spreadMode, coverAlone)}
              >
                右綴じ
              </button>
              <button
                type="button"
                className={`mode-pill-btn ${readingDir === "ltr" ? "active" : ""}`}
                title="横書き向け：右方向へ進む"
                onClick={() => updateReaderPrefs("ltr", spreadMode, coverAlone)}
              >
                左綴じ
              </button>
            </div>

            {spreadMode === "spread" && (
              <button
                type="button"
                className={`pill-btn ${coverAlone ? "active" : ""}`}
                title="1ページ目（表紙）を単独表示する"
                onClick={() => updateReaderPrefs(readingDir, spreadMode, !coverAlone)}
              >
                表紙単独
              </button>
            )}

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
              title="クリックでフィット ↔ 拡大表示を切替"
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

            {!readOnly && group && group.deletedAt === null && (
              <button
                type="button"
                className="icon-btn"
                title="ページ構成を編集"
                aria-label="ページ構成を編集"
                onClick={() => onOpenEditor(group.id)}
              >
                <ListOrdered size={15} />
              </button>
            )}

            {group && group.deletedAt === null && onPlainExport && (
              <button
                type="button"
                className="icon-btn"
                title={isWebClient ? "ZIP でダウンロード" : "フォルダとして書き出し..."}
                aria-label={isWebClient ? "ZIP でダウンロード" : "フォルダとして書き出し"}
                onClick={() => onPlainExport(group)}
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
              aria-label="リーダーを閉じる"
              onClick={() => dialogRef.current?.close()}
            >
              <X size={16} />
            </button>
          </div>
        </div>

        <div
          ref={viewportRef}
          className={`lightbox-viewport image-mode ${physics.hud.canPan ? "pannable" : ""} ${
            physics.isDragging ? "dragging" : ""
          }`}
          {...physics.handlers}
        >
          <div
            ref={stageRef}
            className="spread-stage"
            style={{
              inlineSize: stageLayout.contentWidth,
              blockSize: stageLayout.contentHeight,
            }}
          >
            {orderedVisiblePages.map((p) => {
              const rawUrl = pageUrls[p.itemId];
              const cachedLiveUrl = imageMemoryCache.getCachedFullImageUrl(p.itemId);
              const url = rawUrl && cachedLiveUrl === rawUrl ? rawUrl : cachedLiveUrl;
              const dim = stageLayout.sizes.get(p.itemId);
              return url ? (
                <img
                  key={p.itemId}
                  src={url}
                  alt={p.title || `ページ ${p.pageOrder + 1}`}
                  draggable={false}
                  className="spread-page-img"
                  onLoad={(e) => {
                    const nw = e.currentTarget.naturalWidth;
                    const nh = e.currentTarget.naturalHeight;
                    if (nw > 0 && nh > 0) {
                      setNaturalDims((prev) => {
                        const existing = prev[p.itemId];
                        if (existing && existing.w === nw && existing.h === nh) return prev;
                        return { ...prev, [p.itemId]: { w: nw, h: nh } };
                      });
                    }
                  }}
                  style={
                    dim
                      ? { inlineSize: dim.width, blockSize: dim.height }
                      : undefined
                  }
                />
              ) : (
                <div
                  key={p.itemId}
                  style={{
                    inlineSize: dim?.width ?? 400,
                    blockSize: dim?.height ?? 600,
                    display: "grid",
                    placeItems: "center",
                    color: "var(--text-secondary)",
                    fontSize: 24,
                  }}
                >
                  P.{p.pageOrder + 1} 読み込み中...
                </div>
              );
            })}
          </div>

          <button
            type="button"
            className="lightbox-nav-btn prev"
            disabled={leftDisabled}
            aria-label={readingDir === "rtl" ? "次のページ" : "前のページ"}
            onClick={handleLeftSideAction}
          >
            <ChevronLeft size={22} />
          </button>

          <button
            type="button"
            className="lightbox-nav-btn next"
            disabled={rightDisabled}
            aria-label={readingDir === "rtl" ? "前のページ" : "次のページ"}
            onClick={handleRightSideAction}
          >
            <ChevronRight size={22} />
          </button>
        </div>

        {pages.length > 1 && (
          <div className="reader-scrubber">
            <span className="tabular-nums" style={{ fontSize: 11, color: "var(--text-muted)" }}>
              {readingDir === "rtl" ? `${pages.length}P` : "1P"}
            </span>
            <input
              type="range"
              min={0}
              max={pages.length - 1}
              value={pageIdx}
              aria-label="ページスライダー"
              style={readingDir === "rtl" ? { direction: "rtl" } : undefined}
              onChange={(e) => setPageIdx(Number(e.target.value))}
            />
            <span className="tabular-nums" style={{ fontSize: 11, color: "var(--text-muted)" }}>
              {readingDir === "rtl" ? "1P" : `${pages.length}P`}
            </span>
          </div>
        )}
      </div>

      {drawerOpen && group && (
        <aside className="lightbox-drawer" aria-label="漫画詳細パネル">
          <Inspector
            mode="group"
            itemDetail={null}
            groupDetail={group}
            selectedEntries={[{ kind: "group", id: group.id }]}
            status={status}
            allTags={allTags}
            allFolders={allFolders}
            currentFolderId={null}
            inTrash={group.deletedAt !== null}
            hidePreview
            readOnly={readOnly}
            isWebClient={isWebClient}
            onClose={() => setDrawerOpen(false)}
            onUpdateTitle={(_, title) => onUpdateTitle(group.id, title)}
            onUpdateRating={(_, rating) => onUpdateRating(group.id, rating)}
            onUpdateMemo={(_, memo) => onUpdateMemo(group.id, memo)}
            onAddTag={(_, tag) => onAddTag(group.id, tag)}
            onRemoveTag={(_, tag) => onRemoveTag(group.id, tag)}
            onAddToFolder={(fid) => onAddToFolder(fid, group.id)}
            onRemoveFromFolder={(fid) => onRemoveFromFolder(fid, group.id)}
            onOpenEditor={() => onOpenEditor(group.id)}
            onPlainExport={onPlainExport ? () => onPlainExport(group) : undefined}
            onUngroup={() => onUngroup(group.id)}
            onMoveToTrash={() => onMoveToTrash(group.id)}
            onRestoreFromTrash={() => onRestoreFromTrash(group.id)}
            onPermanentlyDelete={() => onPermanentlyDelete(group.id)}
          />
        </aside>
      )}
    </dialog>
  );
}
