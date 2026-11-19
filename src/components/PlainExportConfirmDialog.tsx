import { useEffect, useRef, useState } from "react";
import { Download } from "lucide-react";
import { isVideoMime, type PlainExportTarget } from "../types/vault";

interface PlainExportConfirmDialogProps {
  target: PlainExportTarget | null;
  isWebClient: boolean;
  busy: boolean;
  onCancel: () => void;
  onConfirm: (options: { includeSubfolders: boolean; dontAskAgain: boolean }) => void;
}

function summarizeTarget(target: PlainExportTarget): string {
  switch (target.kind) {
    case "item": {
      const kindLabel = isVideoMime(target.mimeType) ? "動画" : "画像";
      const name = target.title.trim() || (isVideoMime(target.mimeType) ? "無題の動画" : "無題の画像");
      return `${kindLabel}「${name}」`;
    }
    case "group": {
      const name = target.title.trim() || "無題の漫画";
      const pages = typeof target.pageCount === "number" ? `（${target.pageCount}ページ）` : "";
      return `漫画グループ「${name}」${pages}`;
    }
    case "entries": {
      const parts: string[] = [];
      if (target.imageCount > 0) parts.push(`画像・動画 ${target.imageCount}件`);
      if (target.groupCount > 0) parts.push(`漫画 ${target.groupCount}冊`);
      const detail = parts.length > 0 ? `（${parts.join(" / ")}）` : "";
      return `選択した ${target.entries.length}件${detail}`;
    }
    case "folder":
      return `フォルダ「${target.folderName}」`;
  }
}

/**
 * 平文ファイル書き出し・ブラウザダウンロード実行前の確認ダイアログ。
 */
export function PlainExportConfirmDialog({
  target,
  isWebClient,
  busy,
  onCancel,
  onConfirm,
}: PlainExportConfirmDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [includeSubfolders, setIncludeSubfolders] = useState(true);
  const [dontAskAgain, setDontAskAgain] = useState(false);

  const open = target !== null;

  useEffect(() => {
    const dlg = dialogRef.current;
    if (!dlg) return;
    if (open) {
      setIncludeSubfolders(true);
      setDontAskAgain(false);
      if (!dlg.open) dlg.showModal();
    } else if (dlg.open) {
      dlg.close();
    }
  }, [open, target]);

  const title = isWebClient
    ? "通常のファイルとしてダウンロードしますか？"
    : "通常のファイルとして書き出しますか？";

  const warningText = isWebClient
    ? "暗号化を解除し、通常のファイルとしてこの端末へダウンロードします。保存したファイルは暗号化されないため、Tuck の保護対象外になります。"
    : "暗号化を解除し、元の画像・動画ファイルとして指定した保存先へ書き出します。書き出したファイルは暗号化されないため、Tuck の保護対象外になります。";

  return (
    <dialog
      ref={dialogRef}
      className="confirm-dialog"
      aria-labelledby="plain-export-confirm-title"
      onCancel={(e) => {
        if (busy) {
          e.preventDefault();
          return;
        }
        e.preventDefault();
        onCancel();
      }}
    >
      <div className="confirm-dialog-inner">
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <Download size={16} style={{ color: "var(--accent-blue)", flexShrink: 0 }} />
          <h2 id="plain-export-confirm-title">{title}</h2>
        </div>

        {target && (
          <div className="plain-export-summary">{summarizeTarget(target)}</div>
        )}

        <p>{warningText}</p>

        {isWebClient && target && target.kind !== "item" && (
          <p className="plain-export-zip-note">
            複数ファイル・漫画グループ・フォルダは ZIP ファイルにまとめてダウンロードします。
          </p>
        )}

        <div className="plain-export-options">
          {target?.kind === "folder" && (
            <label className="checklist-row" style={{ paddingInline: 0 }}>
              <input
                type="checkbox"
                checked={includeSubfolders}
                disabled={busy}
                onChange={(e) => setIncludeSubfolders(e.target.checked)}
              />
              <span>{isWebClient ? "子フォルダも階層ごと含める" : "子フォルダも階層ごと書き出す"}</span>
            </label>
          )}

          <label className="checklist-row" style={{ paddingInline: 0 }}>
            <input
              type="checkbox"
              checked={dontAskAgain}
              disabled={busy}
              onChange={(e) => setDontAskAgain(e.target.checked)}
            />
            <span>
              {isWebClient
                ? "この接続中は今後確認しない"
                : "今後は確認しない（設定から元に戻せます）"}
            </span>
          </label>
        </div>

        <div className="confirm-dialog-actions">
          <button type="button" className="btn" disabled={busy} onClick={onCancel}>
            キャンセル
          </button>
          <button
            type="button"
            className="btn btn-primary"
            autoFocus
            disabled={busy || !target}
            onClick={() => onConfirm({ includeSubfolders, dontAskAgain })}
          >
            <Download size={13} />
            {isWebClient ? "ダウンロード" : "書き出し先を選ぶ..."}
          </button>
        </div>
      </div>
    </dialog>
  );
}
