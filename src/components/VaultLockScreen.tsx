import { useState, type FormEvent } from "react";
import { Eye, EyeOff, KeyRound, Lock, LogOut } from "lucide-react";
import { formatErrorMessage, vaultApi } from "../services/vaultApi";
import type { VaultStatus } from "../types/vault";

interface VaultLockScreenProps {
  onUnlocked: (status: VaultStatus) => void;
  onSwitchVault: () => void;
}

export function VaultLockScreen({ onUnlocked, onSwitchVault }: VaultLockScreenProps) {
  const [passphrase, setPassphrase] = useState("");
  const [showPass, setShowPass] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!passphrase) {
      setError("パスワードを入力してください");
      return;
    }
    setError(null);
    setBusy(true);
    try {
      const nextStatus = await vaultApi.unlockVault(passphrase);
      setPassphrase("");
      onUnlocked(nextStatus);
    } catch (err) {
      setError(formatErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="gate-shell">
      <section className="gate-card" aria-label="ロック解除">
        <header className="gate-header">
          <div className="gate-logo" aria-hidden="true">
            <Lock size={20} />
          </div>
          <div>
            <h1>ロック中</h1>
            <p>閲覧を再開するにはパスワードを入力してください</p>
          </div>
        </header>

        <form className="gate-form" onSubmit={handleSubmit}>
          <div className="form-field">
            <label htmlFor="unlock-pass-input">パスワード</label>
            <div className="input-with-icon">
              <input
                id="unlock-pass-input"
                type={showPass ? "text" : "password"}
                className="inspector-input"
                autoComplete="off"
                placeholder="パスワードを入力"
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

          {error && (
            <div className="error-banner" role="alert">
              {error}
            </div>
          )}

          <button type="submit" className="btn btn-primary" disabled={busy}>
            <KeyRound size={14} />
            {busy ? "復号中..." : "ロックを解除"}
          </button>

          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={onSwitchVault}
          >
            <LogOut size={14} />
            別の保管庫を開く
          </button>
        </form>
      </section>
    </main>
  );
}
