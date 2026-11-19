import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";
import type { ToastItem } from "../types/vault";

interface ToastRegionProps {
  toasts: ToastItem[];
  onDismiss: (id: number) => void;
}

export function ToastRegion({ toasts, onDismiss }: ToastRegionProps) {
  if (toasts.length === 0) return null;

  return (
    <div className="toast-region" aria-live="polite" aria-label="通知">
      {toasts.map((toast) => (
        <div key={toast.id} className={`toast-item ${toast.tone}`}>
          {toast.tone === "success" && (
            <CheckCircle2 size={15} style={{ color: "var(--success)", flexShrink: 0 }} />
          )}
          {toast.tone === "warning" && (
            <AlertCircle size={15} style={{ color: "var(--warning)", flexShrink: 0 }} />
          )}
          {toast.tone === "error" && (
            <AlertCircle size={15} style={{ color: "var(--danger)", flexShrink: 0 }} />
          )}
          {toast.tone === "info" && (
            <Info size={15} style={{ color: "var(--accent-blue)", flexShrink: 0 }} />
          )}
          <span style={{ flex: 1 }}>{toast.message}</span>
          <button
            type="button"
            className="tag-chip-remove"
            aria-label="通知を閉じる"
            onClick={() => onDismiss(toast.id)}
          >
            <X size={13} />
          </button>
        </div>
      ))}
    </div>
  );
}
