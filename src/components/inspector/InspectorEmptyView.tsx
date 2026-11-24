import { X } from "lucide-react";
import type { VaultStatus } from "../../types/vault";
import { formatBytes } from "../../utils/format";
import { formatShortcutLabel } from "../../utils/formatShortcut";

export interface InspectorEmptyViewProps {
  status: VaultStatus;
  readOnly: boolean;
  isWebClient: boolean;
  onClose?: () => void;
}

export function InspectorEmptyView({
  status,
  readOnly,
  isWebClient,
  onClose,
}: InspectorEmptyViewProps) {
  return (
    <div className="inspector-scroll">
      {onClose && (
        <div className="inspector-drawer-bar">
          <span>保管庫の概要</span>
          <button
            type="button"
            className="icon-btn"
            aria-label="詳細パネルを閉じる"
            onClick={onClose}
          >
            <X size={15} />
          </button>
        </div>
      )}
      <div
        style={{
          padding: "20px 14px",
          borderRadius: "var(--radius-md)",
          background: "var(--bg-elevated)",
          border: "1px solid var(--border-subtle)",
          display: "flex",
          flexDirection: "column",
          gap: 8,
        }}
      >
        <div style={{ fontWeight: 600 }}>保管庫の概要</div>
        <div className="info-table">
          <div className="info-row">
            <span className="info-key">登録数</span>
            <span className="info-val">{status.itemCount} 件</span>
          </div>
          <div className="info-row">
            <span className="info-key">未分類</span>
            <span className="info-val">{status.uncategorizedCount} 件</span>
          </div>
          <div className="info-row">
            <span className="info-key">合計容量</span>
            <span className="info-val">{formatBytes(status.totalBytes)}</span>
          </div>
        </div>
      </div>

      <div style={{ color: "var(--text-muted)", fontSize: 12, lineHeight: 1.7 }}>
        <div style={{ fontWeight: 600, color: "var(--text-secondary)", marginBottom: 4 }}>
          操作ヒント
        </div>
        {isWebClient ? (
          <>
            <div>・タップ / クリック：画像・動画・漫画を開く</div>
            <div>・2本指ピンチ：拡大・縮小</div>
            <div>・左右スワイプ：前後のページ / 画像へ移動</div>
            <div>
              ・{readOnly ? "現在は閲覧専用モードです" : "LAN 編集モードで整理・アップロード可"}
            </div>
          </>
        ) : (
          <>
            <div>・Cmd/Ctrl + クリック：複数選択</div>
            <div>・フォルダ / タグへドラッグ：追加・タグ付け</div>
            <div>・{formatShortcutLabel(status.lockSettings.shortcut)}：即時ロック</div>
            <div>・Cmd/Ctrl + V：クリップボードから取り込み</div>
          </>
        )}
      </div>
    </div>
  );
}
