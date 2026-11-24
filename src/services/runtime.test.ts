// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

class MockEventSource {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSED = 2;
  static instances: MockEventSource[] = [];

  readonly url: string;
  readyState = MockEventSource.OPEN;
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;
  private readonly handlers = new Map<string, Set<(ev: Event) => void>>();

  constructor(url: string) {
    this.url = url;
    MockEventSource.instances.push(this);
  }

  addEventListener(type: string, cb: (ev: Event) => void): void {
    let set = this.handlers.get(type);
    if (!set) {
      set = new Set();
      this.handlers.set(type, set);
    }
    set.add(cb);
  }

  emit(type: string, data: string): void {
    const ev = { data } as unknown as Event;
    for (const cb of this.handlers.get(type) ?? []) {
      cb(ev);
    }
  }

  close(): void {
    this.closed = true;
    this.readyState = MockEventSource.CLOSED;
  }
}

import {
  connectLanEventStream,
  disconnectLanEventStream,
  isTauriRuntime,
  subscribeVaultEvent,
} from "./runtime";

describe("runtime LAN event stream", () => {
  beforeEach(() => {
    MockEventSource.instances = [];
    vi.stubGlobal("EventSource", MockEventSource);
    disconnectLanEventStream();
  });

  afterEach(() => {
    disconnectLanEventStream();
    vi.unstubAllGlobals();
  });

  it("delivers SSE events to web subscribers until unsubscribed", async () => {
    expect(isTauriRuntime()).toBe(false);
    const received: unknown[] = [];
    const unsub = await subscribeVaultEvent("vault://library-changed", (payload) => {
      received.push(payload);
    });

    connectLanEventStream();
    expect(MockEventSource.instances).toHaveLength(1);
    const es = MockEventSource.instances[0];
    expect(es.url).toBe("/api/events");

    es.emit("library_changed", JSON.stringify({ scope: "all" }));
    expect(received).toEqual([{ scope: "all" }]);

    unsub();
    es.emit("library_changed", JSON.stringify({ scope: "again" }));
    expect(received).toHaveLength(1);
  });

  it("resets errorStreak on open/message and disconnects after 3 consecutive errors", async () => {
    const stoppedPayloads: unknown[] = [];
    const unsub = await subscribeVaultEvent("vault://lan-stopped", (payload) => {
      stoppedPayloads.push(payload);
    });

    connectLanEventStream();
    const es = MockEventSource.instances[0];

    es.onerror?.();
    es.onerror?.();
    expect(es.closed).toBe(false);
    expect(stoppedPayloads).toHaveLength(0);

    // Recovery resets errorStreak
    es.onopen?.();
    es.onerror?.();
    es.onerror?.();
    expect(es.closed).toBe(false);

    // 3rd consecutive error triggers disconnect + connection_lost event
    es.onerror?.();
    expect(es.closed).toBe(true);
    expect(stoppedPayloads).toEqual([{ reason: "connection_lost" }]);

    unsub();
  });

  it("disconnects immediately when EventSource enters CLOSED state", async () => {
    const stoppedPayloads: unknown[] = [];
    const unsub = await subscribeVaultEvent("vault://lan-stopped", (payload) => {
      stoppedPayloads.push(payload);
    });

    connectLanEventStream();
    const es = MockEventSource.instances[0];
    es.readyState = MockEventSource.CLOSED;
    es.onerror?.();

    expect(es.closed).toBe(true);
    expect(stoppedPayloads).toEqual([{ reason: "connection_lost" }]);
    unsub();
  });
});
