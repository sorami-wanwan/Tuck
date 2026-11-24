import type { RefObject } from "react";
import { BookOpen, Folder, Hash, Trash2 } from "lucide-react";
import type { DragPayload, DropTarget } from "../hooks/useInternalDrag";
import type { FolderNode } from "../types/vault";

export interface DragGhostProps {
  activePayload: DragPayload | null;
  hoverTarget: DropTarget;
  ghostRef: RefObject<HTMLDivElement | null>;
  folders: FolderNode[];
}

export function DragGhost({ activePayload, hoverTarget, ghostRef, folders }: DragGhostProps) {
  if (!activePayload) return null;

  let content = (
    <>
      <Folder size={13} />
      <span>{activePayload.label}</span>
    </>
  );

  if (hoverTarget?.type === "trash" && activePayload.type === "entries") {
    content = (
      <>
        <Trash2 size={13} />
        <span>ゴミ箱へ移動 ({activePayload.entries.length}件)</span>
      </>
    );
  } else if (hoverTarget?.type === "group" && activePayload.type === "entries") {
    content = (
      <>
        <BookOpen size={13} />
        <span>グループへページ追加</span>
      </>
    );
  } else if (hoverTarget?.type === "tag" && activePayload.type === "entries") {
    content = (
      <>
        <Hash size={13} />
        <span>
          #{hoverTarget.tagName} を付与 ({activePayload.entries.length}件)
        </span>
      </>
    );
  } else if (hoverTarget?.type === "folder") {
    const targetFolderName =
      folders.find((f) => f.id === hoverTarget.folderId)?.name ?? "フォルダ";
    content = (
      <>
        <Folder size={13} />
        <span>
          「{targetFolderName}」へ{activePayload.type === "folder" ? "移動" : "追加"}
        </span>
      </>
    );
  } else if (hoverTarget?.type === "folderRoot" && activePayload.type === "folder") {
    content = (
      <>
        <Folder size={13} />
        <span>最上位（ルート）へ移動</span>
      </>
    );
  }

  return (
    <div
      ref={ghostRef}
      className="drag-ghost"
      style={{ insetInlineStart: 0, insetBlockStart: 0 }}
    >
      {content}
    </div>
  );
}
