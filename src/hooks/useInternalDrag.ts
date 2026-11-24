import { useCallback, useEffect, useRef, useState } from "react";
import type { EntryRef } from "../types/vault";

export type DragPayload =
  | { type: "entries"; entries: EntryRef[]; label: string }
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
      const hasStandaloneImage = payload.entries.some((e) => e.kind === "image");
      if (hasStandaloneImage) {
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
  const [cursor, setCursor] = useState<{ x: number; y: number }>({ x: 0, y: 0 });
  const [hoverTarget, setHoverTarget] = useState<DropTarget>(null);

  const pendingRef = useRef<{
    payload: DragPayload;
    startX: number;
    startY: number;
    started: boolean;
  } | null>(null);
  const targetRef = useRef<DropTarget>(null);
  const optionsRef = useRef(options);
  optionsRef.current = options;

  const beginPendingDrag = useCallback(
    (payload: DragPayload, clientX: number, clientY: number) => {
      pendingRef.current = {
        payload,
        startX: clientX,
        startY: clientY,
        started: false,
      };
    },
    []
  );

  useEffect(() => {
    const onMove = (e: PointerEvent) => {
      const p = pendingRef.current;
      if (!p) return;
      if (!p.started) {
        const dx = e.clientX - p.startX;
        const dy = e.clientY - p.startY;
        if (dx * dx + dy * dy < 36) return;
        p.started = true;
        setActivePayload(p.payload);
      }
      setCursor({ x: e.clientX, y: e.clientY });
      const target = hitTestDropTarget(e.clientX, e.clientY, p.payload);
      targetRef.current = target;
      setHoverTarget(target);
    };

    const onEnd = () => {
      const p = pendingRef.current;
      if (!p) return;
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
              .filter((e) => e.kind === "image")
              .map((e) => e.id);
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

      targetRef.current = null;
      setActivePayload(null);
      setHoverTarget(null);
    };

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onEnd);
    window.addEventListener("pointercancel", onEnd);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onEnd);
      window.removeEventListener("pointercancel", onEnd);
    };
  }, []);

  return {
    activePayload,
    cursor,
    hoverTarget,
    beginPendingDrag,
  };
}
