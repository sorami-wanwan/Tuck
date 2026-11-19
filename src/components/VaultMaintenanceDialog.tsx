import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import {
  AlertCircle,
  Archive,
  CheckCircle2,
  Download,
  Eye,
  EyeOff,
  Folder,
  FolderOpen,
  HardDrive,
  RefreshCw,
  ShieldCheck,
  Sparkles,
  Upload,
  X,
} from "lucide-react";
import { imageMemoryCache } from "../services/imageCache";
import { formatErrorMessage, vaultApi } from "../services/vaultApi";
import { formatBytes } from "../utils/format";
import type {
  BackupReport,
  EntryRef,
  FolderNode,
  OptimizeReport,
  PackageExportReport,
  PackageImportReport,
  TagWithCount,
  ThumbnailRegenScope,
  ThumbnailStats,
  VaultHealthInfo,
} from "../types/vault";

export type MaintenanceTab = "maintenance" | "export" | "import";

interface VaultMaintenanceDialogProps {
  open: boolean;
  initialTab: MaintenanceTab;
  folders: FolderNode[];
  tags: TagWithCount[];
  preselectedFolderIds: string[];
  preselectedEntries: EntryRef[];
  onClose: () => void;
  onDataChanged: (avifPendingIds?: string[], videoPendingIds?: string[]) => Promise<void>;
  onToast: (tone: "info" | "success" | "warning" | "error", msg: string) => void;
}

function buildBackupFilename(): string {
  const d = new Date();
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `tuck-backup-${y}${m}${day}.vault`;
}

export function VaultMaintenanceDialog({
  open,
  initialTab,
  folders,
  tags,
  preselectedFolderIds,
  preselectedEntries,
  onClose,
  onDataChanged,
  onToast,
}: VaultMaintenanceDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [tab, setTab] = useState<MaintenanceTab>(initialTab);

  // Tab 1: Health, Optimize & Backup
  const [health, setHealth] = useState<VaultHealthInfo | null>(null);
  const [healthLoading, setHealthLoading] = useState(false);
  const [optimizing, setOptimizing] = useState(false);
  const [optimizeReport, setOptimizeReport] = useState<OptimizeReport | null>(null);
  const [backingUp, setBackingUp] = useState(false);
  const [backupReport, setBackupReport] = useState<BackupReport | null>(null);
  const [maintError, setMaintError] = useState<string | null>(null);
  const [thumbStats, setThumbStats] = useState<ThumbnailStats | null>(null);
  const [thumbScope, setThumbScope] = useState<Exclude<ThumbnailRegenScope, "entries">>("missingOnly");
  const [thumbRegening, setThumbRegening] = useState(false);
  const [thumbProgress, setThumbProgress] = useState<{
    done: number;
    total: number;
    succeeded: number;
    failed: number;
  } | null>(null);
  const [thumbReport, setThumbReport] = useState<{
    total: number;
    succeeded: number;
    failed: number;
    cancelled: boolean;
  } | null>(null);
  const thumbCancelRef = useRef(false);
  const thumbScopeTouchedRef = useRef(false);

  // Tab 2: Export Partial Package
  const [includePreselectedEntries, setIncludePreselectedEntries] = useState(true);
  const [exportFolderIds, setExportFolderIds] = useState<Set<string>>(() => new Set());
  const [includeSubfolders, setIncludeSubfolders] = useState(true);
  const [exportTagNames, setExportTagNames] = useState<Set<string>>(() => new Set());
  const [exportPath, setExportPath] = useState("");
  const [exportPickedViaSave, setExportPickedViaSave] = useState(false);
  const [exportPass, setExportPass] = useState("");
  const [exportConfirmPass, setExportConfirmPass] = useState("");
  const [showExportPass, setShowExportPass] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [exportError, setExportError] = useState<string | null>(null);
  const [exportReport, setExportReport] = useState<PackageExportReport | null>(null);

  // Tab 3: Import Partial Package
  const [importPath, setImportPath] = useState("");
  const [importPass, setImportPass] = useState("");
  const [showImportPass, setShowImportPass] = useState(false);
  const [targetFolderId, setTargetFolderId] = useState<string>("");
  const [importing, setImporting] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);
  const [importReport, setImportReport] = useState<PackageImportReport | null>(null);

  const loadHealth = async () => {
    setHealthLoading(true);
    setMaintError(null);
    try {
      const info = await vaultApi.getVaultHealth();
      setHealth(info);
    } catch (err) {
      setMaintError(formatErrorMessage(err));
    } finally {
      setHealthLoading(false);
    }
  };

  const loadThumbStats = async (): Promise<ThumbnailStats | null> => {
    try {
      const st = await vaultApi.getThumbnailStats();
      setThumbStats(st);
      return st;
    } catch {
      return null;
    }
  };

  useEffect(() => {
    const dlg = dialogRef.current;
    if (!dlg) return;
    if (open) {
      setTab(initialTab);
      setOptimizeReport(null);
      setBackupReport(null);
      setMaintError(null);
      setThumbRegening(false);
      setThumbProgress(null);
      setThumbReport(null);
      thumbCancelRef.current = false;
      thumbScopeTouchedRef.current = false;
      setExportError(null);
      setExportReport(null);
      setExportPass("");
      setExportConfirmPass("");
      setExportPath("");
      setExportPickedViaSave(false);
      setExportFolderIds(new Set(preselectedFolderIds));
      setIncludePreselectedEntries(preselectedEntries.length > 0);
      setExportTagNames(new Set());
      setImportError(null);
      setImportReport(null);
      setImportPass("");
      setImportPath("");
      setTargetFolderId("");

      if (!dlg.open) dlg.showModal();
      void loadHealth();
      void loadThumbStats().then((st) => {
        if (st && !thumbScopeTouchedRef.current) {
          setThumbScope(
            st.missingCount > 0 ? "missingOnly" : st.videoCount > 0 ? "videosOnly" : "all"
          );
        }
      });
    } else if (dlg.open) {
      thumbCancelRef.current = true;
      dlg.close();
    }
  }, [open, initialTab, preselectedFolderIds, preselectedEntries]);

  const folderDepths = useMemo(() => {
    const map = new Map<string, FolderNode>();
    for (const f of folders) map.set(f.id, f);
    const ordered: Array<{ folder: FolderNode; depth: number }> = [];
    const walk = (parentId: string | null, depth: number) => {
      for (const f of folders) {
        if (f.parentId === parentId) {
          ordered.push({ folder: f, depth });
          walk(f.id, depth + 1);
        }
      }
    };
    walk(null, 0);
    return ordered;
  }, [folders]);

  const handleOptimize = async () => {
    setOptimizing(true);
    setMaintError(null);
    setOptimizeReport(null);
    try {
      const rep = await vaultApi.optimizeVault();
      setOptimizeReport(rep);
      await loadHealth();
      onToast(
        "success",
        rep.reclaimedBytes > 0
          ? `保管庫を最適化し ${formatBytes(rep.reclaimedBytes)} 削減しました`
          : "保管庫を最適化しました"
      );
    } catch (err) {
      setMaintError(formatErrorMessage(err));
    } finally {
      setOptimizing(false);
    }
  };

  const handleBackup = async () => {
    setMaintError(null);
    setBackupReport(null);
    try {
      const selected = await saveDialog({
        title: "バックアップの保存先を選択",
        defaultPath: buildBackupFilename(),
        filters: [{ name: "Tuck", extensions: ["vault"] }],
      });
      if (!selected) return;
      const destPath = selected.endsWith(".vault") ? selected : `${selected}.vault`;
      setBackingUp(true);
      const rep = await vaultApi.backupVault(destPath, true);
      setBackupReport(rep);
      onToast("success", `バックアップを保存しました (${formatBytes(rep.bytesWritten)})`);
    } catch (err) {
      setMaintError(formatErrorMessage(err));
    } finally {
      setBackingUp(false);
    }
  };

  const handleRegenThumbnails = async () => {
    if (thumbRegening) return;
    setMaintError(null);
    setThumbReport(null);
    thumbCancelRef.current = false;
    setThumbRegening(true);
    try {
      const targets = await vaultApi.listThumbnailTargets(thumbScope, []);
      if (targets.length === 0) {
        setThumbReport({ total: 0, succeeded: 0, failed: 0, cancelled: false });
        onToast("info", "再生成対象のアイテムはありません");
        return;
      }

      let done = 0;
      let succeeded = 0;
      let failed = 0;
      setThumbProgress({ done: 0, total: targets.length, succeeded: 0, failed: 0 });

      for (const target of targets) {
        if (thumbCancelRef.current) break;
        try {
          await imageMemoryCache.regenerateItemThumbnail(target.itemId, target.mimeType);
          succeeded += 1;
        } catch {
          failed += 1;
        }
        done += 1;
        setThumbProgress({ done, total: targets.length, succeeded, failed });
      }

      const cancelled = thumbCancelRef.current;
      setThumbReport({ total: targets.length, succeeded, failed, cancelled });
      await onDataChanged();
      await Promise.all([loadHealth(), loadThumbStats()]);

      if (cancelled) {
        onToast("info", `サムネイル再生成を中断しました（${succeeded}件完了）`);
      } else if (failed === 0) {
        onToast("success", `${succeeded}件のサムネイルを再生成しました`);
      } else {
        onToast("warning", `${succeeded}件を再生成しました（${failed}件失敗）`);
      }
    } catch (err) {
      setMaintError(formatErrorMessage(err));
    } finally {
      setThumbRegening(false);
      setThumbProgress(null);
    }
  };

  const handleBrowseExportDest = async () => {
    setExportError(null);
    const selected = await saveDialog({
      title: "パッケージの保存先を指定",
      defaultPath: "share.tuckpack",
      filters: [{ name: "Tuck パッケージ", extensions: ["tuckpack", "vault"] }],
    });
    if (selected) {
      const normalized =
        selected.endsWith(".tuckpack") || selected.endsWith(".vault")
          ? selected
          : `${selected}.tuckpack`;
      setExportPath(normalized);
      setExportPickedViaSave(true);
    }
  };

  const handleExportSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setExportError(null);
    setExportReport(null);

    const activeEntries = includePreselectedEntries ? preselectedEntries : [];
    const activeFolders = Array.from(exportFolderIds);
    const activeTags = Array.from(exportTagNames);

    if (activeEntries.length === 0 && activeFolders.length === 0 && activeTags.length === 0) {
      setExportError("書き出すフォルダ・タグ・項目を1つ以上選んでください");
      return;
    }
    if (!exportPath.trim()) {
      setExportError("書き出し先のファイルパスを指定してください");
      return;
    }
    if (!exportPass) {
      setExportError("パッケージに設定するパスワードを入力してください");
      return;
    }
    if (exportPass.length < 4) {
      setExportError("パスワードは4文字以上で設定してください");
      return;
    }
    if (exportPass !== exportConfirmPass) {
      setExportError("確認用パスワードが一致しません");
      return;
    }

    setExporting(true);
    try {
      const rep = await vaultApi.exportPackage({
        destPath: exportPath.trim(),
        passphrase: exportPass,
        overwrite: exportPickedViaSave,
        folderIds: activeFolders,
        includeSubfolders,
        tagNames: activeTags,
        entries: activeEntries,
      });
      setExportReport(rep);
      setExportPass("");
      setExportConfirmPass("");
      onToast("success", `パッケージを書き出しました (${formatBytes(rep.fileSizeBytes)})`);
    } catch (err) {
      setExportError(formatErrorMessage(err));
    } finally {
      setExporting(false);
    }
  };

  const handleBrowseImportSource = async () => {
    setImportError(null);
    const selected = await openDialog({
      title: "取り込むパッケージファイルを選択",
      multiple: false,
      directory: false,
      filters: [{ name: "Tuck パッケージ", extensions: ["tuckpack", "kvpack", "vault"] }],
    });
    if (typeof selected === "string" && selected.length > 0) {
      setImportPath(selected);
    }
  };

  const handleImportSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setImportError(null);
    setImportReport(null);

    if (!importPath.trim()) {
      setImportError("取り込むパッケージファイル (.tuckpack / .kvpack / .vault) を選択してください");
      return;
    }
    if (!importPass) {
      setImportError("パッケージのパスワードを入力してください");
      return;
    }

    setImporting(true);
    try {
      const rep = await vaultApi.importPackage({
        sourcePath: importPath.trim(),
        passphrase: importPass,
        targetFolderId: targetFolderId || null,
      });
      setImportReport(rep);
      setImportPass("");
      await onDataChanged(rep.avifPendingIds, rep.videoPendingIds);
      await loadHealth();
      onToast(
        "success",
        `パッケージを取り込みました（単体画像 ${rep.importedItems}件・漫画 ${rep.importedGroups}冊を追加）`
      );
    } catch (err) {
      setImportError(formatErrorMessage(err));
    } finally {
      setImporting(false);
    }
  };

  return (
    <dialog
      ref={dialogRef}
      className="maintenance-dialog"
      aria-labelledby="maintenance-dialog-title"
      onClose={(e) => {
        e.currentTarget.returnValue = "";
        thumbCancelRef.current = true;
        onClose();
      }}
    >
      <div className="maintenance-header">
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <Archive size={16} style={{ color: "var(--accent-blue)" }} />
          <h2 id="maintenance-dialog-title">保管庫の管理・パッケージ</h2>
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

      <div className="maintenance-tabs" role="tablist" aria-label="管理・パッケージタブ">
        <button
          type="button"
          role="tab"
          aria-selected={tab === "maintenance"}
          className={`gate-tab ${tab === "maintenance" ? "active" : ""}`}
          onClick={() => setTab("maintenance")}
        >
          管理・バックアップ
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "export"}
          className={`gate-tab ${tab === "export" ? "active" : ""}`}
          onClick={() => setTab("export")}
        >
          パッケージを書き出す
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "import"}
          className={`gate-tab ${tab === "import" ? "active" : ""}`}
          onClick={() => setTab("import")}
        >
          パッケージを取り込む
        </button>
      </div>

      <div className="maintenance-body">
        {tab === "maintenance" && (
          <>
            <section className="maintenance-card">
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
                <h3>保管庫の状態・整合性チェック</h3>
                <button
                  type="button"
                  className="btn"
                  style={{ padding: "4px 10px" }}
                  disabled={healthLoading}
                  onClick={() => void loadHealth()}
                >
                  <RefreshCw size={12} />
                  再確認
                </button>
              </div>

              {health ? (
                <div className="health-stats-grid">
                  <div className="health-stat-box">
                    <span className="health-stat-label">現在のファイルサイズ</span>
                    <span className="health-stat-value">{formatBytes(health.fileSizeBytes)}</span>
                  </div>
                  <div className="health-stat-box">
                    <span className="health-stat-label">回収可能な空き領域</span>
                    <span
                      className="health-stat-value"
                      style={{
                        color: health.reclaimableBytes > 0 ? "var(--warning)" : "var(--text-primary)",
                      }}
                    >
                      {formatBytes(health.reclaimableBytes)}
                    </span>
                  </div>
                  <div className="health-stat-box">
                    <span className="health-stat-label">データ整合性チェック</span>
                    <span
                      className="health-stat-value"
                      style={{
                        color: health.integrityOk ? "var(--success)" : "var(--danger)",
                        display: "inline-flex",
                        alignItems: "center",
                        gap: 5,
                      }}
                    >
                      <CheckCircle2 size={14} />
                      {health.integrityOk ? "正常" : health.integrityDetail}
                    </span>
                  </div>
                  <div className="health-stat-box">
                    <span className="health-stat-label">データ保護</span>
                    <span className="health-stat-value">
                      {health.journalMode.toLowerCase() === "delete"
                        ? "単一ファイル（安全書き込み）"
                        : health.journalMode.toUpperCase()}
                    </span>
                  </div>
                </div>
              ) : (
                <p>{healthLoading ? "診断中..." : "状態を取得できませんでした"}</p>
              )}
            </section>

            <section className="maintenance-card">
              <h3>サムネイルの再生成</h3>
              <p>
                動画や画像のサムネイルを作り直します。砂嵐・真っ暗になった動画も、有効なフレームを探索して更新します。
              </p>

              {thumbStats && (
                <div className={`url-status-badge ${thumbStats.ffmpegAvailable ? "ok" : ""}`}>
                  {thumbStats.ffmpegAvailable ? (
                    <CheckCircle2 size={13} style={{ flexShrink: 0 }} />
                  ) : (
                    <AlertCircle size={13} style={{ flexShrink: 0 }} />
                  )}
                  <span>
                    {thumbStats.ffmpegAvailable
                      ? "ffmpeg 検出済み（動画フレームを高速・高精度に抽出します）"
                      : "ffmpeg 未検出（ブラウザ抽出を使用します。設定画面で ffmpeg を指定するとより安定します）"}
                  </span>
                </div>
              )}

              <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
                <button
                  type="button"
                  className={`pill-btn ${thumbScope === "missingOnly" ? "active" : ""}`}
                  disabled={thumbRegening}
                  onClick={() => {
                    thumbScopeTouchedRef.current = true;
                    setThumbScope("missingOnly");
                  }}
                >
                  欠落のみ生成 ({thumbStats?.missingCount ?? 0})
                </button>
                <button
                  type="button"
                  className={`pill-btn ${thumbScope === "videosOnly" ? "active" : ""}`}
                  disabled={thumbRegening}
                  onClick={() => {
                    thumbScopeTouchedRef.current = true;
                    setThumbScope("videosOnly");
                  }}
                >
                  動画のみ再生成 ({thumbStats?.videoCount ?? 0})
                </button>
                <button
                  type="button"
                  className={`pill-btn ${thumbScope === "all" ? "active" : ""}`}
                  disabled={thumbRegening}
                  onClick={() => {
                    thumbScopeTouchedRef.current = true;
                    setThumbScope("all");
                  }}
                >
                  すべて再生成 ({thumbStats?.totalItems ?? 0})
                </button>
              </div>

              {thumbRegening && thumbProgress && (
                <div className="url-progress-box">
                  <div className="url-progress-head">
                    <span className="tabular-nums">
                      処理中: {thumbProgress.done} / {thumbProgress.total} 件
                    </span>
                    <button
                      type="button"
                      className="btn"
                      style={{ padding: "3px 8px", fontSize: 11 }}
                      onClick={() => {
                        thumbCancelRef.current = true;
                      }}
                    >
                      キャンセル
                    </button>
                  </div>
                  <div className="url-progress-track">
                    <div
                      className="url-progress-fill"
                      style={{
                        inlineSize: `${Math.round(
                          (thumbProgress.done / Math.max(1, thumbProgress.total)) * 100
                        )}%`,
                      }}
                    />
                  </div>
                  <div className="url-progress-meta tabular-nums">
                    <span>成功 {thumbProgress.succeeded} 件</span>
                    <span>失敗 {thumbProgress.failed} 件</span>
                  </div>
                </div>
              )}

              <div>
                <button
                  type="button"
                  className="btn btn-primary"
                  disabled={thumbRegening || optimizing || backingUp}
                  onClick={() => void handleRegenThumbnails()}
                >
                  <RefreshCw size={14} className={thumbRegening ? "spin" : ""} />
                  {thumbRegening ? "再生成中..." : "サムネイルを再生成"}
                </button>
              </div>

              {thumbReport && (
                <div className="success-banner">
                  {thumbReport.total === 0
                    ? "対象となるアイテムはありませんでした。"
                    : thumbReport.cancelled
                      ? `再生成を中断しました：成功 ${thumbReport.succeeded}件 / 失敗 ${thumbReport.failed}件（計 ${thumbReport.total}件中）`
                      : `再生成完了：成功 ${thumbReport.succeeded}件${
                          thumbReport.failed > 0 ? ` / 失敗 ${thumbReport.failed}件` : ""
                        }`}
                </div>
              )}
            </section>

            <section className="maintenance-card">
              <h3>容量の最適化（空き領域の圧縮）</h3>
              <p>
                画像を完全に削除した後に残った不要な空き領域を整理し、保管庫ファイルを最小サイズへ再構成します。一時処理はすべてメモリ上で行われ、平文がディスクへ漏れることはありません。
              </p>
              <div>
                <button
                  type="button"
                  className="btn btn-primary"
                  disabled={optimizing || healthLoading}
                  onClick={() => void handleOptimize()}
                >
                  <Sparkles size={14} />
                  {optimizing ? "最適化中..." : "保管庫を最適化"}
                </button>
              </div>
              {optimizeReport && (
                <div className="success-banner">
                  最適化完了：{formatBytes(optimizeReport.sizeBeforeBytes)} →{" "}
                  <strong>{formatBytes(optimizeReport.sizeAfterBytes)}</strong>（
                  {formatBytes(optimizeReport.reclaimedBytes)} 削減・整合性{" "}
                  {optimizeReport.integrityOk ? "正常" : "要確認"}）
                </div>
              )}
            </section>

            <section className="maintenance-card">
              <h3>バックアップの作成</h3>
              <p>
                整合性チェックを通過した暗号化保管庫の完全なコピーを、USBメモリや別ドライブへ安全に保存します。
              </p>
              <div>
                <button
                  type="button"
                  className="btn"
                  disabled={backingUp}
                  onClick={() => void handleBackup()}
                >
                  <HardDrive size={14} />
                  {backingUp ? "保存中..." : "バックアップを保存..."}
                </button>
              </div>
              {backupReport && (
                <div className="success-banner">
                  バックアップを保存しました（{formatBytes(backupReport.bytesWritten)}）
                </div>
              )}
            </section>

            {maintError && <div className="error-banner">{maintError}</div>}
          </>
        )}

        {tab === "export" && (
          <form onSubmit={handleExportSubmit} style={{ display: "flex", flexDirection: "column", gap: 14 }}>
            <section className="maintenance-card">
              <h3>1. 書き出す対象を選択</h3>
              <p>
                選択したフォルダ・タグ・項目に含まれる単体画像や漫画を、タグ情報・評価・メモごと独立した暗号化パッケージファイル（<code>.tuckpack</code>）へ書き出します。
              </p>

              {preselectedEntries.length > 0 && (
                <label className="checklist-row" style={{ background: "var(--accent-blue-soft)" }}>
                  <input
                    type="checkbox"
                    checked={includePreselectedEntries}
                    onChange={(e) => setIncludePreselectedEntries(e.target.checked)}
                  />
                  <span>選択中の項目を含める（{preselectedEntries.length}件）</span>
                </label>
              )}

              <div className="form-field">
                <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
                  <label>フォルダ</label>
                  <label style={{ display: "inline-flex", alignItems: "center", gap: 5, fontSize: 11, cursor: "pointer" }}>
                    <input
                      type="checkbox"
                      checked={includeSubfolders}
                      onChange={(e) => setIncludeSubfolders(e.target.checked)}
                    />
                    子フォルダも含める
                  </label>
                </div>
                {folderDepths.length === 0 ? (
                  <div style={{ fontSize: 12, color: "var(--text-muted)" }}>フォルダがありません</div>
                ) : (
                  <div className="checklist-box">
                    {folderDepths.map(({ folder, depth }) => {
                      const checked = exportFolderIds.has(folder.id);
                      return (
                        <label
                          key={folder.id}
                          className="checklist-row"
                          style={{ paddingInlineStart: 6 + depth * 14 }}
                        >
                          <input
                            type="checkbox"
                            checked={checked}
                            onChange={() => {
                              setExportFolderIds((prev) => {
                                const next = new Set(prev);
                                if (next.has(folder.id)) next.delete(folder.id);
                                else next.add(folder.id);
                                return next;
                              });
                            }}
                          />
                          <Folder size={13} style={{ color: folder.color || "var(--text-secondary)" }} />
                          <span style={{ flex: 1 }}>{folder.name}</span>
                          <span className="nav-item-count tabular-nums">{folder.itemCount}</span>
                        </label>
                      );
                    })}
                  </div>
                )}
              </div>

              <div className="form-field">
                <label>タグ</label>
                {tags.length === 0 ? (
                  <div style={{ fontSize: 12, color: "var(--text-muted)" }}>タグがありません</div>
                ) : (
                  <div className="tag-chips-wrap" style={{ maxBlockSize: 110, overflowY: "auto" }}>
                    {tags.map((t) => {
                      const active = exportTagNames.has(t.name);
                      return (
                        <button
                          key={t.id}
                          type="button"
                          className={`pill-btn ${active ? "active" : ""}`}
                          onClick={() => {
                            setExportTagNames((prev) => {
                              const next = new Set(prev);
                              if (next.has(t.name)) next.delete(t.name);
                              else next.add(t.name);
                              return next;
                            });
                          }}
                        >
                          #{t.name} ({t.count})
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>
            </section>

            <section className="maintenance-card">
              <h3>2. 保存先とパスワード</h3>
              <div className="form-field">
                <label htmlFor="pkg-export-path">保存先ファイル (.tuckpack)</label>
                <div className="path-picker-row">
                  <input
                    id="pkg-export-path"
                    type="text"
                    className="inspector-input"
                    placeholder="保存先を選択してください"
                    value={exportPath}
                    onChange={(e) => {
                      setExportPath(e.target.value);
                      setExportPickedViaSave(false);
                    }}
                  />
                  <button type="button" className="btn" onClick={() => void handleBrowseExportDest()}>
                    <FolderOpen size={14} />
                    選択
                  </button>
                </div>
              </div>

              <div className="form-field">
                <label htmlFor="pkg-export-pass">パッケージに設定するパスワード</label>
                <div className="input-with-icon">
                  <input
                    id="pkg-export-pass"
                    type={showExportPass ? "text" : "password"}
                    className="inspector-input"
                    autoComplete="off"
                    placeholder="共有・保管用の任意のパスワード（4文字以上）"
                    value={exportPass}
                    onChange={(e) => setExportPass(e.target.value)}
                  />
                  <button
                    type="button"
                    className="input-trailing-btn"
                    aria-label={showExportPass ? "パスワードを隠す" : "パスワードを表示"}
                    onClick={() => setShowExportPass((v) => !v)}
                  >
                    {showExportPass ? <EyeOff size={14} /> : <Eye size={14} />}
                  </button>
                </div>
              </div>

              <div className="form-field">
                <label htmlFor="pkg-export-confirm">パスワード（確認）</label>
                <input
                  id="pkg-export-confirm"
                  type={showExportPass ? "text" : "password"}
                  className="inspector-input"
                  autoComplete="off"
                  placeholder="同じパスワードを再入力"
                  value={exportConfirmPass}
                  onChange={(e) => setExportConfirmPass(e.target.value)}
                />
              </div>
            </section>

            {exportError && <div className="error-banner">{exportError}</div>}

            {exportReport && (
              <div className="success-banner">
                書き出し完了：単体画像 {exportReport.itemCount}件 / 漫画 {exportReport.groupCount}冊
                （{exportReport.pageCount}P）/ フォルダ {exportReport.folderCount}件 / タグ{" "}
                {exportReport.tagCount}種類（計 {formatBytes(exportReport.fileSizeBytes)}）
              </div>
            )}

            <button type="submit" className="btn btn-primary" disabled={exporting}>
              <Upload size={14} />
              {exporting ? "暗号化パッケージを作成中..." : "暗号化パッケージを書き出す"}
            </button>
          </form>
        )}

        {tab === "import" && (
          <form onSubmit={handleImportSubmit} style={{ display: "flex", flexDirection: "column", gap: 14 }}>
            <section className="maintenance-card">
              <h3>パッケージを取り込んで統合</h3>
              <p>
                別の保管庫から書き出した暗号化パッケージ（<code>.tuckpack</code> / <code>.kvpack</code> / <code>.vault</code>）を復号し、現在の保管庫へ統合します。重複画像は自動で検出し、タグ情報のみ統合されます。
              </p>

              <div className="form-field">
                <label htmlFor="pkg-import-path">パッケージファイル</label>
                <div className="path-picker-row">
                  <input
                    id="pkg-import-path"
                    type="text"
                    className="inspector-input"
                    placeholder=".tuckpack / .kvpack / .vault を選択"
                    value={importPath}
                    onChange={(e) => setImportPath(e.target.value)}
                  />
                  <button type="button" className="btn" onClick={() => void handleBrowseImportSource()}>
                    <FolderOpen size={14} />
                    選択
                  </button>
                </div>
              </div>

              <div className="form-field">
                <label htmlFor="pkg-import-pass">パッケージのパスワード</label>
                <div className="input-with-icon">
                  <input
                    id="pkg-import-pass"
                    type={showImportPass ? "text" : "password"}
                    className="inspector-input"
                    autoComplete="off"
                    placeholder="書き出し時に設定したパスワード"
                    value={importPass}
                    onChange={(e) => setImportPass(e.target.value)}
                  />
                  <button
                    type="button"
                    className="input-trailing-btn"
                    aria-label={showImportPass ? "パスワードを隠す" : "パスワードを表示"}
                    onClick={() => setShowImportPass((v) => !v)}
                  >
                    {showImportPass ? <EyeOff size={14} /> : <Eye size={14} />}
                  </button>
                </div>
              </div>

              <div className="form-field">
                <label htmlFor="pkg-import-target">取り込み先フォルダ（任意）</label>
                <select
                  id="pkg-import-target"
                  className="toolbar-select"
                  value={targetFolderId}
                  onChange={(e) => setTargetFolderId(e.target.value)}
                >
                  <option value="">ルート（パッケージ内のフォルダ階層をそのまま維持）</option>
                  {folderDepths.map(({ folder, depth }) => (
                    <option key={folder.id} value={folder.id}>
                      {"　".repeat(depth)}
                      {folder.name}
                    </option>
                  ))}
                </select>
              </div>
            </section>

            {importError && <div className="error-banner">{importError}</div>}

            {importReport && (
              <div className="success-banner">
                <div style={{ display: "inline-flex", alignItems: "center", gap: 6, fontWeight: 600 }}>
                  <ShieldCheck size={14} />
                  統合完了
                </div>
                <div>
                  ・新規取り込み（単体画像）：{importReport.importedItems}件（重複統合：{importReport.mergedDuplicateItems}件）
                </div>
                <div>
                  ・新規取り込み（漫画グループ）：{importReport.importedGroups}冊（重複スキップ：{importReport.skippedDuplicateGroups}冊）
                </div>
                <div>
                  ・作成したフォルダ：{importReport.importedFolders}件 / 追加したタグ付け：{importReport.mergedTagLinks}件
                </div>
              </div>
            )}

            <button type="submit" className="btn btn-primary" disabled={importing}>
              <Download size={14} />
              {importing ? "復号・統合中..." : "パッケージを取り込んで統合"}
            </button>
          </form>
        )}
      </div>
    </dialog>
  );
}
