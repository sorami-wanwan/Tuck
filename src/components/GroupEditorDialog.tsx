import { useEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ArrowLeft, ArrowRight, BookOpen, Plus, Unlink, X } from "lucide-react";
import { imageMemoryCache } from "../services/imageCache";
import { isTauriRuntime } from "../services/runtime";
import type { GroupDetail } from "../types/vault";

interface GroupEditorDialogProps {
  open: boolean;
  group: GroupDetail | null;
  onClose: () => void;
  onReorder: (groupId: string, orderedIds: string[]) => Promise<void>;
  onRemovePage: (groupId: string, itemId: string) => Promise<void>;
  onAddFilesToGroup: (groupId: string, paths: string[]) => Promise<void>;
  onAddBrowserFilesToGroup?: (groupId: string, files: File[]) => Promise<void>;
}

function PageThumb({ itemId }: { itemId: string }) {
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
    void imageMemoryCache
      .getThumbnailUrl(itemId)
      .then((u) => {
        if (active) setUrl(u);
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, [itemId]);

  return (
    <div className="page-editor-thumb">
      {url ? <img src={url} alt="" draggable={false} /> : <div className="thumb-skeleton" />}
    </div>
  );
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

  useEffect(() => {
    const dlg = dialogRef.current;
    if (!dlg) return;
    if (open && group && !dlg.open) {
      dlg.showModal();
    } else if ((!open || !group) && dlg.open) {
      dlg.close();
    }
  }, [open, group]);

  const movePage = (index: number, delta: number) => {
    if (!group) return;
    const nextIdx = index + delta;
    if (nextIdx < 0 || nextIdx >= group.pages.length) return;
    const ids = group.pages.map((p) => p.itemId);
    const [moved] = ids.splice(index, 1);
    ids.splice(nextIdx, 0, moved);
    void onReorder(group.id, ids);
  };

  const makeCover = (index: number) => {
    if (!group || index <= 0) return;
    const ids = group.pages.map((p) => p.itemId);
    const [moved] = ids.splice(index, 1);
    ids.unshift(moved);
    void onReorder(group.id, ids);
  };

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

  return (
    <dialog
      ref={dialogRef}
      className="group-editor-dialog"
      aria-labelledby="group-editor-title"
      onClose={(e) => {
        e.currentTarget.returnValue = "";
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
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <BookOpen size={16} style={{ color: "var(--accent-blue)" }} />
          <h2 id="group-editor-title">
            {group?.title || "無題のグループ"} — ページ構成 ({group?.pageCount ?? 0}P)
          </h2>
        </div>

        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
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

      <div className="group-editor-grid">
        {group?.pages.map((page, index) => (
          <div key={page.itemId} className="page-editor-card">
            <PageThumb itemId={page.itemId} />
            <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", fontSize: 11 }}>
              <span className="tabular-nums" style={{ fontWeight: 600 }}>
                P.{index + 1} {index === 0 ? "(表紙)" : ""}
              </span>
              {index > 0 && (
                <button
                  type="button"
                  className="pill-btn"
                  style={{ padding: "1px 6px", fontSize: 10 }}
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
                  aria-label="前へ移動"
                  onClick={() => movePage(index, -1)}
                >
                  <ArrowLeft size={13} />
                </button>
                <button
                  type="button"
                  className="icon-btn"
                  style={{ inlineSize: 24, blockSize: 24 }}
                  disabled={index === group.pages.length - 1}
                  title="後ろへ移動"
                  aria-label="後ろへ移動"
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
                aria-label="グループから外す"
                onClick={() => void onRemovePage(group.id, page.itemId)}
              >
                <Unlink size={13} />
              </button>
            </div>
          </div>
        ))}
      </div>
    </dialog>
  );
}
