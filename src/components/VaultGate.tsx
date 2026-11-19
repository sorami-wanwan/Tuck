import { useState, type FormEvent } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Eye, EyeOff, FolderOpen, Lock, ShieldCheck } from "lucide-react";
import { formatErrorMessage, vaultApi } from "../services/vaultApi";
import type { VaultStatus } from "../types/vault";

interface VaultGateProps {
  onOpened: (status: VaultStatus) => void;
}

export function VaultGate({ onOpened }: VaultGateProps) {
  const [mode, setMode] = useState<"open" | "create">("open");
  const [vaultPath, setVaultPath] = useState("");
  const [pickedViaSaveDialog, setPickedViaSaveDialog] = useState(false);
  const [passphrase, setPassphrase] = useState("");
  const [confirmPassphrase, setConfirmPassphrase] = useState("");
  const [showPass, setShowPass] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const switchMode = (nextMode: "open" | "create") => {
    setMode(nextMode);
    setError(null);
    setPassphrase("");
    setConfirmPassphrase("");
    setPickedViaSaveDialog(false);
  };

  const handleBrowse = async () => {
    setError(null);
    try {
      if (mode === "open") {
        const selected = await open({
          title: "保管庫ファイルを開く",
          multiple: false,
          directory: false,
          filters: [{ name: "Tuck", extensions: ["vault", "tuckpack", "kvpack"] }],
        });
        if (typeof selected === "string" && selected.length > 0) {
          setVaultPath(selected);
          setPickedViaSaveDialog(false);
        }
      } else {
        const selected = await save({
          title: "新しい保管庫ファイルを作成",
          defaultPath: "library.vault",
          filters: [{ name: "Tuck", extensions: ["vault"] }],
        });
        if (selected && selected.length > 0) {
          const normalized = selected.endsWith(".vault") ? selected : `${selected}.vault`;
          setVaultPath(normalized);
          setPickedViaSaveDialog(true);
        }
      }
    } catch (err) {
      setError(formatErrorMessage(err));
    }
  };

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);

    const trimmedPath = vaultPath.trim();
    if (!trimmedPath) {
      setError("保管庫ファイル (.vault) を指定してください");
      return;
    }
    if (!passphrase) {
      setError("パスワードを入力してください");
      return;
    }
    if (mode === "create") {
      if (passphrase.length < 4) {
        setError("パスワードは4文字以上で設定してください");
        return;
      }
      if (passphrase !== confirmPassphrase) {
        setError("確認用パスワードが一致しません");
        return;
      }
    }

    setBusy(true);
    try {
      const status =
        mode === "create"
          ? await vaultApi.createVault(trimmedPath, passphrase, pickedViaSaveDialog)
          : await vaultApi.openVault(trimmedPath, passphrase);
      setPassphrase("");
      setConfirmPassphrase("");
      onOpened(status);
    } catch (err) {
      setError(formatErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="gate-shell">
      <section className="gate-card" aria-label="保管庫のロック解除または作成">
        <header className="gate-header">
          <div className="gate-logo" aria-hidden="true">
            <ShieldCheck size={22} />
          </div>
          <h1>Tuck</h1>
        </header>

        <div className="gate-tabs" role="tablist" aria-label="操作モード">
          <button
            type="button"
            role="tab"
            aria-selected={mode === "open"}
            className={`gate-tab ${mode === "open" ? "active" : ""}`}
            onClick={() => switchMode("open")}
          >
            既存の保管庫を開く
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={mode === "create"}
            className={`gate-tab ${mode === "create" ? "active" : ""}`}
            onClick={() => switchMode("create")}
          >
            新しい保管庫を作成
          </button>
        </div>

        <form className="gate-form" onSubmit={handleSubmit}>
          <div className="form-field">
            <label htmlFor="vault-path-input">保管庫ファイル (.vault)</label>
            <div className="path-picker-row">
              <input
                id="vault-path-input"
                type="text"
                className="inspector-input"
                placeholder={
                  mode === "create"
                    ? "保存先を選択してください（例: library.vault）"
                    : "ファイルを選択してください"
                }
                value={vaultPath}
                onChange={(e) => {
                  setVaultPath(e.target.value);
                  setPickedViaSaveDialog(false);
                }}
              />
              <button type="button" className="btn" onClick={handleBrowse} disabled={busy}>
                <FolderOpen size={14} />
                選択
              </button>
            </div>
          </div>

          <div className="form-field">
            <label htmlFor="vault-pass-input">パスワード</label>
            <div className="input-with-icon">
              <input
                id="vault-pass-input"
                type={showPass ? "text" : "password"}
                className="inspector-input"
                autoComplete="off"
                placeholder={mode === "create" ? "パスワードを設定（4文字以上）" : "パスワードを入力"}
                value={passphrase}
                onChange={(e) => setPassphrase(e.target.value)}
                autoFocus
              />
              <button
                type="button"
                className="input-trailing-btn"
                aria-label={showPass ? "パスワードを隠す" : "パスワードを表示"}
                onClick={() => setShowPass((v) => !v)}
              >
                {showPass ? <EyeOff size={14} /> : <Eye size={14} />}
              </button>
            </div>
          </div>

          {mode === "create" && (
            <div className="form-field">
              <label htmlFor="vault-confirm-input">パスワード（確認）</label>
              <input
                id="vault-confirm-input"
                type={showPass ? "text" : "password"}
                className="inspector-input"
                autoComplete="off"
                placeholder="同じパスワードを再入力"
                value={confirmPassphrase}
                onChange={(e) => setConfirmPassphrase(e.target.value)}
              />
            </div>
          )}

          {error && (
            <div className="error-banner" role="alert">
              {error}
            </div>
          )}

          <button type="submit" className="btn btn-primary" disabled={busy}>
            <Lock size={14} />
            {busy
              ? mode === "create"
                ? "保管庫を作成中..."
                : "保管庫を開いています..."
              : mode === "create"
                ? "暗号化保管庫を作成して開く"
                : "保管庫を開く"}
          </button>
        </form>
      </section>
    </main>
  );
}
