import { listen } from "@tauri-apps/api/event";

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

type EventHandler = (payload: unknown) => void;

const SSE_EVENT_MAP: Record<string, string> = {
  "vault://locked": "vault_locked",
  "vault://lan-stopped": "lan_stopped",
  "vault://lan-mode-changed": "lan_mode_changed",
  "vault://library-changed": "library_changed",
  "vault://url-import-progress": "url_import_progress",
};

const listeners = new Map<string, Set<EventHandler>>();
let eventSource: EventSource | null = null;

function dispatchWebEvent(eventName: string, rawData: string): void {
  const handlers = listeners.get(eventName);
  if (!handlers || handlers.size === 0) return;

  let parsed: unknown = undefined;
  if (rawData && rawData.length > 0) {
    try {
      parsed = JSON.parse(rawData);
    } catch {
      parsed = undefined;
    }
  }
  for (const fn of handlers) {
    try {
      fn(parsed);
    } catch {
      // ignore subscriber errors
    }
  }
}

export function connectLanEventStream(): void {
  if (isTauriRuntime() || typeof EventSource === "undefined") return;
  if (eventSource) return;

  const es = new EventSource("/api/events");
  let errorStreak = 0;

  es.onopen = () => {
    errorStreak = 0;
  };

  for (const [vaultEventName, sseName] of Object.entries(SSE_EVENT_MAP)) {
    es.addEventListener(sseName, (ev) => {
      errorStreak = 0;
      const msg = ev as MessageEvent<string>;
      dispatchWebEvent(vaultEventName, msg.data);
    });
  }

  es.onerror = () => {
    errorStreak += 1;
    if (es.readyState === EventSource.CLOSED || errorStreak >= 3) {
      if (eventSource === es) {
        disconnectLanEventStream();
        dispatchWebEvent("vault://lan-stopped", JSON.stringify({ reason: "connection_lost" }));
      }
    }
  };

  eventSource = es;
}

export function disconnectLanEventStream(): void {
  if (eventSource) {
    eventSource.close();
    eventSource = null;
  }
}

export async function subscribeVaultEvent<T = unknown>(
  eventName: string,
  handler: (payload: T) => void
): Promise<() => void> {
  if (isTauriRuntime()) {
    return listen<T>(eventName, (event) => handler(event.payload));
  }

  let set = listeners.get(eventName);
  if (!set) {
    set = new Set();
    listeners.set(eventName, set);
  }
  const wrapped: EventHandler = (payload) => handler(payload as T);
  set.add(wrapped);

  return () => {
    const current = listeners.get(eventName);
    if (!current) return;
    current.delete(wrapped);
    if (current.size === 0) {
      listeners.delete(eventName);
    }
  };
}
