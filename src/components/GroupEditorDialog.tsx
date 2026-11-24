import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  ArrowLeft,
  ArrowRight,
  ArrowUpDown,
  BookOpen,
  GripVertical,
  Plus,
  Unlink,
  X,
} from "lucide-react";
import { usePageGridDrag } from "../hooks/usePageGridDrag";
import { imageMemoryCache } from "../services/imageCache";
import { isTauriRuntime } from "../services/runtime";
import type { GroupDetail, GroupPage } from "../types/vault";
import { applyPageMove, sortGroupPages, type GroupPageSortMode } from "../utils/groupOrder";
import { ContextMenu, type ContextMenuItem } from "./ContextMenu";

interface GroupEditorDialogProps {
  open: boolean;
  group: GroupDetail | null;
  onClose: () => void;
  onReorder: (groupId: string, orderedIds: string[]) => Promise<void>;
  onRemovePage: (groupId: string, itemId: string) => Promise<void>;
  onAddFilesToGroup: (groupId: string, paths: string[]) => Promise<void>;
  onAddBrowserFilesToGroup?: (groupId: string, files: File[]) => Promise<void>;
}

function PageThumb({
  itemId,
  hasThumbnail,
  mimeType,
}: {
  itemId: string;
  hasThumbnail: boolean;
  mimeType: string;
}) {
  const [url, setUrl] = useState<string | undefined>(() =>
    imageMemoryCache.getCachedThumbnailUrl(itemId)
  );

  useEffect(() => {
    let active = true;
    const cached = imageMemoryCache.getCachedThumbnailUrl(itemId);
    if (cached) {
      setUrl(cached);
      return;
    }
    const load = async () => {
      try {
        if (!hasThumbnail && mimeType === "image/avif") {
          await imageMemoryCache.processPendingAvifThumbnail(itemId);
        }
        const nextUrl = await imageMemoryCache.getThumbnailUrl(itemId);
        if (active) setUrl(nextUrl);
      } catch {
        // ignore
      }
    };
    void load();
    return () => {
      active = false;
    };
  }, [itemId, hasThumbnail, mimeType]);

  return (
    <div className="page-editor-thumb">
      {url ? <img src={url} alt="" draggable={false} /> : <div className="thumb-skeleton" />}
    </div>
  );
}

function sameItemIdSet(a: readonly GroupPage[], b: readonly GroupPage[]): boolean {
  if (a.length !== b.length) return false;
  const setA = new Set(a.map((p) => p.itemId));
  if (setA.size !== b.length) return false;
  return b.every((p) => setA.has(p.itemId));
}

export function GroupEditorDialog({
  open,
  group,
  onClose,
  onReorder,
  onRemovePage,
  onAddFilesToGroup,
  onAddBrowserFilesToGroup,
}: GroupEditorDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const browserFileInputRef = useRef<HTMLInputElement>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const cardRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const prevRectsRef = useRef<Map<string, DOMRect>>(new Map());

  const [displayPages, setDisplayPages] = useState<GroupPage[]>(() => group?.pages ?? []);
  const [sortMenuPos, setSortMenuPos] = useState<{ x: number; y: number } | null>(null);
  const [liveMessage, setLiveMessage] = useState("");

  const latestGroupRef = useRef<GroupDetail | null>(group);
  latestGroupRef.current = group;

  const displayPagesRef = useRef(displayPages);
  displayPagesRef.current = displayPages;

  const prevGroupIdRef = useRef<string | null>(group?.id ?? null);
  const inFlightCountRef = useRef(0);
  const opGenRef = useRef(0);
  const reorderChainRef = useRef<Promise<void>>(Promise.resolve());
  const announceTimerRef = useRef<number | null>(null);

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
    if (!open || !group) {
      prevGroupIdRef.current = null;
      inFlightCountRef.current = 0;
      opGenRef.current += 1;
      prevRectsRef.current.clear();
      setDisplayPages([]);
      setSortMenuPos(null);
      setLiveMessage("");
      return;
    }

    const groupChanged = prevGroupIdRef.current !== group.id;
    prevGroupIdRef.current = group.id;

    if (groupChanged) {
      inFlightCountRef.current = 0;
      opGenRef.current += 1;
      prevRectsRef.current.clear();
      setSortMenuPos(null);
      setDisplayPages(group.pages);
      return;
    }

    if (inFlightCountRef.current === 0 || !sameItemIdSet(displayPagesRef.current, group.pages)) {
      setDisplayPages(group.pages);
    }
  }, [open, group]);

  useEffect(() => {
    return () => {
      if (announceTimerRef.current !== null) {
        window.clearTimeout(announceTimerRef.current);
      }
    };
  }, []);

  const announce = useCallback((message: string) => {
    if (announceTimerRef.current !== null) {
      window.clearTimeout(announceTimerRef.current);
      announceTimerRef.current = null;
    }
    setLiveMessage("");
    announceTimerRef.current = window.setTimeout(() => {
      announceTimerRef.current = null;
      setLiveMessage(message);
    }, 30);
  }, []);

  const snapshotCardRects = useCallback(() => {
    prevRectsRef.current.clear();
    if (displayPagesRef.current.length > 200) return;
    if (
      typeof window !== "undefined" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches
    ) {
      return;
    }

    for (const page of displayPagesRef.current) {
      const el = cardRefs.current.get(page.itemId);
      if (!el) continue;
      for (const anim of el.getAnimations()) {
        anim.finish();
      }
      prevRectsRef.current.set(page.itemId, el.getBoundingClientRect());
    }
  }, []);

  const pagesOrderKey = useMemo(
    () => displayPages.map((p) => p.itemId).join(","),
    [displayPages]
  );

  useLayoutEffect(() => {
    const prevRects = prevRectsRef.current;
    if (prevRects.size === 0) return;

    if (
      typeof window !== "undefined" &&
      !window.matchMedia("(prefers-reduced-motion: reduce)").matches
    ) {
      for (const page of displayPages) {
        const oldRect = prevRects.get(page.itemId);
        const el = cardRefs.current.get(page.itemId);
        if (!oldRect || !el) continue;
        const newRect = el.getBoundingClientRect();
        const dx = oldRect.left - newRect.left;
        const dy = oldRect.top - newRect.top;
        if (Math.abs(dx) < 0.5 && Math.abs(dy) < 0.5) continue;

        for (const anim of el.getAnimations()) {
          anim.cancel();
        }
        el.animate(
          [{ transform: `translate(${dx}px, ${dy}px)` }, { transform: "translate(0, 0)" }],
          { duration: 180, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" }
        );
      }
    }

    prevRects.clear();
  }, [pagesOrderKey, displayPages]);

  const commitReorder = useCallback(
    (nextPages: GroupPage[], announcement: string) => {
      const targetGroup = latestGroupRef.current;
      if (!targetGroup) return;
      const targetGroupId = targetGroup.id;

      snapshotCardRects();
      setDisplayPages(nextPages);
      announce(announcement);

      const orderedIds = nextPages.map((p) => p.itemId);
      const opGen = ++opGenRef.current;
      inFlightCountRef.current += 1;

      reorderChainRef.current = reorderChainRef.current
        .then(async () => {
          try {
            await onReorder(targetGroupId, orderedIds);
          } catch {
            if (opGenRef.current === opGen) {
              const current = latestGroupRef.current;
              if (current && current.id === targetGroupId) {
                setDisplayPages(current.pages);
              }
            }
          } finally {
            inFlightCountRef.current = Math.max(0, inFlightCountRef.current - 1);
          }
        })
        .catch(() => {});
    },
    [announce, onReorder, snapshotCardRects]
  );

  const handleRemovePage = useCallback(
    (itemId: string) => {
      const targetGroup = latestGroupRef.current;
      if (!targetGroup) return;
      const targetGroupId = targetGroup.id;
      const currentPages = displayPagesRef.current;
      if (currentPages.length > 1) {
        snapshotCardRects();
        setDisplayPages(currentPages.filter((p) => p.itemId !== itemId));
      }

      const opGen = ++opGenRef.current;
      inFlightCountRef.current += 1;

      reorderChainRef.current = reorderChainRef.current
        .then(async () => {
          try {
            await onRemovePage(targetGroupId, itemId);
          } catch {
            if (opGenRef.current === opGen) {
              const current = latestGroupRef.current;
              if (current && current.id === targetGroupId) {
                setDisplayPages(current.pages);
              }
            }
          } finally {
            inFlightCountRef.current = Math.max(0, inFlightCountRef.current - 1);
          }
        })
        .catch(() => {});
    },
    [onRemovePage, snapshotCardRects]
  );

  const pageIds = useMemo(() => displayPages.map((p) => p.itemId), [displayPages]);

  const handleDragCommitMove = useCallback(
    (fromIdx: number, targetIdx: number) => {
      const current = displayPagesRef.current;
      if (fromIdx === targetIdx || fromIdx < 0 || fromIdx >= current.length) return;
      const next = applyPageMove(current, fromIdx, targetIdx);
      const coverSuffix = targetIdx === 0 ? "（表紙）" : "";
      commitReorder(next, `P.${fromIdx + 1} を P.${targetIdx + 1}${coverSuffix} へ移動しました`);
    },
    [commitReorder]
  );

  const handleDragCancelAnnouncement = useCallback(() => {
    announce("ページの並び替えをキャンセルしました");
  }, [announce]);

  const drag = usePageGridDrag({
    gridRef,
    cardRefs,
    pageIds,
    disabled: !open || !group || displayPages.length < 2,
    onCommitMove: handleDragCommitMove,
    onCancelDragAnnouncement: handleDragCancelAnnouncement,
  });

  const movePage = (index: number, delta: number) => {
    const nextIdx = index + delta;
    if (nextIdx < 0 || nextIdx >= displayPages.length) return;
    const next = applyPageMove(displayPages, index, nextIdx);
    const coverSuffix = nextIdx === 0 ? "（表紙）" : "";
    commitReorder(next, `P.${index + 1} を P.${nextIdx + 1}${coverSuffix} へ移動しました`);
  };

  const makeCover = (index: number) => {
    if (index <= 0 || index >= displayPages.length) return;
    const next = applyPageMove(displayPages, index, 0);
    commitReorder(next, `P.${index + 1} を表紙（P.1）に設定しました`);
  };

  const applyBulkSort = (mode: GroupPageSortMode, label: string) => {
    if (displayPages.length < 2) return;
    const next = sortGroupPages(displayPages, mode);
    const changed = next.some((p, idx) => p.itemId !== displayPages[idx]?.itemId);
    if (!changed) {
      announce(`すでに${label}の順番です`);
      return;
    }
    commitReorder(next, `${label}に並び替えました（${next.length}ページ）`);
  };

  const sortMenuItems: ContextMenuItem[] = [
    {
      id: "sort-created-asc",
      label: "追加日が古い順",
      onClick: () => applyBulkSort("createdAsc", "追加日が古い順"),
    },
    {
      id: "sort-created-desc",
      label: "追加日が新しい順",
      onClick: () => applyBulkSort("createdDesc", "追加日が新しい順"),
    },
    {
      id: "sort-title-asc",
      label: "タイトル順",
      onClick: () => applyBulkSort("titleAsc", "タイトル順"),
    },
    {
      id: "sort-reverse",
      label: "ページ順を反転",
      separatorBefore: true,
      onClick: () => applyBulkSort("reverse", "逆順"),
    },
  ];

  const handleAddFiles = async () => {
    if (!group) return;
    if (!isTauriRuntime()) {
      browserFileInputRef.current?.click();
      return;
    }
    const selected = await openDialog({
      title: "グループに追加する画像を選択",
      multiple: true,
      directory: false,
      filters: [{ name: "画像ファイル", extensions: ["png", "jpg", "jpeg", "webp", "gif", "avif"] }],
    });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    if (paths.length > 0) {
      await onAddFilesToGroup(group.id, paths);
    }
  };

  const draggedPage =
    drag.isDragging && drag.fromIndex !== null ? displayPages[drag.fromIndex] ?? null : null;
  const draggedPreviewUrl = draggedPage
    ? imageMemoryCache.getCachedThumbnailUrl(draggedPage.itemId)
    : undefined;
  const previewLeft =
    typeof window !== "undefined"
      ? Math.min(window.innerWidth - 190, Math.max(8, drag.cursor.x + 14))
      : drag.cursor.x + 14;
  const previewTop =
    typeof window !== "undefined"
      ? Math.min(window.innerHeight - 66, Math.max(8, drag.cursor.y + 14))
      : drag.cursor.y + 14;

  return (
    <dialog
      ref={dialogRef}
      className={`group-editor-dialog ${drag.isDragging ? "is-reordering" : ""}`}
      aria-labelledby="group-editor-title"
      onCancel={(e) => {
        if (drag.isDragging) {
          e.preventDefault();
          drag.cancelDrag();
          return;
        }
        if (sortMenuPos !== null) {
          e.preventDefault();
          setSortMenuPos(null);
        }
      }}
      onClose={(e) => {
        e.currentTarget.returnValue = "";
        drag.cancelDrag();
        setSortMenuPos(null);
        onClose();
      }}
    >
      <input
        ref={browserFileInputRef}
        type="file"
        multiple
        accept="image/*"
        style={{ display: "none" }}
        onChange={(e) => {
          const files = Array.from(e.currentTarget.files ?? []);
          e.currentTarget.value = "";
          if (group && files.length > 0 && onAddBrowserFilesToGroup) {
            void onAddBrowserFilesToGroup(group.id, files);
          }
        }}
      />
      <div className="group-editor-header">
        <div className="group-editor-title-wrap">
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <BookOpen size={16} style={{ color: "var(--accent-blue)", flexShrink: 0 }} />
            <h2 id="group-editor-title">
              {group?.title || "無題のグループ"} — ページ構成 ({displayPages.length}P)
            </h2>
          </div>
          {displayPages.length >= 2 && (
            <span className="group-editor-subtitle">
              ドラッグ＆ドロップまたはボタンでページ順を変更できます
            </span>
          )}
        </div>

        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <button
            type="button"
            className={`btn ${sortMenuPos ? "active" : ""}`}
            disabled={displayPages.length < 2}
            aria-haspopup="menu"
            aria-expanded={sortMenuPos !== null}
            onPointerDown={(e) => e.stopPropagation()}
            onClick={(e) => {
              if (sortMenuPos) {
                setSortMenuPos(null);
                return;
              }
              const rect = e.currentTarget.getBoundingClientRect();
              setSortMenuPos({ x: rect.left, y: rect.bottom + 6 });
            }}
          >
            <ArrowUpDown size={13} />
            並び替え
          </button>
          <button type="button" className="btn" onClick={() => void handleAddFiles()}>
            <Plus size={13} />
            画像を追加
          </button>
          <button
            type="button"
            className="icon-btn"
            aria-label="閉じる"
            onClick={() => dialogRef.current?.close()}
          >
            <X size={16} />
          </button>
        </div>
      </div>

      <div ref={gridRef} className="group-editor-grid">
        {displayPages.map((page, index) => {
          const isSource = drag.isDragging && drag.fromIndex === index;
          return (
            <div
              key={page.itemId}
              ref={(el) => {
                if (el) cardRefs.current.set(page.itemId, el);
                else cardRefs.current.delete(page.itemId);
              }}
              className={`page-editor-card ${isSource ? "is-dragging-source" : ""}`}
              onPointerDown={(e) => drag.handleCardPointerDown(index, e)}
            >
              <PageThumb
                itemId={page.itemId}
                hasThumbnail={page.hasThumbnail}
                mimeType={page.mimeType}
              />
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "space-between",
                  gap: 4,
                  fontSize: 11,
                }}
              >
                <div style={{ display: "flex", alignItems: "center", gap: 3, minInlineSize: 0 }}>
                  {displayPages.length >= 2 && (
                    <span
                      data-drag-handle="true"
                      className="page-drag-handle"
                      title="ドラッグしてページを並び替え"
                      aria-hidden="true"
                    >
                      <GripVertical size={13} />
                    </span>
                  )}
                  <span className="tabular-nums" style={{ fontWeight: 600 }}>
                    P.{index + 1} {index === 0 ? "(表紙)" : ""}
                  </span>
                </div>
                {index > 0 && (
                  <button
                    type="button"
                    className="pill-btn"
                    style={{ padding: "1px 6px", fontSize: 10, flexShrink: 0 }}
                    onClick={() => makeCover(index)}
                  >
                    表紙にする
                  </button>
                )}
              </div>
              <div className="page-editor-controls">
                <div style={{ display: "flex", gap: 2 }}>
                  <button
                    type="button"
                    className="icon-btn"
                    style={{ inlineSize: 24, blockSize: 24 }}
                    disabled={index === 0}
                    title="前へ移動"
                    aria-label={`P.${index + 1} を前へ移動`}
                    onClick={() => movePage(index, -1)}
                  >
                    <ArrowLeft size={13} />
                  </button>
                  <button
                    type="button"
                    className="icon-btn"
                    style={{ inlineSize: 24, blockSize: 24 }}
                    disabled={index === displayPages.length - 1}
                    title="後ろへ移動"
                    aria-label={`P.${index + 1} を後ろへ移動`}
                    onClick={() => movePage(index, 1)}
                  >
                    <ArrowRight size={13} />
                  </button>
                </div>
                <button
                  type="button"
                  className="icon-btn"
                  style={{ inlineSize: 24, blockSize: 24, color: "var(--text-secondary)" }}
                  title="グループから外して単体画像に戻す"
                  aria-label={`P.${index + 1} をグループから外す`}
                  onClick={() => handleRemovePage(page.itemId)}
                >
                  <Unlink size={13} />
                </button>
              </div>
            </div>
          );
        })}

        {drag.isDragging && drag.indicatorPos && (
          <div
            className="page-drop-indicator"
            aria-hidden="true"
            style={{
              insetInlineStart: drag.indicatorPos.left,
              insetBlockStart: drag.indicatorPos.top,
              blockSize: drag.indicatorPos.height,
            }}
          />
        )}
      </div>

      {drag.isDragging && drag.fromIndex !== null && draggedPage && (
        <div
          className="page-drag-preview"
          aria-hidden="true"
          style={{
            insetInlineStart: previewLeft,
            insetBlockStart: previewTop,
          }}
        >
          <div className="page-drag-preview-thumb">
            {draggedPreviewUrl ? (
              <img src={draggedPreviewUrl} alt="" draggable={false} />
            ) : (
              <div className="thumb-skeleton" />
            )}
          </div>
          <span className="tabular-nums">
            {!drag.dropTarget || drag.dropTarget.isNoOp
              ? `P.${drag.fromIndex + 1}`
              : `P.${drag.fromIndex + 1} → P.${drag.dropTarget.targetIndex + 1}${
                  drag.dropTarget.targetIndex === 0 ? " (表紙)" : ""
                }`}
          </span>
        </div>
      )}

      <ContextMenu
        open={sortMenuPos !== null}
        x={sortMenuPos?.x ?? 0}
        y={sortMenuPos?.y ?? 0}
        items={sortMenuItems}
        onClose={() => setSortMenuPos(null)}
      />

      <div className="visually-hidden" role="status" aria-live="polite">
        {liveMessage}
      </div>
    </dialog>
  );
}
