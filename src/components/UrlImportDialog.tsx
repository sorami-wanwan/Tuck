import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { AlertCircle, CheckCircle2, Cookie, Link2, Settings } from "lucide-react";
import { isTauriRuntime, subscribeVaultEvent } from "../services/runtime";
import { formatErrorMessage, vaultApi } from "../services/vaultApi";
import { formatProgressBytes } from "../utils/format";
import {
  formatCookieModeLabel,
  type UrlImportProgressEvent,
  type UrlImportSettings,
  type YtdlpStatus,
} from "../types/vault";

interface UrlImportDialogProps {
  open: boolean;
  initialUrl: string;
  autoStart?: boolean;
  urlImportSettings: UrlImportSettings;
  onClose: () => void;
  onOpenSettings: () => void;
  onSaveUrlSettings: (next: UrlImportSettings) => Promise<void>;
  onImportUrl: (url: string) => Promise<void>;
}

export function UrlImportDialog({
  open,
  initialUrl,
  autoStart = false,
  urlImportSettings,
  onClose,
  onOpenSettings,
  onSaveUrlSettings,
  onImportUrl,
}: UrlImportDialogProps) {
  const tauri = isTauriRuntime();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const autoStartedRef = useRef(false);
  const latestAutoEnabledRef = useRef(urlImportSettings.enabled);
  latestAutoEnabledRef.current = urlImportSettings.enabled;

  const [url, setUrl] = useState(initialUrl);
  const [autoOnPaste, setAutoOnPaste] = useState(urlImportSettings.enabled);
  const [ytdlpStatus, setYtdlpStatus] = useState<YtdlpStatus | null>(null);
  const [checkingTool, setCheckingTool] = useState(false);
  const [busy, setBusy] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [progress, setProgress] = useState<UrlImportProgressEvent | null>(null);
  const [error, setError] = useState<string | null>(null);

  const runImport = useCallback(
    async (rawTargetUrl: string, persistAutoFlag: boolean) => {
      const trimmed = rawTargetUrl.trim();
      if (!trimmed) {
        setError("URLを入力してください");
        return;
      }

      let succeeded = false;
      setError(null);
      setCancelling(false);
      setProgress({
        stage: "meta",
        percent: null,
        speed: "",
        eta: "",
        downloadedBytes: 0,
        totalBytes: null,
        message: "URLの解析を準備しています...",
      });
      setBusy(true);
      try {
        if (tauri && persistAutoFlag && autoOnPaste !== urlImportSettings.enabled) {
          await onSaveUrlSettings({
            ...urlImportSettings,
            enabled: autoOnPaste,
          });
        }
        await onImportUrl(trimmed);
        succeeded = true;
      } catch (err) {
        setError(formatErrorMessage(err));
      } finally {
        setBusy(false);
        setCancelling(false);
        setProgress(null);
      }

      if (succeeded) {
        dialogRef.current?.close();
      }
    },
    [tauri, autoOnPaste, onImportUrl, onSaveUrlSettings, urlImportSettings]
  );

  useEffect(() => {
    const dlg = dialogRef.current;
    if (!dlg) return;
    if (open) {
      setUrl(initialUrl);
      setAutoOnPaste(latestAutoEnabledRef.current);
      setError(null);
      setProgress(null);
      setCancelling(false);
      if (!dlg.open) dlg.showModal();
    } else {
      autoStartedRef.current = false;
      setBusy(false);
      setCancelling(false);
      setProgress(null);
      if (dlg.open) dlg.close();
    }
  }, [open, initialUrl]);

  useEffect(() => {
    if (!open) return;
    let active = true;
    setCheckingTool(true);
    void vaultApi
      .checkYtdlpStatus(urlImportSettings.ytdlpPath, urlImportSettings.ffmpegPath)
      .then((st) => {
        if (active) setYtdlpStatus(st);
      })
      .catch(() => {
        if (active) setYtdlpStatus(null);
      })
      .finally(() => {
        if (active) setCheckingTool(false);
      });

    return () => {
      active = false;
    };
  }, [open, urlImportSettings.ytdlpPath, urlImportSettings.ffmpegPath]);

  useEffect(() => {
    if (!open) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    void subscribeVaultEvent<UrlImportProgressEvent>("vault://url-import-progress", (payload) => {
      if (active) {
        setProgress(payload);
      }
    }).then((fn) => {
      if (active) unlisten = fn;
      else fn();
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [open]);

  useEffect(() => {
    if (!open || !autoStart || autoStartedRef.current) return;
    const trimmed = initialUrl.trim();
    if (!trimmed) return;
    autoStartedRef.current = true;
    void runImport(trimmed, false);
  }, [open, autoStart, initialUrl, runImport]);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    await runImport(url, true);
  };

  const handleCancelAction = async () => {
    if (busy) {
      if (cancelling) return;
      setCancelling(true);
      try {
        await vaultApi.cancelUrlImport();
      } catch {
        // Ignore cancel race if already finishing
      }
      return;
    }
    dialogRef.current?.close();
  };

  const cookieLabel = formatCookieModeLabel(urlImportSettings);
  const cookieEnabled = Boolean(urlImportSettings.cookieBrowser);

  return (
    <dialog
      ref={dialogRef}
      className="confirm-dialog url-import-dialog"
      aria-labelledby="url-import-title"
      onCancel={(e) => {
        if (busy) {
          e.preventDefault();
          void handleCancelAction();
        }
      }}
      onClose={(e) => {
        e.currentTarget.returnValue = "";
        onClose();
      }}
    >
      <form className="confirm-dialog-inner" onSubmit={handleSubmit}>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <Link2 size={16} style={{ color: "var(--accent-blue)" }} />
          <h2 id="url-import-title">URLから動画を取り込む</h2>
        </div>

        <div className="form-field">
          <label htmlFor="url-import-input">動画ページのURL（X / TikTok など）</label>
          <input
            id="url-import-input"
            type="url"
            className="inspector-input"
            placeholder="https://x.com/... または https://www.tiktok.com/..."
            value={url}
            disabled={busy}
            autoFocus
            onChange={(e) => setUrl(e.target.value)}
          />
        </div>

        {checkingTool ? (
          <div className="url-status-badge">
            <span>yt-dlp / ffmpeg を確認中...</span>
          </div>
        ) : ytdlpStatus?.available ? (
          <div className="url-status-badge ok">
            <CheckCircle2 size={14} style={{ flexShrink: 0 }} />
            <span style={{ flex: 1 }}>
              yt-dlp 検出済み（{ytdlpStatus.version ?? "利用可能"}）
              {ytdlpStatus.ffmpegAvailable
                ? ` ／ ffmpeg (${ytdlpStatus.ffmpegVersion ?? "OK"})`
                : " ／ ffmpeg 未導入（短尺 MP4 のみ対応）"}
            </span>
          </div>
        ) : (
          <div className="url-status-badge warn">
            <AlertCircle size={14} style={{ flexShrink: 0 }} />
            <span style={{ flex: 1 }}>
              yt-dlp が見つかりません（brew / winget / apt 等で yt-dlp を導入してください）
            </span>
          </div>
        )}

        {isTauriRuntime() && (
          <div className="url-cookie-row">
            <div className="url-cookie-info">
              <Cookie size={13} style={{ flexShrink: 0, color: cookieEnabled ? "var(--accent-blue)" : "var(--text-muted)" }} />
              <span>
                Cookie 連携: <strong>{cookieLabel}</strong>
              </span>
            </div>
            <button
              type="button"
              className="btn"
              style={{ padding: "3px 8px", fontSize: 11 }}
              disabled={busy}
              onClick={() => {
                dialogRef.current?.close();
                onOpenSettings();
              }}
            >
              <Settings size={11} />
              設定
            </button>
          </div>
        )}

        {busy && progress && (
          <div className="url-progress-box" aria-live="polite">
            <div className="url-progress-head">
              <span>
                {cancelling
                  ? "キャンセルしています..."
                  : progress.itemTotal && progress.itemTotal > 1
                  ? `${progress.message} (${progress.itemIndex ?? 1} / ${progress.itemTotal}件)`
                  : progress.message}
              </span>
              <span className="tabular-nums">
                {progress.percent !== null ? `${progress.percent.toFixed(1)}%` : "処理中"}
              </span>
            </div>
            <div className="url-progress-track">
              <div
                className={`url-progress-fill ${progress.percent === null ? "indeterminate" : ""}`}
                style={
                  progress.percent !== null
                    ? { inlineSize: `${Math.max(2, Math.min(100, progress.percent))}%` }
                    : undefined
                }
              />
            </div>
            {(progress.downloadedBytes > 0 || progress.speed || progress.eta) && (
              <div className="url-progress-meta tabular-nums">
                <span>
                  {progress.downloadedBytes > 0
                    ? progress.totalBytes
                      ? `${formatProgressBytes(progress.downloadedBytes)} / ${formatProgressBytes(progress.totalBytes)}`
                      : formatProgressBytes(progress.downloadedBytes)
                    : ""}
                </span>
                <span>
                  {[progress.speed, progress.eta ? `残り ${progress.eta}` : ""]
                    .filter(Boolean)
                    .join(" ・ ")}
                </span>
              </div>
            )}
          </div>
        )}

        {tauri && (
          <label className="checklist-row" style={{ paddingInline: 0 }}>
            <input
              type="checkbox"
              checked={autoOnPaste}
              disabled={busy}
              onChange={(e) => setAutoOnPaste(e.target.checked)}
            />
            <span>今後は URL ペースト時（Cmd/Ctrl + V）に確認なしで自動取り込みする</span>
          </label>
        )}

        <p style={{ fontSize: 11, color: "var(--text-muted)", lineHeight: 1.55 }}>
          ※ 外部コマンド <code>yt-dlp</code> を用いて指定URLのみへ通信し、取得した動画は一時ファイルを残さず暗号化保管庫へ保存します（HLS・長尺変換時は処理完了直後に一時ファイルを自動消去します）。
        </p>

        {error && (
          <div className="error-banner" role="alert">
            {error}
          </div>
        )}

        <div className="confirm-dialog-actions">
          <button
            type="button"
            className="btn"
            disabled={cancelling}
            onClick={() => void handleCancelAction()}
          >
            {busy ? (cancelling ? "キャンセル中..." : "中止する") : "キャンセル"}
          </button>
          <button
            type="submit"
            className="btn btn-primary"
            disabled={busy}
          >
            {busy ? "取得中..." : "取り込む"}
          </button>
        </div>
      </form>
    </dialog>
  );
}
