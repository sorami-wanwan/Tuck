import { useState, type FormEvent } from "react";
import { KeyRound, Loader2, Lock, RefreshCw, ShieldCheck } from "lucide-react";

interface LanAuthScreenProps {
  phase: "checking" | "authenticating" | "pin" | "ended";
  endReason?: string;
  error: string | null;
  onSubmitPin: (pin: string) => Promise<void>;
  onRetryConnection: () => void;
}

export function LanAuthScreen({
  phase,
  endReason,
  error,
  onSubmitPin,
  onRetryConnection,
}: LanAuthScreenProps) {
  const [pin, setPin] = useState("");
  const [busy, setBusy] = useState(false);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    const cleaned = pin.replace(/\D/g, "").slice(0, 6);
    if (cleaned.length !== 6) return;
    setBusy(true);
    try {
      await onSubmitPin(cleaned);
      setPin("");
    } finally {
      setBusy(false);
    }
  };

  if (phase === "checking" || phase === "authenticating") {
    return (
      <main className="gate-shell">
        <section className="gate-card" aria-label="接続中">
          <header className="gate-header">
            <div className="gate-logo" aria-hidden="true">
              <Loader2 size={20} className="spin" />
            </div>
            <div>
              <h1>{phase === "authenticating" ? "QR 認証を確認中..." : "Tuck に接続中..."}</h1>
              <p>ホスト端末とセッションを確立しています</p>
            </div>
          </header>
        </section>
      </main>
    );
  }

  if (phase === "ended") {
    return (
      <main className="gate-shell">
        <section className="gate-card" aria-label="公開終了">
          <header className="gate-header">
            <div className="gate-logo" aria-hidden="true">
              <Lock size={20} />
            </div>
            <div>
              <h1>LAN 公開が終了しました</h1>
              <p>
                {endReason ||
                  "ホスト側で公開が停止されたか、保管庫がロックされたため接続を終了しました。メモリ上の画像は消去されています。"}
              </p>
            </div>
          </header>

          {error && (
            <div className="error-banner" role="alert" style={{ marginTop: 12 }}>
              {error}
            </div>
          )}

          <div style={{ marginTop: 16, display: "flex", flexDirection: "column", gap: 8 }}>
            <button type="button" className="btn btn-primary" onClick={onRetryConnection}>
              <RefreshCw size={14} />
              再接続を試す
            </button>
          </div>
        </section>
      </main>
    );
  }

  return (
    <main className="gate-shell">
      <section className="gate-card" aria-label="LAN アクセス認証">
        <header className="gate-header">
          <div className="gate-logo" aria-hidden="true">
            <ShieldCheck size={20} />
          </div>
          <div>
            <h1>Tuck (LAN 閲覧)</h1>
            <p>ホスト端末の画面に表示されている 6 桁の PIN を入力してください</p>
          </div>
        </header>

        <form className="gate-form" onSubmit={handleSubmit}>
          <div className="form-field">
            <label htmlFor="lan-pin-input">6 桁 PIN</label>
            <input
              id="lan-pin-input"
              type="text"
              inputMode="numeric"
              pattern="[0-9]*"
              autoComplete="one-time-code"
              maxLength={6}
              placeholder="000000"
              className="inspector-input lan-pin-input"
              value={pin}
              onChange={(e) => setPin(e.target.value.replace(/\D/g, "").slice(0, 6))}
              autoFocus
            />
          </div>

          {error && (
            <div className="error-banner" role="alert">
              {error}
            </div>
          )}

          <button
            type="submit"
            className="btn btn-primary"
            disabled={busy || pin.length !== 6}
          >
            <KeyRound size={14} />
            {busy ? "確認中..." : "接続する"}
          </button>
        </form>
      </section>
    </main>
  );
}
