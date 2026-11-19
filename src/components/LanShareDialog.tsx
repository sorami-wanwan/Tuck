import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import {
  Check,
  Clock,
  Copy,
  Edit3,
  Eye,
  MonitorSmartphone,
  Plus,
  Radio,
  RefreshCw,
  ShieldAlert,
  Wifi,
  X,
} from "lucide-react";
import { formatErrorMessage, vaultApi } from "../services/vaultApi";
import type {
  LanAccessMode,
  LanInterfaceInfo,
  LanShareStatus,
  StartLanShareRequest,
  ToastItem,
} from "../types/vault";

interface LanShareDialogProps {
  open: boolean;
  status: LanShareStatus;
  onStatusChange: (next: LanShareStatus) => void;
  onClose: () => void;
  onToast: (tone: ToastItem["tone"], message: string) => void;
}

const DURATION_OPTIONS = [
  { label: "15分", value: 15 },
  { label: "30分（推奨）", value: 30 },
  { label: "1時間", value: 60 },
  { label: "2時間", value: 120 },
  { label: "無制限（ロックまたは手動停止まで）", value: 0 },
];

function formatRemaining(expiresAtMs: number | null, now: number): string {
  if (expiresAtMs === null) return "無制限";
  const diffSec = Math.max(0, Math.floor((expiresAtMs - now) / 1000));
  const hours = Math.floor(diffSec / 3600);
  const mins = Math.floor((diffSec % 3600) / 60);
  const secs = diffSec % 60;
  if (hours > 0) {
    return `${hours}:${String(mins).padStart(2, "0")}:${String(secs).padStart(2, "0")}`;
  }
  return `${String(mins).padStart(2, "0")}:${String(secs).padStart(2, "0")}`;
}

function formatRelativeSeen(lastSeenMs: number, now: number): string {
  const diffSec = Math.max(0, Math.floor((now - lastSeenMs) / 1000));
  if (diffSec < 10) return "たった今";
  if (diffSec < 60) return `${diffSec}秒前`;
  const mins = Math.floor(diffSec / 60);
  if (mins < 60) return `${mins}分前`;
  return `${Math.floor(mins / 60)}時間前`;
}

export function LanShareDialog({
  open,
  status,
  onStatusChange,
  onClose,
  onToast,
}: LanShareDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [mode, setMode] = useState<LanAccessMode>("readOnly");
  const [durationMinutes, setDurationMinutes] = useState<number>(30);
  const [bindIp, setBindIp] = useState<string>("");
  const [pauseIdleLock, setPauseIdleLock] = useState<boolean>(true);
  const [interfaces, setInterfaces] = useState<LanInterfaceInfo[]>(status.interfaces);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copiedField, setCopiedField] = useState<"url" | "pin" | null>(null);
  const [nowMs, setNowMs] = useState(() => Date.now());

  useEffect(() => {
    const dlg = dialogRef.current;
    if (!dlg) return;
    if (open) {
      setError(null);
      if (!dlg.open) dlg.showModal();
      void vaultApi
        .getLanNetworkInterfaces()
        .then((list) => {
          setInterfaces(list);
          setBindIp((prev) => {
            if (prev && list.some((i) => i.ip === prev)) return prev;
            return list[0]?.ip ?? "";
          });
        })
        .catch(() => {});
    } else if (dlg.open) {
      dlg.close();
    }
  }, [open]);

  useEffect(() => {
    if (!open || !status.running) return;
    setNowMs(Date.now());
    const id = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [open, status.running]);

  const qrDataUri = useMemo(() => {
    if (!status.qrSvg) return "";
    return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(status.qrSvg)}`;
  }, [status.qrSvg]);

  const copyText = async (text: string, field: "url" | "pin") => {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedField(field);
      window.setTimeout(() => {
        setCopiedField((cur) => (cur === field ? null : cur));
      }, 1800);
    } catch {
      onToast("error", "クリップボードへのコピーに失敗しました");
    }
  };

  const handleStart = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    setBusy(true);
    try {
      const req: StartLanShareRequest = {
        mode,
        durationMinutes,
        bindIp: bindIp || null,
        pauseIdleLock,
      };
      const next = await vaultApi.startLanShare(req);
      onStatusChange(next);
      onToast("success", "LAN 内での一時公開を開始しました");
    } catch (err) {
      setError(formatErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleStop = async () => {
    setBusy(true);
    setError(null);
    try {
      const next = await vaultApi.stopLanShare();
      onStatusChange(next);
      onToast("info", "LAN 公開を停止しました");
    } catch (err) {
      setError(formatErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleModeChange = async (nextMode: LanAccessMode) => {
    if (nextMode === status.mode || busy) return;
    setBusy(true);
    setError(null);
    try {
      const next = await vaultApi.updateLanShareMode(nextMode);
      onStatusChange(next);
      onToast(
        "info",
        nextMode === "readOnly"
          ? "権限を「閲覧のみ」に切り替えました"
          : "権限を「編集・取り込み可」に切り替えました"
      );
    } catch (err) {
      setError(formatErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleExtendTimer = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const next = await vaultApi.extendLanShareTimer(30);
      onStatusChange(next);
      onToast("success", "公開時間を +30分 延長しました");
    } catch (err) {
      setError(formatErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleRegenerate = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const next = await vaultApi.regenerateLanCredentials(true);
      onStatusChange(next);
      onToast("info", "接続中の全端末を切断し、PIN と QR コードを再発行しました");
    } catch (err) {
      setError(formatErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const selectedInterface = useMemo(
    () => interfaces.find((iface) => iface.ip === bindIp) ?? null,
    [interfaces, bindIp]
  );

  const runningInterfaceIsTailscale = useMemo(() => {
    const match = (interfaces.length > 0 ? interfaces : status.interfaces).find(
      (iface) => iface.ip === status.selectedIp
    );
    return Boolean(match?.isTailscale);
  }, [interfaces, status.interfaces, status.selectedIp]);

  const formattedPin =
    status.pin.length === 6 ? `${status.pin.slice(0, 3)} ${status.pin.slice(3)}` : status.pin;

  return (
    <dialog
      ref={dialogRef}
      className="confirm-dialog lan-share-dialog"
      aria-labelledby="lan-share-title"
      onClose={(e) => {
        e.currentTarget.returnValue = "";
        onClose();
      }}
    >
      <div className="confirm-dialog-inner">
        <div className="lan-dialog-header">
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <Radio
              size={16}
              style={{ color: status.running ? "var(--success)" : "var(--accent-blue)" }}
            />
            <h2 id="lan-share-title">LAN 一時ブラウザ公開</h2>
            {status.running && (
              <span className="lan-live-pill">公開中</span>
            )}
          </div>
          <button
            type="button"
            className="icon-btn"
            aria-label="閉じる"
            onClick={() => dialogRef.current?.close()}
          >
            <X size={16} />
          </button>
        </div>

        {!status.running ? (
          <form onSubmit={handleStart} style={{ display: "flex", flexDirection: "column", gap: 14 }}>
            <p className="lan-lead-text">
              同じ Wi-Fi（ローカルエリアネットワーク）内のスマホ・タブレット・別 PC のブラウザから、この保管庫を一時的に閲覧・操作できます。
            </p>

            <div className="form-field">
              <span style={{ fontSize: 12, fontWeight: 600 }}>許可する操作権限</span>
              <div className="lan-mode-grid">
                <button
                  type="button"
                  className={`lan-mode-card ${mode === "readOnly" ? "active" : ""}`}
                  onClick={() => setMode("readOnly")}
                >
                  <div className="lan-mode-card-head">
                    <Eye size={14} />
                    <span>閲覧のみ（推奨）</span>
                  </div>
                  <p>
                    検索・ギャラリー表示・画像拡大・動画再生・漫画リーダーのみ利用できます。
                  </p>
                </button>

                <button
                  type="button"
                  className={`lan-mode-card ${mode === "readWrite" ? "active" : ""}`}
                  onClick={() => setMode("readWrite")}
                >
                  <div className="lan-mode-card-head">
                    <Edit3 size={14} />
                    <span>編集・取り込みも許可</span>
                  </div>
                  <p>
                    タグ・評価・メモ・フォルダ整理・ブラウザからの写真/動画追加も許可します。
                  </p>
                </button>
              </div>
            </div>

            <div className="settings-grid-2">
              <div className="form-field">
                <label htmlFor="lan-duration-select">自動停止タイマー</label>
                <select
                  id="lan-duration-select"
                  className="toolbar-select"
                  value={durationMinutes}
                  onChange={(e) => setDurationMinutes(Number(e.target.value))}
                >
                  {DURATION_OPTIONS.map((opt) => (
                    <option key={opt.value} value={opt.value}>
                      {opt.label}
                    </option>
                  ))}
                </select>
              </div>

              <div className="form-field">
                <label htmlFor="lan-interface-select">公開ネットワーク</label>
                <select
                  id="lan-interface-select"
                  className="toolbar-select"
                  value={bindIp}
                  onChange={(e) => setBindIp(e.target.value)}
                >
                  {interfaces.length === 0 ? (
                    <option value="">ローカル (127.0.0.1)</option>
                  ) : (
                    interfaces.map((iface) => (
                      <option key={iface.ip} value={iface.ip}>
                        {iface.isTailscale
                          ? `${iface.ip} (Tailscale / ${iface.name})`
                          : `${iface.ip} (${iface.name})`}
                      </option>
                    ))
                  )}
                </select>
              </div>
            </div>

            <label className="checklist-row" style={{ paddingInline: 0 }}>
              <input
                type="checkbox"
                checked={pauseIdleLock}
                onChange={(e) => setPauseIdleLock(e.target.checked)}
              />
              <span>LAN 公開中はホストの無操作自動ロックを一時停止する（手動即時ロックは常に有効）</span>
            </label>

            <div className="lan-security-note">
              <ShieldAlert size={14} style={{ flexShrink: 0, marginTop: 1 }} />
              <span>
                {selectedInterface?.isTailscale
                  ? "選択した Tailscale IP（100.64.0.0/10）のみで待ち受け、Tailscale の暗号化トンネル経由でアクセスできます。起動ごとに 6 桁の PIN と QR トークンを発行し、ホスト側でロックすると全端末を切断します。"
                  : "選択したローカル IP のみで待ち受けます。通信は平文 HTTP のため、自宅など信頼できるローカル Wi-Fi で利用し、公衆 Wi-Fi では使用しないでください。起動ごとに 6 桁の PIN と QR トークンを発行し、ホスト側でロックすると全端末を切断します。"}
              </span>
            </div>

            {error && (
              <div className="error-banner" role="alert">
                {error}
              </div>
            )}

            <div className="confirm-dialog-actions">
              <button
                type="button"
                className="btn"
                onClick={() => dialogRef.current?.close()}
              >
                キャンセル
              </button>
              <button type="submit" className="btn btn-primary" disabled={busy}>
                <Wifi size={14} />
                {busy ? "起動中..." : "LAN 公開を開始"}
              </button>
            </div>
          </form>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
            <div className="lan-running-hero">
              <div className="lan-qr-card">
                {qrDataUri ? (
                  <img
                    src={qrDataUri}
                    alt="LAN 接続用 QR コード"
                    width={176}
                    height={176}
                  />
                ) : null}
                <span>カメラで読取で即接続</span>
              </div>

              <div className="lan-credentials-col">
                <div className="form-field">
                  <span style={{ fontSize: 11, color: "var(--text-muted)" }}>
                    {runningInterfaceIsTailscale ? "アクセス URL（Tailscale 経由）" : "アクセス URL（ローカルネットワーク）"}
                  </span>
                  <div className="lan-copy-box">
                    <code className="lan-url-text">{status.url}</code>
                    <button
                      type="button"
                      className="icon-btn"
                      title="URL をコピー"
                      aria-label="URL をコピー"
                      onClick={() => void copyText(status.url, "url")}
                    >
                      {copiedField === "url" ? <Check size={14} style={{ color: "var(--success)" }} /> : <Copy size={14} />}
                    </button>
                  </div>
                </div>

                <div className="form-field">
                  <span style={{ fontSize: 11, color: "var(--text-muted)" }}>使い捨て 6 桁 PIN（手動接続用）</span>
                  <div className="lan-copy-box">
                    <span className="lan-pin-display tabular-nums">{formattedPin}</span>
                    <button
                      type="button"
                      className="icon-btn"
                      title="PIN をコピー"
                      aria-label="PIN をコピー"
                      onClick={() => void copyText(status.pin, "pin")}
                    >
                      {copiedField === "pin" ? <Check size={14} style={{ color: "var(--success)" }} /> : <Copy size={14} />}
                    </button>
                  </div>
                </div>

                <div className="lan-timer-row">
                  <div style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                    <Clock size={13} style={{ color: "var(--text-secondary)" }} />
                    <span className="tabular-nums" style={{ fontSize: 12, fontWeight: 600 }}>
                      残り {formatRemaining(status.expiresAtMs, nowMs)}
                    </span>
                  </div>
                  {status.expiresAtMs !== null && (
                    <button
                      type="button"
                      className="pill-btn"
                      disabled={busy}
                      onClick={() => void handleExtendTimer()}
                    >
                      <Plus size={11} style={{ marginInlineEnd: 2 }} />
                      30分延長
                    </button>
                  )}
                </div>
              </div>
            </div>

            <div className="lan-live-controls">
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 10, flexWrap: "wrap" }}>
                <span style={{ fontSize: 12, fontWeight: 600 }}>現在の公開権限</span>
                <div className="mode-pill-group" role="group" aria-label="公開権限を切り替え">
                  <button
                    type="button"
                    className={`mode-pill-btn ${status.mode === "readOnly" ? "active" : ""}`}
                    disabled={busy}
                    onClick={() => void handleModeChange("readOnly")}
                  >
                    <Eye size={11} style={{ marginInlineEnd: 4, verticalAlign: "-1px" }} />
                    閲覧のみ
                  </button>
                  <button
                    type="button"
                    className={`mode-pill-btn ${status.mode === "readWrite" ? "active" : ""}`}
                    disabled={busy}
                    onClick={() => void handleModeChange("readWrite")}
                  >
                    <Edit3 size={11} style={{ marginInlineEnd: 4, verticalAlign: "-1px" }} />
                    編集・取り込み可
                  </button>
                </div>
              </div>
            </div>

            <div className="lan-clients-section">
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 }}>
                <span style={{ fontSize: 12, fontWeight: 600, display: "inline-flex", alignItems: "center", gap: 6 }}>
                  <MonitorSmartphone size={13} />
                  接続中の端末 ({status.clients.length})
                </span>
                <button
                  type="button"
                  className="btn"
                  style={{ padding: "4px 10px", fontSize: 11 }}
                  disabled={busy}
                  onClick={() => void handleRegenerate()}
                >
                  <RefreshCw size={12} />
                  全端末を切断してPIN/QR再発行
                </button>
              </div>

              {status.clients.length === 0 ? (
                <div className="lan-clients-empty">
                  まだ接続中の端末はありません（URL または QR コードからアクセス待ち）
                </div>
              ) : (
                <div className="lan-client-list">
                  {status.clients.map((c) => (
                    <div key={c.id} className="lan-client-row">
                      <span className="lan-client-device">{c.deviceLabel}</span>
                      <span className="lan-client-meta tabular-nums">
                        {c.ip} ・ {formatRelativeSeen(c.lastSeenAtMs, nowMs)}
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </div>

            {error && (
              <div className="error-banner" role="alert">
                {error}
              </div>
            )}

            <div className="confirm-dialog-actions">
              <button
                type="button"
                className="btn"
                onClick={() => dialogRef.current?.close()}
              >
                バックグラウンドで公開を続ける
              </button>
              <button
                type="button"
                className="btn btn-danger"
                disabled={busy}
                onClick={() => void handleStop()}
              >
                公開を停止
              </button>
            </div>
          </div>
        )}
      </div>
    </dialog>
  );
}
