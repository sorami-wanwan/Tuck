import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { EntryRef } from "../types/vault";

export type DragPayload =
  | { type: "entries"; entries: EntryRef[]; hasImageForGroup?: boolean; label: string }
  | { type: "folder"; folderId: string; label: string };

export type DropTarget =
  | { type: "folder"; folderId: string }
  | { type: "folderRoot" }
  | { type: "trash" }
  | { type: "group"; groupId: string }
  | { type: "tag"; tagName: string }
  | null;

interface UseInternalDragOptions {
  onDropEntriesToFolder: (entries: EntryRef[], folderId: string) => void;
  onDropEntriesToTrash: (entries: EntryRef[]) => void;
  onDropImagesToGroup: (itemIds: string[], groupId: string) => void;
  onDropEntriesToTag: (entries: EntryRef[], tagName: string) => void;
  onDropFolderToParent: (folderId: string, newParentId: string | null) => void;
}

function sameDropTarget(a: DropTarget, b: DropTarget): boolean {
  if (a === b) return true;
  if (!a || !b || a.type !== b.type) return false;
  switch (a.type) {
    case "folder":
      return b.type === "folder" && a.folderId === b.folderId;
    case "folderRoot":
    case "trash":
      return true;
    case "group":
      return b.type === "group" && a.groupId === b.groupId;
    case "tag":
      return b.type === "tag" && a.tagName === b.tagName;
  }
}

function hitTestDropTarget(x: number, y: number, payload: DragPayload): DropTarget {
  const els = document.elementsFromPoint(x, y);
  for (const el of els) {
    if (!(el instanceof HTMLElement)) continue;
    const folderId = el.dataset.dropFolderId;
    if (folderId) {
      if (payload.type === "folder" && payload.folderId === folderId) {
        return null;
      }
      return { type: "folder", folderId };
    }
    if (el.dataset.dropFolderRoot === "true" && payload.type === "folder") {
      return { type: "folderRoot" };
    }
    if (el.dataset.dropTrash === "true" && payload.type === "entries") {
      return { type: "trash" };
    }
    const groupId = el.dataset.dropGroupId;
    if (groupId && payload.type === "entries") {
      const canAddToGroup =
        payload.hasImageForGroup !== undefined
          ? payload.hasImageForGroup
          : payload.entries.some((e) => e.kind === "image");
      if (canAddToGroup) {
        return { type: "group", groupId };
      }
    }
    const tagName = el.dataset.dropTagName;
    if (tagName && payload.type === "entries") {
      return { type: "tag", tagName };
    }
  }
  return null;
}

export function useInternalDrag(options: UseInternalDragOptions) {
  const [activePayload, setActivePayload] = useState<DragPayload | null>(null);
  const [hoverTarget, setHoverTarget] = useState<DropTarget>(null);
  const ghostRef = useRef<HTMLDivElement>(null);

  const pendingRef = useRef<{
    payload: DragPayload;
    startX: number;
    startY: number;
    pointerId?: number;
    started: boolean;
  } | null>(null);
  const lastPosRef = useRef<{ x: number; y: number }>({ x: 0, y: 0 });
  const targetRef = useRef<DropTarget>(null);
  const optionsRef = useRef(options);
  optionsRef.current = options;

  const updateGhostTransform = useCallback((x: number, y: number) => {
    lastPosRef.current = { x, y };
    if (ghostRef.current) {
      ghostRef.current.style.transform = `translate3d(${Math.round(x + 14)}px, ${Math.round(
        y + 12
      )}px, 0)`;
    }
  }, []);

  useLayoutEffect(() => {
    if (activePayload) {
      updateGhostTransform(lastPosRef.current.x, lastPosRef.current.y);
    }
  }, [activePayload, updateGhostTransform]);

  const beginPendingDrag = useCallback(
    (payload: DragPayload, clientX: number, clientY: number, pointerId?: number) => {
      if (pendingRef.current !== null) return;
      lastPosRef.current = { x: clientX, y: clientY };
      pendingRef.current = {
        payload,
        startX: clientX,
        startY: clientY,
        pointerId,
        started: false,
      };
    },
    []
  );

  useEffect(() => {
    const clearDrag = () => {
      pendingRef.current = null;
      targetRef.current = null;
      setActivePayload(null);
      setHoverTarget(null);
    };

    const onMove = (e: PointerEvent) => {
      const p = pendingRef.current;
      if (!p) return;
      if (p.pointerId !== undefined && e.pointerId !== p.pointerId) return;
      if (!p.started) {
        const dx = e.clientX - p.startX;
        const dy = e.clientY - p.startY;
        if (dx * dx + dy * dy < 36) return;
        p.started = true;
        setActivePayload(p.payload);
      }
      updateGhostTransform(e.clientX, e.clientY);
      const target = hitTestDropTarget(e.clientX, e.clientY, p.payload);
      if (!sameDropTarget(targetRef.current, target)) {
        targetRef.current = target;
        setHoverTarget(target);
      }
    };

    const onEnd = (e: PointerEvent) => {
      const p = pendingRef.current;
      if (!p) return;
      if (p.pointerId !== undefined && e.pointerId !== p.pointerId) return;
      pendingRef.current = null;

      if (p.started) {
        const target = targetRef.current;
        const opts = optionsRef.current;
        if (target && p.payload.type === "entries") {
          if (target.type === "folder") {
            opts.onDropEntriesToFolder(p.payload.entries, target.folderId);
          } else if (target.type === "trash") {
            opts.onDropEntriesToTrash(p.payload.entries);
          } else if (target.type === "group") {
            const imgIds = p.payload.entries
              .filter((entry) => entry.kind === "image")
              .map((entry) => entry.id);
            if (imgIds.length > 0) {
              opts.onDropImagesToGroup(imgIds, target.groupId);
            }
          } else if (target.type === "tag") {
            opts.onDropEntriesToTag(p.payload.entries, target.tagName);
          }
        } else if (target && p.payload.type === "folder") {
          if (target.type === "folder") {
            opts.onDropFolderToParent(p.payload.folderId, target.folderId);
          } else if (target.type === "folderRoot") {
            opts.onDropFolderToParent(p.payload.folderId, null);
          }
        }
      }

      clearDrag();
    };

    const onCancel = (e: PointerEvent) => {
      const p = pendingRef.current;
      if (!p) return;
      if (p.pointerId !== undefined && e.pointerId !== p.pointerId) return;
      clearDrag();
    };

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape" && pendingRef.current?.started) {
        e.preventDefault();
        e.stopPropagation();
        clearDrag();
      }
    };

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onEnd);
    window.addEventListener("pointercancel", onCancel);
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onEnd);
      window.removeEventListener("pointercancel", onCancel);
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, [updateGhostTransform]);

  return {
    activePayload,
    ghostRef,
    hoverTarget,
    beginPendingDrag,
  };
}
