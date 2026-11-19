import { useCallback, useRef, useState } from "react";
import type { ToastItem } from "../types/vault";

/**
 * トースト通知のキュー追加と自動消去・手動閉じるを管理するフック。
 */
export function useToasts() {
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const toastIdRef = useRef(1);

  const pushToast = useCallback((tone: ToastItem["tone"], message: string) => {
    const id = toastIdRef.current++;
    setToasts((prev) => [...prev.slice(-3), { id, tone, message }]);
    window.setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 4200);
  }, []);

  const dismissToast = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  return { toasts, pushToast, dismissToast };
}
