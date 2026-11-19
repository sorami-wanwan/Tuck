import { useCallback, useEffect, useRef, useState, type MutableRefObject, type SetStateAction } from "react";
import { imageMemoryCache } from "../services/imageCache";
import {
  connectLanEventStream,
  disconnectLanEventStream,
  isTauriRuntime,
  subscribeVaultEvent,
} from "../services/runtime";
import { formatErrorMessage, setAuthInvalidatedHandler, vaultApi } from "../services/vaultApi";
import type {
  ClipboardImportEvent,
  LanAccessMode,
  LanShareStatus,
  ToastItem,
  VaultStatus,
} from "../types/vault";

export type LanAuthPhase = "checking" | "authenticating" | "pin" | "authenticated" | "ended";

const CLOSED_LAN_STATUS: LanShareStatus = {
  running: false,
  mode: "readOnly",
  url: "",
  qrUrl: "",
  qrSvg: "",
  pin: "",
  port: 0,
  selectedIp: "",
  interfaces: [],
  startedAtMs: null,
  expiresAtMs: null,
  pauseIdleLock: true,
  clients: [],
};

const CLOSED_STATUS: VaultStatus = {
  isOpen: false,
  canUnlock: false,
  itemCount: 0,
  uncategorizedCount: 0,
  untaggedCount: 0,
  trashCount: 0,
  totalBytes: 0,
  clipboardWatchEnabled: false,
  skipPlainExportConfirm: false,
  lockSettings: {
    shortcut: "CommandOrControl+Shift+L",
    idleTimeoutMinutes: 5,
  },
  urlImportSettings: {
    enabled: false,
    ytdlpPath: "",
    ffmpegPath: "",
    cookieBrowser: "",
    cookiesFile: "",
    maxVideoSizeMb: 2048,
    maxVideoHeight: 0,
  },
};

function matchesShortcut(e: KeyboardEvent, shortcut: string): boolean {
  if (!shortcut) return false;
  const parts = shortcut.split("+").map((p) => p.trim().toLowerCase());
  const mainKey = parts[parts.length - 1];
  const wantCmdCtrl =
    parts.includes("commandorcontrol") || parts.includes("ctrl") || parts.includes("cmd");
  const wantAlt = parts.includes("alt") || parts.includes("option");
  const wantShift = parts.includes("shift");

  if ((e.metaKey || e.ctrlKey) !== wantCmdCtrl) return false;
  if (e.altKey !== wantAlt) return false;
  if (e.shiftKey !== wantShift) return false;
  return e.key.toLowerCase() === mainKey;
}

interface UseVaultSessionOptions {
  pushToast: (tone: ToastItem["tone"], message: string) => void;
  refreshLibraryRef: MutableRefObject<() => Promise<void>>;
  onResetWorkspaceState: () => void;
  onReadOnlyEnforced?: () => void;
}

/**
 * 保管庫の開閉・即時ロック・無操作自動ロック・LAN クライアント認証およびバックエンドイベント購読を管理するフック。
 */
export function useVaultSession({
  pushToast,
  refreshLibraryRef,
  onResetWorkspaceState,
  onReadOnlyEnforced,
}: UseVaultSessionOptions) {
  const isWebClient = !isTauriRuntime();
  const [lanAuthPhase, setLanAuthPhase] = useState<LanAuthPhase>(
    isWebClient ? "checking" : "authenticated"
  );
  const [lanEndReason, setLanEndReason] = useState("");
  const [lanAuthError, setLanAuthError] = useState<string | null>(null);
  const [lanMode, setLanMode] = useState<LanAccessMode>("readOnly");
  const [lanShareStatus, setLanShareStatus] = useState<LanShareStatus>(CLOSED_LAN_STATUS);
  const [lanDialogOpen, setLanDialogOpen] = useState(false);

  const canEdit = !isWebClient || lanMode === "readWrite";

  const [status, setRawStatus] = useState<VaultStatus>(CLOSED_STATUS);

  const setStatus = useCallback(
    (nextOrUpdater: SetStateAction<VaultStatus>) => {
      if (typeof nextOrUpdater !== "function" && !isWebClient) {
        imageMemoryCache.setLocalMediaEndpoint(
          nextOrUpdater.localMediaBaseUrl ?? null,
          nextOrUpdater.localMediaToken ?? null
        );
      }
      setRawStatus((prev) => {
        const next = typeof nextOrUpdater === "function" ? nextOrUpdater(prev) : nextOrUpdater;
        if (!isWebClient) {
          imageMemoryCache.setLocalMediaEndpoint(
            next.localMediaBaseUrl ?? null,
            next.localMediaToken ?? null
          );
        }
        return next;
      });
    },
    [isWebClient]
  );

  if (!isWebClient) {
    if (status.isOpen) {
      imageMemoryCache.setLocalMediaEndpoint(
        status.localMediaBaseUrl ?? null,
        status.localMediaToken ?? null
      );
    } else {
      imageMemoryCache.setLocalMediaEndpoint(null, null);
    }
  }

  const libraryRefreshTimerRef = useRef<number | null>(null);
  const lastActivityRef = useRef(Date.now());
  const resetWorkspaceRef = useRef(onResetWorkspaceState);
  resetWorkspaceRef.current = onResetWorkspaceState;
  const readOnlyEnforcedRef = useRef(onReadOnlyEnforced);
  readOnlyEnforcedRef.current = onReadOnlyEnforced;

  const wipeFrontendSensitiveState = useCallback(() => {
    setLanDialogOpen(false);
    resetWorkspaceRef.current();
    imageMemoryCache.revokeAll();
  }, []);

  const performInstantLock = useCallback(
    async (callBackend: boolean) => {
      wipeFrontendSensitiveState();
      setLanShareStatus(CLOSED_LAN_STATUS);
      setStatus({ ...CLOSED_STATUS, canUnlock: true });
      if (callBackend) {
        try {
          await vaultApi.lockVault();
        } catch {
          // Best-effort lock
        }
      }
    },
    [wipeFrontendSensitiveState]
  );

  const handleCloseCompletely = useCallback(async () => {
    wipeFrontendSensitiveState();
    setLanShareStatus(CLOSED_LAN_STATUS);
    setStatus(CLOSED_STATUS);
    try {
      await vaultApi.closeVault();
    } catch {
      // Best-effort close
    }
  }, [wipeFrontendSensitiveState]);

  // Revoke object URLs on unload
  useEffect(() => {
    const onBeforeUnload = () => {
      imageMemoryCache.revokeAll();
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    return () => window.removeEventListener("beforeunload", onBeforeUnload);
  }, []);

  // Host: load LAN share status when vault opens
  useEffect(() => {
    if (isWebClient || !status.isOpen) return;
    void vaultApi
      .getLanShareStatus()
      .then(setLanShareStatus)
      .catch(() => {});
  }, [isWebClient, status.isOpen]);

  // Web client: startup auth check (?k= QR token or existing cookie) + auth invalidation hook
  const initWebClientAuth = useCallback(async () => {
    if (!isWebClient) return;
    setLanAuthError(null);
    const params = new URLSearchParams(window.location.search);
    const qrToken = params.get("k");
    if (qrToken) {
      window.history.replaceState({}, "", window.location.pathname);
      setLanAuthPhase("authenticating");
      try {
        const auth = await vaultApi.authenticateLanToken(qrToken);
        imageMemoryCache.setLanMediaToken(auth.mediaToken);
        setLanMode(auth.mode);
        connectLanEventStream();
        const st = await vaultApi.getVaultStatus();
        setStatus(st);
        setLanAuthPhase("authenticated");
        return;
      } catch (err) {
        setLanAuthError(formatErrorMessage(err));
        setLanAuthPhase("pin");
        return;
      }
    }

    setLanAuthPhase("checking");
    try {
      const auth = await vaultApi.getLanAuthStatus();
      if (auth.authenticated && auth.vaultOpen) {
        imageMemoryCache.setLanMediaToken(auth.mediaToken);
        setLanMode(auth.mode);
        connectLanEventStream();
        const st = await vaultApi.getVaultStatus();
        setStatus(st);
        setLanAuthPhase("authenticated");
      } else if (!auth.vaultOpen) {
        setLanEndReason("現在ホスト側で保管庫が開かれていないか、LAN 公開が停止しています。");
        setLanAuthPhase("ended");
      } else {
        setLanMode(auth.mode);
        setLanAuthPhase("pin");
      }
    } catch (err) {
      setLanAuthError(formatErrorMessage(err));
      setLanAuthPhase("ended");
    }
  }, [isWebClient]);

  useEffect(() => {
    if (!isWebClient) return;
    setAuthInvalidatedHandler((reason) => {
      disconnectLanEventStream();
      wipeFrontendSensitiveState();
      setStatus(CLOSED_STATUS);
      setLanEndReason(
        reason === "locked"
          ? "ホスト側で保管庫がロックされたか、LAN 公開が終了しました。"
          : "セッションの有効期限が切れたか、切断されました。"
      );
      setLanAuthPhase(reason === "locked" ? "ended" : "pin");
    });
    void initWebClientAuth();
    return () => {
      setAuthInvalidatedHandler(null);
      disconnectLanEventStream();
    };
  }, [isWebClient, initWebClientAuth, wipeFrontendSensitiveState]);

  const handleWebPinSubmit = useCallback(async (pin: string) => {
    setLanAuthError(null);
    try {
      const auth = await vaultApi.authenticateLanPin(pin);
      imageMemoryCache.setLanMediaToken(auth.mediaToken);
      setLanMode(auth.mode);
      connectLanEventStream();
      const st = await vaultApi.getVaultStatus();
      setStatus(st);
      setLanAuthPhase("authenticated");
    } catch (err) {
      setLanAuthError(formatErrorMessage(err));
    }
  }, []);

  const handleLogoutLan = useCallback(async () => {
    try {
      await vaultApi.logoutLanSession();
    } catch {
      // ignore
    }
    disconnectLanEventStream();
    wipeFrontendSensitiveState();
    setStatus(CLOSED_STATUS);
    setLanAuthError(null);
    setLanAuthPhase("pin");
  }, [wipeFrontendSensitiveState]);

  // Listen to backend Instant Lock, Clipboard Watcher, and LAN sync events
  useEffect(() => {
    const cleanups: Array<() => void> = [];
    let active = true;

    const addSub = async <T,>(name: string, fn: (payload: T) => void) => {
      const unsub = await subscribeVaultEvent<T>(name, fn);
      if (active) cleanups.push(unsub);
      else unsub();
    };

    void addSub("vault://locked", () => {
      if (!active) return;
      if (isWebClient) {
        disconnectLanEventStream();
        wipeFrontendSensitiveState();
        setStatus(CLOSED_STATUS);
        setLanEndReason("ホスト側で保管庫がロックされたため、LAN 公開を終了しました。");
        setLanAuthPhase("ended");
      } else {
        void performInstantLock(false);
      }
    });

    void addSub("vault://library-changed", () => {
      if (!active) return;
      if (libraryRefreshTimerRef.current !== null) {
        window.clearTimeout(libraryRefreshTimerRef.current);
      }
      libraryRefreshTimerRef.current = window.setTimeout(() => {
        libraryRefreshTimerRef.current = null;
        void refreshLibraryRef.current();
      }, 300);
    });

    if (isWebClient) {
      void addSub<{ reason?: string }>("vault://lan-stopped", (payload) => {
        if (!active) return;
        const reason = payload?.reason;
        const msg =
          reason === "expired"
            ? "自動停止タイマーの期限に達したため、LAN 公開を終了しました。"
            : reason === "credentials_regenerated" || reason === "disconnected"
              ? "ホスト側で接続がリセットされました。新しい PIN または QR コードで再接続してください。"
              : "ホスト側で LAN 公開が停止されました。";
        disconnectLanEventStream();
        wipeFrontendSensitiveState();
        setStatus(CLOSED_STATUS);
        setLanEndReason(msg);
        setLanAuthPhase("ended");
      });

      void addSub<{ mode?: LanAccessMode }>("vault://lan-mode-changed", (payload) => {
        if (!active || !payload?.mode) return;
        setLanMode(payload.mode);
        if (payload.mode === "readOnly") {
          readOnlyEnforcedRef.current?.();
          pushToast("info", "ホストにより「閲覧のみ」モードへ切り替えられました");
        } else {
          pushToast("success", "ホストにより「編集・取り込み可」モードへ切り替えられました");
        }
      });
    } else {
      void addSub<ClipboardImportEvent>("vault://clipboard-imported", (p) => {
        if (!active) return;
        if (p.importedId) {
          pushToast("success", "クリップボードの画像を自動で取り込みました");
          void refreshLibraryRef.current();
        } else if (p.duplicate) {
          pushToast("info", "クリップボードの画像はすでに登録済みです（重複スキップ）");
        }
      });

      void addSub("vault://lan-status-changed", () => {
        if (!active) return;
        void vaultApi
          .getLanShareStatus()
          .then(setLanShareStatus)
          .catch(() => {});
      });
    }

    return () => {
      active = false;
      if (libraryRefreshTimerRef.current !== null) {
        window.clearTimeout(libraryRefreshTimerRef.current);
        libraryRefreshTimerRef.current = null;
      }
      for (const unsub of cleanups) unsub();
    };
  }, [isWebClient, performInstantLock, wipeFrontendSensitiveState, pushToast, refreshLibraryRef]);

  // Idle auto-lock & in-app Instant Lock shortcut (Host Only)
  useEffect(() => {
    if (!status.isOpen || isWebClient) return;
    lastActivityRef.current = Date.now();

    const touch = () => {
      lastActivityRef.current = Date.now();
    };

    const onKeyDown = (e: KeyboardEvent) => {
      touch();
      if (matchesShortcut(e, status.lockSettings.shortcut)) {
        e.preventDefault();
        void performInstantLock(true);
      }
    };

    window.addEventListener("pointerdown", touch);
    window.addEventListener("pointermove", touch);
    window.addEventListener("wheel", touch, { passive: true });
    window.addEventListener("keydown", onKeyDown, true);

    const mins = status.lockSettings.idleTimeoutMinutes;
    const pauseIdle = lanShareStatus.running && lanShareStatus.pauseIdleLock;
    const timer =
      mins > 0 && !pauseIdle
        ? window.setInterval(() => {
            if (Date.now() - lastActivityRef.current >= mins * 60_000) {
              void performInstantLock(true);
            }
          }, 10_000)
        : null;

    return () => {
      window.removeEventListener("pointerdown", touch);
      window.removeEventListener("pointermove", touch);
      window.removeEventListener("wheel", touch);
      window.removeEventListener("keydown", onKeyDown, true);
      if (timer !== null) window.clearInterval(timer);
    };
  }, [
    status.isOpen,
    status.lockSettings,
    isWebClient,
    lanShareStatus.running,
    lanShareStatus.pauseIdleLock,
    performInstantLock,
  ]);

  return {
    isWebClient,
    lanAuthPhase,
    lanEndReason,
    lanAuthError,
    lanMode,
    lanShareStatus,
    setLanShareStatus,
    lanDialogOpen,
    setLanDialogOpen,
    canEdit,
    status,
    setStatus,
    performInstantLock,
    handleCloseCompletely,
    initWebClientAuth,
    handleWebPinSubmit,
    handleLogoutLan,
  };
}
