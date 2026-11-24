import { useEffect, useRef } from "react";
import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";
import type { ToastItem } from "../types/vault";

interface ToastRegionProps {
  toasts: ToastItem[];
  onDismiss: (id: number) => void;
}

type PopoverElement = HTMLDivElement & {
  showPopover?: () => void;
  hidePopover?: () => void;
};

export function ToastRegion({ toasts, onDismiss }: ToastRegionProps) {
  const regionRef = useRef<HTMLDivElement>(null);
  const latestIdRef = useRef<number | null>(null);
  const latestId = toasts.length > 0 ? toasts[toasts.length - 1].id : null;

  useEffect(() => {
    const el = regionRef.current as PopoverElement | null;
    if (!el || typeof el.showPopover !== "function" || typeof el.hidePopover !== "function") {
      latestIdRef.current = latestId;
      return;
    }

    try {
      const isOpen = el.matches(":popover-open");
      if (toasts.length === 0) {
        if (isOpen) el.hidePopover();
      } else {
        if (isOpen && latestId !== latestIdRef.current) {
          el.hidePopover();
        }
        if (!el.matches(":popover-open")) {
          el.showPopover();
        }
      }
    } catch {
      // Ignore Popover state errors in non-supporting environments.
    }
    latestIdRef.current = latestId;
  }, [toasts.length, latestId]);

  return (
    <div
      ref={regionRef}
      popover="manual"
      className="toast-region"
      role="region"
      aria-live="polite"
      aria-label="通知"
    >
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
