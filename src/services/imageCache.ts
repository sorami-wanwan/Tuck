import type { ItemDetail } from "../types/vault";
import { isTauriRuntime } from "./runtime";
import { vaultApi } from "./vaultApi";

const THUMB_CACHE_LIMIT = 500;
const FULL_CACHE_LIMIT = 12;
const THUMB_MAX_EDGE = 400;

class LruUrlCache {
  private readonly map = new Map<string, string>();
  private readonly limit: number;

  constructor(limit: number) {
    this.limit = limit;
  }

  get(key: string): string | undefined {
    const val = this.map.get(key);
    if (val === undefined) return undefined;
    // Refresh recency
    this.map.delete(key);
    this.map.set(key, val);
    return val;
  }

  set(key: string, url: string): void {
    const existing = this.map.get(key);
    if (existing !== undefined) {
      if (existing !== url) {
        URL.revokeObjectURL(existing);
      }
      this.map.delete(key);
    }
    this.map.set(key, url);
    while (this.map.size > this.limit) {
      const oldestKey = this.map.keys().next().value;
      if (oldestKey === undefined) break;
      const oldestUrl = this.map.get(oldestKey);
      this.map.delete(oldestKey);
      if (oldestUrl !== undefined) {
        URL.revokeObjectURL(oldestUrl);
      }
    }
  }

  delete(key: string): void {
    const existing = this.map.get(key);
    if (existing !== undefined) {
      URL.revokeObjectURL(existing);
      this.map.delete(key);
    }
  }

  clear(): void {
    for (const url of this.map.values()) {
      URL.revokeObjectURL(url);
    }
    this.map.clear();
  }
}

class ImageMemoryCache {
  private readonly thumbs = new LruUrlCache(THUMB_CACHE_LIMIT);
  private readonly fulls = new LruUrlCache(FULL_CACHE_LIMIT);
  private readonly inflightThumbs = new Map<string, Promise<string>>();
  private readonly inflightFulls = new Map<string, Promise<string>>();
  private readonly inflightAvif = new Map<string, Promise<ItemDetail>>();
  private readonly inflightVideo = new Map<string, Promise<ItemDetail>>();
  private readonly failedPendingThumbs = new Set<string>();
  private videoQueueTail: Promise<void> = Promise.resolve();
  private epoch = 0;
  private lanMediaToken: string | null = null;
  private localMediaBaseUrl: string | null = null;
  private localMediaToken: string | null = null;

  setLanMediaToken(token: string | null): void {
    this.lanMediaToken = token;
  }

  setLocalMediaEndpoint(baseUrl: string | null, token: string | null): void {
    this.localMediaBaseUrl = baseUrl;
    this.localMediaToken = token;
  }

  usesHttpMediaStream(): boolean {
    return !isTauriRuntime() || Boolean(this.localMediaBaseUrl && this.localMediaToken);
  }

  hasPendingThumbnailFailed(itemId: string): boolean {
    return this.failedPendingThumbs.has(itemId);
  }

  getVaultMediaStreamUrl(itemId: string): string {
    const encodedId = encodeURIComponent(itemId);
    if (!isTauriRuntime()) {
      const mt = this.lanMediaToken
        ? `&mt=${encodeURIComponent(this.lanMediaToken)}`
        : "";
      return `/api/media/${encodedId}?e=${this.epoch}${mt}`;
    }
    if (this.localMediaBaseUrl && this.localMediaToken) {
      return `${this.localMediaBaseUrl}/media/${encodedId}?t=${encodeURIComponent(
        this.localMediaToken
      )}&e=${this.epoch}`;
    }
    const isWindows =
      typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);
    return isWindows
      ? `http://vaultmedia.localhost/${encodedId}?e=${this.epoch}`
      : `vaultmedia://localhost/${encodedId}?e=${this.epoch}`;
  }

  getCachedThumbnailUrl(itemId: string): string | undefined {
    return this.thumbs.get(itemId);
  }

  getThumbnailUrl(itemId: string): Promise<string> {
    const cached = this.thumbs.get(itemId);
    if (cached) return Promise.resolve(cached);

    const pending = this.inflightThumbs.get(itemId);
    if (pending) return pending;

    const currentEpoch = this.epoch;
    const promise = vaultApi
      .fetchThumbnailBytes(itemId)
      .then((buf) => {
        if (this.epoch !== currentEpoch) {
          throw new Error("Vault closed");
        }
        const blob = new Blob([buf], { type: "image/webp" });
        const url = URL.createObjectURL(blob);
        this.thumbs.set(itemId, url);
        return url;
      })
      .finally(() => {
        this.inflightThumbs.delete(itemId);
      });

    this.inflightThumbs.set(itemId, promise);
    return promise;
  }

  getFullImageUrl(itemId: string, mimeType: string): Promise<string> {
    const cached = this.fulls.get(itemId);
    if (cached) return Promise.resolve(cached);

    const pending = this.inflightFulls.get(itemId);
    if (pending) return pending;

    const currentEpoch = this.epoch;
    const promise = vaultApi
      .fetchImageBytes(itemId)
      .then((buf) => {
        if (this.epoch !== currentEpoch) {
          throw new Error("Vault closed");
        }
        const blob = new Blob([buf], { type: mimeType });
        const url = URL.createObjectURL(blob);
        this.fulls.set(itemId, url);
        return url;
      })
      .finally(() => {
        this.inflightFulls.delete(itemId);
      });

    this.inflightFulls.set(itemId, promise);
    return promise;
  }

  prefetchFullImage(itemId: string, mimeType: string): void {
    if (this.fulls.get(itemId) || this.inflightFulls.has(itemId)) return;
    void this.getFullImageUrl(itemId, mimeType).catch(() => {});
  }

  processPendingAvifThumbnail(itemId: string): Promise<ItemDetail> {
    if (this.failedPendingThumbs.has(itemId)) {
      return Promise.reject(new Error("AVIF thumbnail previously failed"));
    }
    const existing = this.inflightAvif.get(itemId);
    if (existing) return existing;

    const currentEpoch = this.epoch;
    const promise = (async () => {
      const avifBytes = await vaultApi.fetchImageBytes(itemId);
      if (this.epoch !== currentEpoch) {
        throw new Error("Vault closed");
      }

      const avifBlob = new Blob([avifBytes], { type: "image/avif" });
      const bitmap = await createImageBitmap(avifBlob);
      try {
        const origW = bitmap.width;
        const origH = bitmap.height;
        const scale = Math.min(1, THUMB_MAX_EDGE / Math.max(origW, origH, 1));
        const tw = Math.max(1, Math.round(origW * scale));
        const th = Math.max(1, Math.round(origH * scale));

        const canvas = document.createElement("canvas");
        canvas.width = tw;
        canvas.height = th;
        const ctx = canvas.getContext("2d");
        if (!ctx) {
          throw new Error("Canvas context unavailable");
        }
        ctx.drawImage(bitmap, 0, 0, tw, th);

        const pngBlob = await new Promise<Blob>((resolve, reject) => {
          canvas.toBlob(
            (b) => (b ? resolve(b) : reject(new Error("Canvas PNG encode failed"))),
            "image/png"
          );
        });
        const pngBytes = await pngBlob.arrayBuffer();
        if (this.epoch !== currentEpoch) {
          throw new Error("Vault closed");
        }
        const updated = await vaultApi.saveMediaThumbnail(itemId, pngBytes, origW, origH, 0);
        this.failedPendingThumbs.delete(itemId);
        this.thumbs.delete(itemId);
        return updated;
      } finally {
        bitmap.close();
      }
    })()
      .catch((err) => {
        if (this.epoch === currentEpoch) {
          this.failedPendingThumbs.add(itemId);
        }
        throw err;
      })
      .finally(() => {
        this.inflightAvif.delete(itemId);
      });

    this.inflightAvif.set(itemId, promise);
    return promise;
  }

  private captureVideoFrame(
    srcUrl: string,
    useCrossOrigin: boolean
  ): Promise<{ pngBytes: ArrayBuffer; width: number; height: number; durationMs: number }> {
    return new Promise((resolve, reject) => {
      const video = document.createElement("video");
      video.muted = true;
      video.playsInline = true;
      video.preload = "auto";
      if (useCrossOrigin) {
        video.crossOrigin = "anonymous";
      }

      let finished = false;
      let seekTimer: number | null = null;
      const cleanup = () => {
        window.clearTimeout(timer);
        if (seekTimer !== null) {
          window.clearTimeout(seekTimer);
          seekTimer = null;
        }
        video.pause();
        video.onloadeddata = null;
        video.onseeked = null;
        video.onerror = null;
        video.removeAttribute("src");
        video.load();
      };

      const fail = (err: unknown) => {
        if (finished) return;
        finished = true;
        cleanup();
        reject(err instanceof Error ? err : new Error(String(err)));
      };

      const timer = window.setTimeout(() => {
        fail(new Error("Video thumbnail timeout"));
      }, 15_000);

      const drawFrame = () => {
        if (finished) return;
        if (seekTimer !== null) {
          window.clearTimeout(seekTimer);
          seekTimer = null;
        }
        try {
          const origW = video.videoWidth || 320;
          const origH = video.videoHeight || 240;
          const durationMs =
            Number.isFinite(video.duration) && video.duration > 0
              ? Math.round(video.duration * 1000)
              : 0;
          const scale = Math.min(1, THUMB_MAX_EDGE / Math.max(origW, origH, 1));
          const tw = Math.max(1, Math.round(origW * scale));
          const th = Math.max(1, Math.round(origH * scale));

          const canvas = document.createElement("canvas");
          canvas.width = tw;
          canvas.height = th;
          const ctx = canvas.getContext("2d");
          if (!ctx) {
            fail(new Error("Canvas context unavailable"));
            return;
          }
          ctx.drawImage(video, 0, 0, tw, th);
          canvas.toBlob((blob) => {
            if (!blob) {
              fail(new Error("Canvas PNG encode failed"));
              return;
            }
            void blob
              .arrayBuffer()
              .then((pngBytes) => {
                if (finished) return;
                finished = true;
                cleanup();
                resolve({ pngBytes, width: origW, height: origH, durationMs });
              })
              .catch(fail);
          }, "image/png");
        } catch (err) {
          fail(err);
        }
      };

      video.onerror = () => fail(new Error("Video load error"));
      video.onloadeddata = () => {
        video.onloadeddata = null;
        const dur = video.duration;
        if (Number.isFinite(dur) && dur > 0.5) {
          video.onseeked = drawFrame;
          seekTimer = window.setTimeout(drawFrame, 4000);
          try {
            video.currentTime = Math.min(0.5, dur * 0.1);
          } catch {
            drawFrame();
          }
        } else {
          drawFrame();
        }
      };

      video.src = srcUrl;
    });
  }

  processPendingVideoThumbnail(itemId: string, mimeType: string): Promise<ItemDetail> {
    if (this.failedPendingThumbs.has(itemId)) {
      return Promise.reject(new Error("Video thumbnail previously failed"));
    }
    const existing = this.inflightVideo.get(itemId);
    if (existing) return existing;

    const currentEpoch = this.epoch;
    const runTask = async (): Promise<ItemDetail> => {
      if (this.epoch !== currentEpoch) {
        throw new Error("Vault closed");
      }

      let frame: { pngBytes: ArrayBuffer; width: number; height: number; durationMs: number };
      try {
        frame = await this.captureVideoFrame(this.getVaultMediaStreamUrl(itemId), true);
      } catch (streamErr) {
        if (this.epoch !== currentEpoch) {
          throw new Error("Vault closed");
        }
        if (this.usesHttpMediaStream()) {
          throw streamErr;
        }
        // Fallback to temporary in-memory Blob URL only for <=32 MiB videos if canvas readback fails
        const detail = await vaultApi.getItemDetail(itemId);
        if (detail.byteSize > 32 * 1024 * 1024) {
          throw new Error("Large video thumbnail fallback skipped");
        }
        const bytes = await vaultApi.fetchImageBytes(itemId);
        if (this.epoch !== currentEpoch) {
          throw new Error("Vault closed");
        }
        const resolvedMime = mimeType || detail.mimeType;
        const blobUrl = URL.createObjectURL(new Blob([bytes], { type: resolvedMime }));
        try {
          frame = await this.captureVideoFrame(blobUrl, false);
        } finally {
          URL.revokeObjectURL(blobUrl);
        }
      }

      if (this.epoch !== currentEpoch) {
        throw new Error("Vault closed");
      }

      const updated = await vaultApi.saveMediaThumbnail(
        itemId,
        frame.pngBytes,
        frame.width,
        frame.height,
        frame.durationMs
      );
      this.failedPendingThumbs.delete(itemId);
      this.thumbs.delete(itemId);
      return updated;
    };

    const queued = this.videoQueueTail.catch(() => {}).then(runTask);
    this.videoQueueTail = queued.then(
      () => undefined,
      () => undefined
    );

    const promise = queued
      .catch((err) => {
        if (this.epoch === currentEpoch) {
          this.failedPendingThumbs.add(itemId);
        }
        throw err;
      })
      .finally(() => {
        this.inflightVideo.delete(itemId);
      });

    this.inflightVideo.set(itemId, promise);
    return promise;
  }

  invalidateItems(itemIds: string[]): void {
    for (const id of itemIds) {
      this.thumbs.delete(id);
      this.fulls.delete(id);
      this.failedPendingThumbs.delete(id);
    }
  }

  revokeAll(): void {
    this.epoch += 1;
    this.lanMediaToken = null;
    this.localMediaBaseUrl = null;
    this.localMediaToken = null;
    this.thumbs.clear();
    this.fulls.clear();
    this.inflightThumbs.clear();
    this.inflightFulls.clear();
    this.inflightAvif.clear();
    this.inflightVideo.clear();
    this.failedPendingThumbs.clear();
    this.videoQueueTail = Promise.resolve();
  }
}

export const imageMemoryCache = new ImageMemoryCache();
