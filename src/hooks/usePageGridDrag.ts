import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
} from "react";
import { computeGridDropTarget, type CardRect, type GridDropTarget } from "../utils/groupOrder";

export interface DropIndicatorPos {
  left: number;
  top: number;
  height: number;
}

interface UsePageGridDragOptions {
  gridRef: RefObject<HTMLDivElement | null>;
  cardRefs: RefObject<Map<string, HTMLDivElement>>;
  pageIds: readonly string[];
  disabled?: boolean;
  onCommitMove: (fromIndex: number, targetIndex: number) => void;
  onCancelDragAnnouncement?: () => void;
}

interface PendingDragState {
  pointerId: number;
  index: number;
  startX: number;
  startY: number;
  thresholdSq: number;
  started: boolean;
}

const GRID_HALF_GAP = 6;

export function usePageGridDrag({
  gridRef,
  cardRefs,
  pageIds,
  disabled = false,
  onCommitMove,
  onCancelDragAnnouncement,
}: UsePageGridDragOptions) {
  const [isDragging, setIsDragging] = useState(false);
  const [fromIndex, setFromIndex] = useState<number | null>(null);
  const [dropTarget, setDropTarget] = useState<GridDropTarget | null>(null);
  const [indicatorPos, setIndicatorPos] = useState<DropIndicatorPos | null>(null);
  const [cursor, setCursor] = useState<{ x: number; y: number }>({ x: 0, y: 0 });

  const pendingRef = useRef<PendingDragState | null>(null);
  const dropTargetRef = useRef<GridDropTarget | null>(null);
  const cursorRef = useRef<{ x: number; y: number }>({ x: 0, y: 0 });
  const rafIdRef = useRef<number | null>(null);

  const pageIdsRef = useRef(pageIds);
  pageIdsRef.current = pageIds;

  const disabledRef = useRef(disabled);
  disabledRef.current = disabled;

  const onCommitMoveRef = useRef(onCommitMove);
  onCommitMoveRef.current = onCommitMove;

  const onCancelAnnouncementRef = useRef(onCancelDragAnnouncement);
  onCancelAnnouncementRef.current = onCancelDragAnnouncement;

  const stopAutoScroll = useCallback(() => {
    if (rafIdRef.current !== null) {
      window.cancelAnimationFrame(rafIdRef.current);
      rafIdRef.current = null;
    }
  }, []);

  const resetDragState = useCallback(() => {
    const p = pendingRef.current;
    pendingRef.current = null;
    stopAutoScroll();
    if (p) {
      try {
        const gridEl = gridRef.current;
        if (gridEl && gridEl.hasPointerCapture(p.pointerId)) {
          gridEl.releasePointerCapture(p.pointerId);
        }
      } catch {
        // Ignore if pointer capture is already released.
      }
    }
    dropTargetRef.current = null;
    setIsDragging(false);
    setFromIndex(null);
    setDropTarget(null);
    setIndicatorPos(null);
  }, [gridRef, stopAutoScroll]);

  const updateDropAt = useCallback(
    (clientX: number, clientY: number, draggedIndex: number) => {
      const gridEl = gridRef.current;
      const cardsMap = cardRefs.current;
      const ids = pageIdsRef.current;
      if (!gridEl || !cardsMap || ids.length === 0) {
        dropTargetRef.current = null;
        setDropTarget(null);
        setIndicatorPos(null);
        return;
      }

      const rects: CardRect[] = [];
      for (let i = 0; i < ids.length; i += 1) {
        const el = cardsMap.get(ids[i]);
        if (!el) continue;
        const r = el.getBoundingClientRect();
        rects.push({
          index: i,
          left: r.left,
          right: r.right,
          top: r.top,
          bottom: r.bottom,
        });
      }

      const target = computeGridDropTarget(rects, draggedIndex, clientX, clientY);
      dropTargetRef.current = target;
      setDropTarget(target);

      if (!target || target.isNoOp) {
        setIndicatorPos(null);
        return;
      }

      const anchorId = ids[target.anchorIndex];
      const anchorEl = anchorId ? cardsMap.get(anchorId) : undefined;
      if (!anchorEl) {
        setIndicatorPos(null);
        return;
      }

      const gridRect = gridEl.getBoundingClientRect();
      const anchorRect = anchorEl.getBoundingClientRect();
      const rawLeft =
        target.edge === "left"
          ? anchorRect.left - gridRect.left + gridEl.scrollLeft - GRID_HALF_GAP
          : anchorRect.right - gridRect.left + gridEl.scrollLeft + GRID_HALF_GAP;
      const maxLeft = Math.max(4, gridEl.scrollWidth - 4);
      const left = Math.max(4, Math.min(maxLeft, rawLeft));
      const top = anchorRect.top - gridRect.top + gridEl.scrollTop;
      const height = anchorRect.height;

      setIndicatorPos({ left, top, height });
    },
    [cardRefs, gridRef]
  );

  const startAutoScrollLoop = useCallback(() => {
    stopAutoScroll();
    let lastTime = performance.now();

    const step = (now: number) => {
      const p = pendingRef.current;
      if (!p || !p.started) {
        rafIdRef.current = null;
        return;
      }

      const dt = Math.min(0.05, Math.max(0, (now - lastTime) / 1000));
      lastTime = now;

      const gridEl = gridRef.current;
      if (gridEl) {
        const rect = gridEl.getBoundingClientRect();
        const edgeZone = Math.min(56, rect.height / 3);
        const cy = cursorRef.current.y;
        let deltaY = 0;

        if (edgeZone > 0) {
          if (cy < rect.top + edgeZone) {
            const intensity = Math.min(2, (rect.top + edgeZone - cy) / edgeZone);
            deltaY = -intensity * 620 * dt;
          } else if (cy > rect.bottom - edgeZone) {
            const intensity = Math.min(2, (cy - (rect.bottom - edgeZone)) / edgeZone);
            deltaY = intensity * 620 * dt;
          }
        }

        if (deltaY !== 0) {
          const prevScrollTop = gridEl.scrollTop;
          gridEl.scrollTop = prevScrollTop + deltaY;
          if (gridEl.scrollTop !== prevScrollTop) {
            updateDropAt(cursorRef.current.x, cursorRef.current.y, p.index);
          }
        }
      }

      rafIdRef.current = window.requestAnimationFrame(step);
    };

    rafIdRef.current = window.requestAnimationFrame(step);
  }, [gridRef, stopAutoScroll, updateDropAt]);

  const cancelDrag = useCallback(() => {
    const wasStarted = Boolean(pendingRef.current?.started);
    resetDragState();
    if (wasStarted) {
      onCancelAnnouncementRef.current?.();
    }
  }, [resetDragState]);

  const handleCardPointerDown = useCallback(
    (index: number, e: ReactPointerEvent<HTMLDivElement>) => {
      if (disabledRef.current || pageIdsRef.current.length < 2) return;
      if (e.button !== 0) return;
      if (pendingRef.current !== null) return;

      const targetEl = e.target as HTMLElement | null;
      const isHandle = Boolean(targetEl?.closest("[data-drag-handle='true']"));

      if (e.pointerType === "touch" && !isHandle) {
        return;
      }
      if (!isHandle && targetEl?.closest("button, a, input, select, textarea")) {
        return;
      }
      if (e.pointerType === "touch" && isHandle) {
        e.preventDefault();
      }

      cursorRef.current = { x: e.clientX, y: e.clientY };
      pendingRef.current = {
        pointerId: e.pointerId,
        index,
        startX: e.clientX,
        startY: e.clientY,
        thresholdSq: isHandle ? 16 : 36,
        started: false,
      };
    },
    []
  );

  useEffect(() => {
    const onMove = (e: PointerEvent) => {
      const p = pendingRef.current;
      if (!p || e.pointerId !== p.pointerId) return;

      cursorRef.current = { x: e.clientX, y: e.clientY };

      if (!p.started) {
        const dx = e.clientX - p.startX;
        const dy = e.clientY - p.startY;
        if (dx * dx + dy * dy < p.thresholdSq) return;

        p.started = true;
        setIsDragging(true);
        setFromIndex(p.index);
        try {
          gridRef.current?.setPointerCapture(p.pointerId);
        } catch {
          // Pointer capture may fail in synthetic or non-primary contexts; window listeners still work.
        }
        startAutoScrollLoop();
      }

      setCursor({ x: e.clientX, y: e.clientY });
      updateDropAt(e.clientX, e.clientY, p.index);
    };

    const onEnd = (e: PointerEvent) => {
      const p = pendingRef.current;
      if (!p || e.pointerId !== p.pointerId) return;

      const wasStarted = p.started;
      const draggedIndex = p.index;
      const committedTarget = dropTargetRef.current;

      resetDragState();

      if (wasStarted && committedTarget && !committedTarget.isNoOp) {
        onCommitMoveRef.current(draggedIndex, committedTarget.targetIndex);
      }
    };

    const onCancel = (e: PointerEvent) => {
      const p = pendingRef.current;
      if (!p || e.pointerId !== p.pointerId) return;
      cancelDrag();
    };

    const onKeyDown = (e: KeyboardEvent) => {
      const p = pendingRef.current;
      if (!p || !p.started) return;
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        cancelDrag();
      }
    };

    const gridEl = gridRef.current;
    const onGridScroll = () => {
      const p = pendingRef.current;
      if (!p || !p.started) return;
      updateDropAt(cursorRef.current.x, cursorRef.current.y, p.index);
    };

    gridEl?.addEventListener("scroll", onGridScroll, { passive: true });
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onEnd);
    window.addEventListener("pointercancel", onCancel);
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      gridEl?.removeEventListener("scroll", onGridScroll);
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onEnd);
      window.removeEventListener("pointercancel", onCancel);
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, [cancelDrag, gridRef, resetDragState, startAutoScrollLoop, updateDropAt]);

  const pageIdsKey = pageIds.join(",");
  useEffect(() => {
    if (disabled || pendingRef.current !== null) {
      resetDragState();
    }
  }, [disabled, pageIdsKey, resetDragState]);

  useEffect(() => stopAutoScroll, [stopAutoScroll]);

  return {
    isDragging,
    fromIndex,
    dropTarget,
    indicatorPos,
    cursor,
    handleCardPointerDown,
    cancelDrag,
  };
}
