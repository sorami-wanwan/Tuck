import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { AlertCircle, CheckCircle2, FolderOpen, RefreshCw } from "lucide-react";
import { vaultApi } from "../services/vaultApi";
import {
  COOKIE_BROWSER_OPTIONS,
  type LockSettings,
  type UrlImportSettings,
  type YtdlpStatus,
} from "../types/vault";
import { formatShortcutLabel } from "../utils/formatShortcut";

interface LockSettingsDialogProps {
  open: boolean;
  settings: LockSettings;
  urlImportSettings: UrlImportSettings;
  skipPlainExportConfirm: boolean;
  onClose: () => void;
  onSave: (
    next: LockSettings,
    urlNext: UrlImportSettings,
    skipPlainExportConfirm: boolean
  ) => Promise<void>;
}

const PRESET_SHORTCUTS = [
  { label: "Cmd/Ctrl + Shift + L", value: "CommandOrControl+Shift+L" },
  { label: "Cmd/Ctrl + L", value: "CommandOrControl+L" },
  { label: "Alt + Shift + L", value: "Alt+Shift+L" },
  { label: "F12", value: "F12" },
];

const TIMEOUT_OPTIONS = [
  { label: "無効", value: 0 },
  { label: "1分", value: 1 },
  { label: "3分", value: 3 },
  { label: "5分", value: 5 },
  { label: "10分", value: 10 },
  { label: "15分", value: 15 },
  { label: "30分", value: 30 },
];

const MAX_SIZE_OPTIONS = [
  { label: "256 MB", value: 256 },
  { label: "512 MB", value: 512 },
  { label: "1 GB", value: 1024 },
  { label: "2 GB（推奨）", value: 2048 },
  { label: "4 GB", value: 4096 },
  { label: "無制限", value: 0 },
];

const MAX_HEIGHT_OPTIONS = [
  { label: "制限なし（最高画質）", value: 0 },
  { label: "1080p まで", value: 1080 },
  { label: "720p まで", value: 720 },
  { label: "480p まで", value: 480 },
];

function cookieGuideText(cookieBrowser: string): string {
  switch (cookieBrowser) {
    case "":
      return "X（Twitter）のセンシティブ・年齢制限・ログイン必須動画を取り込む場合は、ログイン済みブラウザを選択してください。";
    case "arc":
      return "Arc の複数プロファイル（Spaces）から対象サイトにログイン済みのプロファイルを自動検出し、対象ドメインの Cookie のみ最小権限で一時利用します。初回は macOS のキーチェーン確認（Arc Safe Storage、「常に許可」推奨）が表示されます。";
    case "safari":
      return "macOS で Safari の Cookie を読み込むには「システム設定 > プライバシーとセキュリティ > フルディスクアクセス」で Tuck（または開発中のターミナル）の許可が必要です。";
    case "file":
      return "Netscape 形式の cookies.txt を指定します（ブラウザ拡張機能等で書き出したファイル）。キーチェーン権限エラー時の回避策としても有効です。";
    default:
      return "選択したブラウザのログイン済みセッションを使用して取得します。macOS では初回のみキーチェーン許可（「常に許可」推奨）が表示されます。";
  }
}

function formatKeyEventToShortcut(e: KeyboardEvent<HTMLInputElement>): string | null {
  const mods: string[] = [];
  if (e.metaKey || e.ctrlKey) mods.push("CommandOrControl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");

  const key = e.key;
  if (["Meta", "Control", "Alt", "Shift"].includes(key)) {
    return null;
  }

  let codeKey = "";
  if (/^[a-zA-Z0-9]$/.test(key)) {
    codeKey = key.toUpperCase();
  } else if (/^F([1-9]|1[0-2])$/.test(key)) {
    codeKey = key;
  } else if (key === "Escape") {
    return null;
  } else {
    return null;
  }

  if (mods.length === 0 && !codeKey.startsWith("F")) {
    return null;
  }
  return [...mods, codeKey].join("+");
}

export function LockSettingsDialog({
  open,
  settings,
  urlImportSettings,
  skipPlainExportConfirm,
  onClose,
  onSave,
}: LockSettingsDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const wasOpenRef = useRef(false);
  const [shortcut, setShortcut] = useState(settings.shortcut);
  const [idleMinutes, setIdleMinutes] = useState(settings.idleTimeoutMinutes);
  const [urlAutoEnabled, setUrlAutoEnabled] = useState(urlImportSettings.enabled);
  const [ytdlpPath, setYtdlpPath] = useState(urlImportSettings.ytdlpPath);
  const [ffmpegPath, setFfmpegPath] = useState(urlImportSettings.ffmpegPath);
  const [cookieBrowser, setCookieBrowser] = useState(urlImportSettings.cookieBrowser);
  const [cookiesFile, setCookiesFile] = useState(urlImportSettings.cookiesFile);
  const [maxVideoSizeMb, setMaxVideoSizeMb] = useState(urlImportSettings.maxVideoSizeMb);
  const [maxVideoHeight, setMaxVideoHeight] = useState(urlImportSettings.maxVideoHeight);
  const [skipExportConfirm, setSkipExportConfirm] = useState(skipPlainExportConfirm);
  const [ytdlpStatus, setYtdlpStatus] = useState<YtdlpStatus | null>(null);
  const [checkingYtdlp, setCheckingYtdlp] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const checkTool = async (ytdlpOverride: string, ffmpegOverride: string) => {
    setCheckingYtdlp(true);
    try {
      const st = await vaultApi.checkYtdlpStatus(ytdlpOverride, ffmpegOverride);
      setYtdlpStatus(st);
    } catch {
      setYtdlpStatus(null);
    } finally {
      setCheckingYtdlp(false);
    }
  };

  useEffect(() => {
    const dlg = dialogRef.current;
    if (!dlg) return;
    const justOpened = open && !wasOpenRef.current;
    wasOpenRef.current = open;
    if (open) {
      if (justOpened) {
        setShortcut(settings.shortcut);
        setIdleMinutes(settings.idleTimeoutMinutes);
        setUrlAutoEnabled(urlImportSettings.enabled);
        setYtdlpPath(urlImportSettings.ytdlpPath);
        setFfmpegPath(urlImportSettings.ffmpegPath);
        setCookieBrowser(urlImportSettings.cookieBrowser);
        setCookiesFile(urlImportSettings.cookiesFile);
        setMaxVideoSizeMb(urlImportSettings.maxVideoSizeMb);
        setMaxVideoHeight(urlImportSettings.maxVideoHeight);
        setSkipExportConfirm(skipPlainExportConfirm);
        setFormError(null);
        void checkTool(urlImportSettings.ytdlpPath, urlImportSettings.ffmpegPath);
      }
      if (!dlg.open) dlg.showModal();
    } else if (dlg.open) {
      dlg.close();
    }
  }, [open, settings, urlImportSettings, skipPlainExportConfirm]);

  const handleBrowseYtdlp = async () => {
    const selected = await openDialog({
      title: "yt-dlp 実行ファイルを選択",
      multiple: false,
      directory: false,
    });
    if (typeof selected === "string" && selected.length > 0) {
      setYtdlpPath(selected);
      void checkTool(selected, ffmpegPath);
    }
  };

  const handleBrowseFfmpeg = async () => {
    const selected = await openDialog({
      title: "ffmpeg 実行ファイルを選択",
      multiple: false,
      directory: false,
    });
    if (typeof selected === "string" && selected.length > 0) {
      setFfmpegPath(selected);
      void checkTool(ytdlpPath, selected);
    }
  };

  const handleBrowseCookiesFile = async () => {
    const selected = await openDialog({
      title: "cookies.txt ファイルを選択",
      multiple: false,
      directory: false,
      filters: [{ name: "Netscape Cookies", extensions: ["txt"] }],
    });
    if (typeof selected === "string" && selected.length > 0) {
      setCookiesFile(selected);
      setFormError(null);
    }
  };

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (cookieBrowser === "file" && !cookiesFile.trim()) {
      setFormError("cookies.txt ファイルを指定する場合は、ファイルを選択してください");
      return;
    }
    setFormError(null);
    setSaving(true);
    try {
      await onSave(
        { shortcut, idleTimeoutMinutes: idleMinutes },
        {
          enabled: urlAutoEnabled,
          ytdlpPath: ytdlpPath.trim(),
          ffmpegPath: ffmpegPath.trim(),
          cookieBrowser,
          cookiesFile: cookiesFile.trim(),
          maxVideoSizeMb,
          maxVideoHeight,
        },
        skipExportConfirm
      );
      dialogRef.current?.close();
    } finally {
      setSaving(false);
    }
  };

  return (
    <dialog
      ref={dialogRef}
      className="confirm-dialog url-import-dialog"
      aria-labelledby="lock-settings-title"
      onClose={(e) => {
        e.currentTarget.returnValue = "";
        onClose();
      }}
    >
      <form className="confirm-dialog-inner" onSubmit={handleSubmit}>
        <h2 id="lock-settings-title">設定</h2>

        <div className="form-field">
          <label htmlFor="lock-shortcut-input">即時ロック ショートカットキー</label>
          <input
            id="lock-shortcut-input"
            type="text"
            className="inspector-input"
            value={formatShortcutLabel(shortcut)}
            readOnly
            onKeyDown={(e) => {
              if (e.key === "Escape" || e.key === "Tab") return;
              e.preventDefault();
              const captured = formatKeyEventToShortcut(e);
              if (captured) setShortcut(captured);
            }}
          />
          <span style={{ fontSize: 11, color: "var(--text-muted)" }}>
            入力欄を選択してキーを押すか、下のプリセットから選べます
          </span>
          <div style={{ display: "flex", flexWrap: "wrap", gap: 4 }}>
            {PRESET_SHORTCUTS.map((preset) => (
              <button
                key={preset.value}
                type="button"
                className={`pill-btn ${shortcut === preset.value ? "active" : ""}`}
                onClick={() => setShortcut(preset.value)}
              >
                {preset.label}
              </button>
            ))}
          </div>
        </div>

        <div className="form-field">
          <label htmlFor="lock-timeout-select">無操作による自動ロック</label>
          <select
            id="lock-timeout-select"
            className="toolbar-select"
            value={idleMinutes}
            onChange={(e) => setIdleMinutes(Number(e.target.value))}
          >
            {TIMEOUT_OPTIONS.map((opt) => (
              <option key={opt.value} value={opt.value}>
                {opt.label}
              </option>
            ))}
          </select>
        </div>

        <div className="settings-divider" />

        <div className="form-field">
          <span style={{ fontSize: 13, fontWeight: 600 }}>URL動画取り込み（yt-dlp / ffmpeg 連携）</span>
          <label className="checklist-row" style={{ paddingInline: 0 }}>
            <input
              type="checkbox"
              checked={urlAutoEnabled}
              onChange={(e) => setUrlAutoEnabled(e.target.checked)}
            />
            <span>URLペースト（Cmd/Ctrl + V）時に確認なしで即取り込む</span>
          </label>
          <span style={{ fontSize: 11, color: "var(--text-muted)", lineHeight: 1.5 }}>
            オフのままでも、URLを貼り付けると確認ダイアログが開きます。
          </span>
        </div>

        <div className="form-field">
          <label htmlFor="ytdlp-path-input">yt-dlp のパス（空欄で自動検出）</label>
          <div className="path-picker-row">
            <input
              id="ytdlp-path-input"
              type="text"
              className="inspector-input"
              placeholder="自動検出（PATH 等から自動検索）"
              value={ytdlpPath}
              onChange={(e) => setYtdlpPath(e.target.value)}
            />
            <button type="button" className="btn" onClick={() => void handleBrowseYtdlp()}>
              <FolderOpen size={14} />
              選択
            </button>
            <button
              type="button"
              className="btn"
              disabled={checkingYtdlp}
              onClick={() => void checkTool(ytdlpPath, ffmpegPath)}
            >
              <RefreshCw size={13} />
              確認
            </button>
          </div>
          {checkingYtdlp ? (
            <div className="url-status-badge">ツール状態を確認中...</div>
          ) : ytdlpStatus?.available ? (
            <div className="url-status-badge ok">
              <CheckCircle2 size={13} style={{ flexShrink: 0 }} />
              <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                yt-dlp 検出済み ({ytdlpStatus.version ?? "OK"}) : {ytdlpStatus.resolvedPath}
              </span>
            </div>
          ) : ytdlpStatus ? (
            <div className="url-status-badge warn">
              <AlertCircle size={13} style={{ flexShrink: 0 }} />
              <span>
                {ytdlpStatus.error ?? "yt-dlp が見つかりません（brew / winget / apt 等でインストールしてください）"}
              </span>
            </div>
          ) : null}
        </div>

        <div className="form-field">
          <label htmlFor="ffmpeg-path-input">ffmpeg のパス（空欄で自動検出・長尺 / HLS 用）</label>
          <div className="path-picker-row">
            <input
              id="ffmpeg-path-input"
              type="text"
              className="inspector-input"
              placeholder="自動検出（PATH 等から自動検索）"
              value={ffmpegPath}
              onChange={(e) => setFfmpegPath(e.target.value)}
            />
            <button type="button" className="btn" onClick={() => void handleBrowseFfmpeg()}>
              <FolderOpen size={14} />
              選択
            </button>
          </div>
          {!checkingYtdlp && ytdlpStatus && (
            ytdlpStatus.ffmpegAvailable ? (
              <div className="url-status-badge ok">
                <CheckCircle2 size={13} style={{ flexShrink: 0 }} />
                <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                  ffmpeg 検出済み ({ytdlpStatus.ffmpegVersion ?? "OK"}) : {ytdlpStatus.ffmpegPath}
                </span>
              </div>
            ) : (
              <div className="url-status-badge">
                <AlertCircle size={13} style={{ flexShrink: 0 }} />
                <span>
                  ffmpeg 未検出（ffmpeg を導入すると HLS・高画質長尺動画にも対応します）
                </span>
              </div>
            )
          )}
        </div>

        <div className="form-field">
          <label htmlFor="cookie-browser-select">認証・Cookie 連携</label>
          <select
            id="cookie-browser-select"
            className="toolbar-select"
            value={cookieBrowser}
            onChange={(e) => {
              setCookieBrowser(e.target.value);
              setFormError(null);
            }}
          >
            {COOKIE_BROWSER_OPTIONS.map((opt) => (
              <option key={opt.value} value={opt.value}>
                {opt.label}
              </option>
            ))}
          </select>

          {cookieBrowser === "file" && (
            <div className="path-picker-row" style={{ marginTop: 4 }}>
              <input
                id="cookies-file-input"
                type="text"
                className="inspector-input"
                placeholder="cookies.txt のパスを選択..."
                value={cookiesFile}
                onChange={(e) => {
                  setCookiesFile(e.target.value);
                  setFormError(null);
                }}
              />
              <button
                type="button"
                className="btn"
                onClick={() => void handleBrowseCookiesFile()}
              >
                <FolderOpen size={14} />
                選択
              </button>
            </div>
          )}

          <span style={{ fontSize: 11, color: "var(--text-muted)", lineHeight: 1.55 }}>
            {cookieGuideText(cookieBrowser)}
          </span>
        </div>

        <div className="settings-grid-2">
          <div className="form-field">
            <label htmlFor="max-video-size-select">動画サイズ上限</label>
            <select
              id="max-video-size-select"
              className="toolbar-select"
              value={maxVideoSizeMb}
              onChange={(e) => setMaxVideoSizeMb(Number(e.target.value))}
            >
              {MAX_SIZE_OPTIONS.map((opt) => (
                <option key={opt.value} value={opt.value}>
                  {opt.label}
                </option>
              ))}
            </select>
          </div>

          <div className="form-field">
            <label htmlFor="max-video-height-select">最大画質</label>
            <select
              id="max-video-height-select"
              className="toolbar-select"
              value={maxVideoHeight}
              onChange={(e) => setMaxVideoHeight(Number(e.target.value))}
            >
              {MAX_HEIGHT_OPTIONS.map((opt) => (
                <option key={opt.value} value={opt.value}>
                  {opt.label}
                </option>
              ))}
            </select>
          </div>
        </div>

        <div className="settings-divider" />

        <div className="form-field">
          <span style={{ fontSize: 13, fontWeight: 600 }}>通常ファイル書き出し</span>
          <label className="checklist-row" style={{ paddingInline: 0 }}>
            <input
              type="checkbox"
              checked={!skipExportConfirm}
              onChange={(e) => setSkipExportConfirm(!e.target.checked)}
            />
            <span>通常のファイルとして書き出す前に確認ダイアログを表示する</span>
          </label>
        </div>

        {formError && (
          <div className="error-banner" role="alert">
            {formError}
          </div>
        )}

        <div className="confirm-dialog-actions">
          <button
            type="button"
            className="btn"
            autoFocus
            onClick={() => dialogRef.current?.close()}
          >
            キャンセル
          </button>
          <button type="submit" className="btn btn-primary" disabled={saving}>
            {saving ? "保存中..." : "設定を保存"}
          </button>
        </div>
      </form>
    </dialog>
  );
}
