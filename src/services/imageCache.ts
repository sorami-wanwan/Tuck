import type { ItemDetail } from "../types/vault";
import { isTauriRuntime } from "./runtime";
import { vaultApi } from "./vaultApi";

const THUMB_CACHE_LIMIT = 500;
const FULL_CACHE_LIMIT = 12;
const THUMB_MAX_EDGE = 400;
const MAX_CONCURRENT_AVIF = 3;

function computeSmartThumbnailRect(
  origW: number,
  origH: number
): { sx: number; sy: number; sw: number; sh: number; tw: number; th: number } {
  let sx = 0;
  let sy = 0;
  let sw = Math.max(1, origW);
  let sh = Math.max(1, origH);

  if (origH > origW * 3) {
    // Tall webtoon strip: top-aligned 1:3 crop
    sh = Math.min(origH, Math.max(1, origW * 3));
  } else if (origW > origH * 3) {
    // Wide panorama: center 1:3 crop
    sw = Math.min(origW, Math.max(1, origH * 3));
    sx = Math.floor((origW - sw) / 2);
  }

  const scale = Math.min(1, THUMB_MAX_EDGE / Math.max(sw, sh, 1));
  const tw = Math.max(1, Math.round(sw * scale));
  const th = Math.max(1, Math.round(sh * scale));
  return { sx, sy, sw, sh, tw, th };
}

type FrameQualityKind = "good" | "flat" | "noise" | "blank";

interface FrameQuality {
  kind: FrameQualityKind;
  score: number;
}

interface CapturedFrame {
  pngBytes: ArrayBuffer;
  width: number;
  height: number;
  durationMs: number;
}

function calcStd(sum: number, sum2: number, n: number): number {
  if (n <= 0) return 0;
  const mean = sum / n;
  return Math.sqrt(Math.max(0, sum2 / n - mean * mean));
}

function calcPearson(
  n: number,
  sx: number,
  sy: number,
  sx2: number,
  sy2: number,
  sxy: number
): number {
  if (n <= 0) return 1.0;
  const mx = sx / n;
  const my = sy / n;
  const vx = Math.max(0, sx2 / n - mx * mx);
  const vy = Math.max(0, sy2 / n - my * my);
  const denom = Math.sqrt(vx * vy);
  if (denom <= 1e-6) return 1.0;
  return (sxy / n - mx * my) / denom;
}

function analyzeCanvasFrame(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number
): FrameQuality {
  if (width <= 0 || height <= 0) {
    return { kind: "blank", score: -1 };
  }
  const data = ctx.getImageData(0, 0, width, height).data;
  const step = Math.max(1, Math.floor(Math.max(width, height) / 160));

  let totalSamples = 0;
  let opaqueCount = 0;
  let darkCount = 0;
  let brightCount = 0;

  let sumR = 0;
  let sumR2 = 0;
  let sumG = 0;
  let sumG2 = 0;
  let sumB = 0;
  let sumB2 = 0;
  let sumY = 0;
  let sumY2 = 0;

  let nh = 0;
  let sumHDiff = 0;
  let sxh = 0;
  let syh = 0;
  let sxh2 = 0;
  let syh2 = 0;
  let sxyh = 0;

  let nv = 0;
  let sumVDiff = 0;
  let sxv = 0;
  let syv = 0;
  let sxv2 = 0;
  let syv2 = 0;
  let sxyv = 0;

  for (let y = 0; y < height; y += step) {
    for (let x = 0; x < width; x += step) {
      totalSamples += 1;
      const off = (y * width + x) * 4;
      const a = data[off + 3];
      if (a < 16) continue;
      opaqueCount += 1;

      const r = data[off];
      const g = data[off + 1];
      const b = data[off + 2];
      const lum = 0.299 * r + 0.587 * g + 0.114 * b;

      sumR += r;
      sumR2 += r * r;
      sumG += g;
      sumG2 += g * g;
      sumB += b;
      sumB2 += b * b;
      sumY += lum;
      sumY2 += lum * lum;

      if (lum < 14) darkCount += 1;
      if (lum > 242) brightCount += 1;

      if (x + 1 < width) {
        const offH = (y * width + (x + 1)) * 4;
        if (data[offH + 3] >= 16) {
          const r2 = data[offH];
          const g2 = data[offH + 1];
          const b2 = data[offH + 2];
          const lum2 = 0.299 * r2 + 0.587 * g2 + 0.114 * b2;
          sumHDiff += (Math.abs(r - r2) + Math.abs(g - g2) + Math.abs(b - b2)) / 3;
          sxh += lum;
          syh += lum2;
          sxh2 += lum * lum;
          syh2 += lum2 * lum2;
          sxyh += lum * lum2;
          nh += 1;
        }
      }

      if (y + 1 < height) {
        const offV = ((y + 1) * width + x) * 4;
        if (data[offV + 3] >= 16) {
          const r2 = data[offV];
          const g2 = data[offV + 1];
          const b2 = data[offV + 2];
          const lum2 = 0.299 * r2 + 0.587 * g2 + 0.114 * b2;
          sumVDiff += (Math.abs(r - r2) + Math.abs(g - g2) + Math.abs(b - b2)) / 3;
          sxv += lum;
          syv += lum2;
          sxv2 += lum * lum;
          syv2 += lum2 * lum2;
          sxyv += lum * lum2;
          nv += 1;
        }
      }
    }
  }

  if (totalSamples === 0 || opaqueCount === 0 || opaqueCount / totalSamples < 0.05) {
    return { kind: "blank", score: -1 };
  }

  const stdY = calcStd(sumY, sumY2, opaqueCount);
  const colorSpread =
    calcStd(sumR, sumR2, opaqueCount) +
    calcStd(sumG, sumG2, opaqueCount) +
    calcStd(sumB, sumB2, opaqueCount);
  const darkRatio = darkCount / opaqueCount;
  const brightRatio = brightCount / opaqueCount;

  const corrH = calcPearson(nh, sxh, syh, sxh2, syh2, sxyh);
  const corrV = calcPearson(nv, sxv, syv, sxv2, syv2, sxyv);
  const meanHDiff = nh > 0 ? sumHDiff / nh : 0;
  const meanVDiff = nv > 0 ? sumVDiff / nv : 0;

  if (
    (stdY < 7 && colorSpread < 18) ||
    (darkRatio > 0.96 && stdY < 16) ||
    (brightRatio > 0.96 && stdY < 16)
  ) {
    return { kind: "flat", score: stdY * 0.5 + colorSpread * 0.1 };
  }

  if (
    stdY >= 10 &&
    corrH < 0.35 &&
    corrV < 0.35 &&
    meanHDiff / stdY > 0.78 &&
    meanVDiff / stdY > 0.78
  ) {
    return {
      kind: "noise",
      score: 5 + (Math.max(0, corrH) + Math.max(0, corrV)) * 10,
    };
  }

  return {
    kind: "good",
    score: 100 + Math.min(80, stdY) + Math.min(40, colorSpread * 0.2) - darkRatio * 30,
  };
}

function buildCandidateTimestampsSec(durationSec: number): number[] {
  if (durationSec === Infinity) {
    // Duration-less WebM/stream: still try non-zero timestamps before falling back to 0s
    return [0.25, 1.0, 0.0];
  }
  if (!Number.isFinite(durationSec) || durationSec <= 0) {
    return [0];
  }
  const maxTs = Math.max(0, durationSec - 0.1);
  let raw: number[];
  let minGap: number;
  if (durationSec >= 5) {
    raw = [
      Math.min(durationSec * 0.12, 15),
      durationSec * 0.25,
      durationSec * 0.5,
      1.0,
      0.0,
    ];
    minGap = 0.4;
  } else if (durationSec >= 0.5) {
    raw = [durationSec * 0.25, durationSec * 0.5, 0.0];
    minGap = 0.2;
  } else {
    return [0];
  }

  const out: number[] = [];
  for (const t of raw) {
    const clamped = Math.min(maxTs, Math.max(0, t));
    if (!out.some((prev) => Math.abs(prev - clamped) < minGap)) {
      out.push(clamped);
    }
  }
  return out.length > 0 ? out : [0];
}

function waitForFrameSettle(video: HTMLVideoElement): Promise<void> {
  const rvfc = (
    video as HTMLVideoElement & {
      requestVideoFrameCallback?: (cb: () => void) => number;
    }
  ).requestVideoFrameCallback;

  const afterRaf = () =>
    new Promise<void>((resolve) => {
      window.requestAnimationFrame(() => resolve());
    });

  if (typeof rvfc === "function") {
    return new Promise<void>((resolve) => {
      let done = false;
      const timer = window.setTimeout(() => {
        if (done) return;
        done = true;
        void afterRaf().then(resolve);
      }, 220);
      try {
        rvfc.call(video, () => {
          if (done) return;
          done = true;
          window.clearTimeout(timer);
          void afterRaf().then(resolve);
        });
      } catch {
        window.clearTimeout(timer);
        if (!done) {
          done = true;
          void afterRaf().then(resolve);
        }
      }
    });
  }

  return new Promise<void>((resolve) => {
    window.requestAnimationFrame(() => {
      window.requestAnimationFrame(() => resolve());
    });
  });
}

async function canvasToPngBuffer(canvas: HTMLCanvasElement): Promise<ArrayBuffer> {
  const pngBlob = await new Promise<Blob>((resolve, reject) => {
    canvas.toBlob(
      (b) => (b ? resolve(b) : reject(new Error("Canvas PNG encode failed"))),
      "image/png"
    );
  });
  return pngBlob.arrayBuffer();
}

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
  private readonly inflightDisplay = new Map<string, Promise<string>>();
  private readonly inflightAvif = new Map<string, Promise<ItemDetail | null>>();
  private readonly inflightVideo = new Map<string, Promise<ItemDetail | null>>();
  private readonly failedPendingThumbs = new Set<string>();
  private readonly thumbVersions = new Map<string, number>();
  private readonly thumbListeners = new Map<string, Set<() => void>>();
  private readonly knownHasThumb = new Set<string>();
  private avifActiveCount = 0;
  private avifWaiters: Array<() => void> = [];
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

  getThumbnailVersion(itemId: string): number {
    return this.thumbVersions.get(itemId) ?? 0;
  }

  hasKnownThumbnail(itemId: string): boolean {
    return this.knownHasThumb.has(itemId);
  }

  subscribeThumbnailChange(itemId: string, listener: () => void): () => void {
    let set = this.thumbListeners.get(itemId);
    if (!set) {
      set = new Set();
      this.thumbListeners.set(itemId, set);
    }
    set.add(listener);
    return () => {
      const cur = this.thumbListeners.get(itemId);
      if (!cur) return;
      cur.delete(listener);
      if (cur.size === 0) {
        this.thumbListeners.delete(itemId);
      }
    };
  }

  private emitThumbnailListeners(itemId: string): void {
    this.thumbVersions.set(itemId, (this.thumbVersions.get(itemId) ?? 0) + 1);
    const listeners = this.thumbListeners.get(itemId);
    if (listeners) {
      for (const fn of Array.from(listeners)) {
        try {
          fn();
        } catch {
          // ignore listener errors
        }
      }
    }
  }

  private cacheLocalThumbnail(itemId: string, bytes: ArrayBuffer, mimeType: string): void {
    const url = URL.createObjectURL(new Blob([bytes], { type: mimeType }));
    this.failedPendingThumbs.delete(itemId);
    this.knownHasThumb.add(itemId);
    this.thumbs.set(itemId, url);
    this.inflightThumbs.delete(itemId);
    this.emitThumbnailListeners(itemId);
  }

  private async withAvifSlot<T>(epoch: number, fn: () => Promise<T>): Promise<T> {
    while (this.avifActiveCount >= MAX_CONCURRENT_AVIF) {
      if (this.epoch !== epoch) {
        throw new Error("Vault closed");
      }
      await new Promise<void>((resolve) => {
        this.avifWaiters.push(resolve);
      });
    }
    if (this.epoch !== epoch) {
      throw new Error("Vault closed");
    }
    this.avifActiveCount += 1;
    try {
      return await fn();
    } finally {
      this.avifActiveCount = Math.max(0, this.avifActiveCount - 1);
      const next = this.avifWaiters.shift();
      next?.();
    }
  }

  notifyThumbnailsUpdated(itemIds: string[]): void {
    for (const id of itemIds) {
      this.failedPendingThumbs.delete(id);
      this.knownHasThumb.add(id);
      this.thumbs.delete(id);
      this.inflightThumbs.delete(id);
      this.emitThumbnailListeners(id);
    }
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
        this.knownHasThumb.add(itemId);
        return url;
      })
      .finally(() => {
        if (this.inflightThumbs.get(itemId) === promise) {
          this.inflightThumbs.delete(itemId);
        }
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
    const isTiff = mimeType === "image/tiff";
    const fetchPromise = isTiff
      ? vaultApi.fetchDisplayImageBytes(itemId)
      : vaultApi.fetchImageBytes(itemId);
    const blobMime = isTiff ? "image/webp" : mimeType;

    const promise = fetchPromise
      .then((buf) => {
        if (this.epoch !== currentEpoch) {
          throw new Error("Vault closed");
        }
        const blob = new Blob([buf], { type: blobMime });
        const url = URL.createObjectURL(blob);
        this.fulls.set(itemId, url);
        return url;
      })
      .finally(() => {
        if (this.inflightFulls.get(itemId) === promise) {
          this.inflightFulls.delete(itemId);
        }
      });

    this.inflightFulls.set(itemId, promise);
    return promise;
  }

  getDisplayFallbackUrl(itemId: string, mimeType: string): Promise<string> {
    if (mimeType === "image/avif" || mimeType.startsWith("video/")) {
      return Promise.reject(new Error("Display conversion unsupported"));
    }
    const pending = this.inflightDisplay.get(itemId);
    if (pending) return pending;

    const currentEpoch = this.epoch;
    const promise = vaultApi
      .fetchDisplayImageBytes(itemId)
      .then((buf) => {
        if (this.epoch !== currentEpoch) {
          throw new Error("Vault closed");
        }
        const url = URL.createObjectURL(new Blob([buf], { type: "image/webp" }));
        this.fulls.set(itemId, url);
        return url;
      })
      .finally(() => {
        if (this.inflightDisplay.get(itemId) === promise) {
          this.inflightDisplay.delete(itemId);
        }
      });

    this.inflightDisplay.set(itemId, promise);
    return promise;
  }

  prefetchFullImage(itemId: string, mimeType: string): void {
    if (this.fulls.get(itemId) || this.inflightFulls.has(itemId)) return;
    void this.getFullImageUrl(itemId, mimeType).catch(() => {});
  }

  processPendingAvifThumbnail(
    itemId: string,
    options?: { force?: boolean; persist?: boolean }
  ): Promise<ItemDetail | null> {
    const persist = options?.persist ?? true;
    if (options?.force) {
      this.failedPendingThumbs.delete(itemId);
    } else if (this.failedPendingThumbs.has(itemId)) {
      return Promise.reject(new Error("AVIF thumbnail previously failed"));
    }

    const existing = this.inflightAvif.get(itemId);
    if (existing) return existing;

    const currentEpoch = this.epoch;
    const promise = this.withAvifSlot(currentEpoch, async () => {
      const avifBytes = await vaultApi.fetchImageBytes(itemId);
      if (this.epoch !== currentEpoch) {
        throw new Error("Vault closed");
      }

      const avifBlob = new Blob([avifBytes], { type: "image/avif" });
      const bitmap = await createImageBitmap(avifBlob);
      try {
        const origW = bitmap.width;
        const origH = bitmap.height;
        const { sx, sy, sw, sh, tw, th } = computeSmartThumbnailRect(origW, origH);

        const canvas = document.createElement("canvas");
        canvas.width = tw;
        canvas.height = th;
        const ctx = canvas.getContext("2d");
        if (!ctx) {
          throw new Error("Canvas context unavailable");
        }
        ctx.drawImage(bitmap, sx, sy, sw, sh, 0, 0, tw, th);

        const pngBytes = await canvasToPngBuffer(canvas);
        if (this.epoch !== currentEpoch) {
          throw new Error("Vault closed");
        }
        if (!persist) {
          this.cacheLocalThumbnail(itemId, pngBytes, "image/png");
          return null;
        }
        const updated = await vaultApi.saveMediaThumbnail(itemId, pngBytes, origW, origH, 0);
        this.notifyThumbnailsUpdated([itemId]);
        return updated;
      } finally {
        bitmap.close();
      }
    })
      .catch((err) => {
        if (this.epoch === currentEpoch && !options?.force) {
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
    useCrossOrigin: boolean,
    explicitTimestampMs?: number
  ): Promise<CapturedFrame> {
    return new Promise((resolve, reject) => {
      const video = document.createElement("video");
      video.muted = true;
      video.playsInline = true;
      video.preload = "auto";
      if (useCrossOrigin) {
        video.crossOrigin = "anonymous";
      }

      let finished = false;
      let activeSeekTimer: number | null = null;
      let activeKickTimer: number | null = null;

      const cleanup = () => {
        window.clearTimeout(overallTimer);
        if (activeSeekTimer !== null) {
          window.clearTimeout(activeSeekTimer);
          activeSeekTimer = null;
        }
        if (activeKickTimer !== null) {
          window.clearTimeout(activeKickTimer);
          activeKickTimer = null;
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

      const succeed = (frame: CapturedFrame) => {
        if (finished) return;
        finished = true;
        cleanup();
        resolve(frame);
      };

      const overallTimer = window.setTimeout(() => {
        fail(new Error("Video thumbnail timeout"));
      }, 20_000);

      const seekTo = (targetSec: number): Promise<boolean> => {
        if (finished) return Promise.resolve(false);
        if (Math.abs((video.currentTime || 0) - targetSec) <= 0.05) {
          return Promise.resolve(true);
        }
        return new Promise<boolean>((res) => {
          let settled = false;
          const finishSeek = (ok: boolean) => {
            if (settled) return;
            settled = true;
            if (activeSeekTimer !== null) {
              window.clearTimeout(activeSeekTimer);
              activeSeekTimer = null;
            }
            video.onseeked = null;
            res(ok);
          };
          video.onseeked = () => finishSeek(true);
          activeSeekTimer = window.setTimeout(() => finishSeek(false), 4000);
          try {
            video.currentTime = targetSec;
          } catch {
            finishSeek(false);
          }
        });
      };

      const kickDecoder = async (): Promise<void> => {
        if (finished) return;
        await video.play().catch(() => {});
        if (finished) return;
        await new Promise<void>((res) => {
          activeKickTimer = window.setTimeout(() => {
            activeKickTimer = null;
            res();
          }, 120);
        });
        video.pause();
        if (!finished) {
          await waitForFrameSettle(video);
        }
      };

      const renderAndInspect = async (): Promise<{
        quality: FrameQuality;
        frame?: CapturedFrame;
      }> => {
        const origW = video.videoWidth;
        const origH = video.videoHeight;
        if (!origW || !origH) {
          throw new Error("Video dimensions unavailable");
        }
        const durationMs =
          Number.isFinite(video.duration) && video.duration > 0
            ? Math.round(video.duration * 1000)
            : 0;
        const { sx, sy, sw, sh, tw, th } = computeSmartThumbnailRect(origW, origH);

        const canvas = document.createElement("canvas");
        canvas.width = tw;
        canvas.height = th;
        const ctx = canvas.getContext("2d", { willReadFrequently: true });
        if (!ctx) {
          throw new Error("Canvas context unavailable");
        }
        ctx.clearRect(0, 0, tw, th);
        ctx.drawImage(video, sx, sy, sw, sh, 0, 0, tw, th);
        const quality = analyzeCanvasFrame(ctx, tw, th);
        if (quality.kind === "blank" || quality.kind === "noise") {
          return { quality };
        }
        const pngBytes = await canvasToPngBuffer(canvas);
        return {
          quality,
          frame: { pngBytes, width: origW, height: origH, durationMs },
        };
      };

      const runCapture = async () => {
        if (!video.videoWidth || !video.videoHeight) {
          fail(new Error("Video dimensions unavailable"));
          return;
        }

        const dur = video.duration;
        const maxTs =
          Number.isFinite(dur) && dur > 0
            ? Math.max(0, dur - 0.1)
            : dur === Infinity
              ? Number.MAX_SAFE_INTEGER
              : 0;

        if (explicitTimestampMs !== undefined) {
          const targetSec = Math.min(maxTs, Math.max(0, explicitTimestampMs / 1000));
          await seekTo(targetSec);
          if (finished) return;
          await waitForFrameSettle(video);
          if (finished) return;

          let inspected = await renderAndInspect();
          if (inspected.quality.kind === "blank") {
            await kickDecoder();
            if (finished) return;
            if (Math.abs((video.currentTime || 0) - targetSec) > 0.35) {
              await seekTo(targetSec);
              if (finished) return;
              await waitForFrameSettle(video);
              if (finished) return;
            }
            inspected = await renderAndInspect();
          }

          if (inspected.quality.kind === "blank") {
            fail(new Error("動画フレームを取得できませんでした"));
            return;
          }
          if (inspected.quality.kind === "noise") {
            fail(
              new Error(
                "動画フレームの取得結果が破損しています（砂嵐検出）。設定画面で ffmpeg を導入すると安定して生成できます"
              )
            );
            return;
          }
          if (inspected.frame) {
            succeed(inspected.frame);
          } else {
            fail(new Error("動画フレームを取得できませんでした"));
          }
          return;
        }

        const candidates = buildCandidateTimestampsSec(dur);
        let bestFlat: { frame: CapturedFrame; score: number } | null = null;
        let sawNoise = false;
        let sawBlank = false;
        let kickedOnce = false;

        for (const ts of candidates) {
          if (finished) return;
          const seekOk = await seekTo(ts);
          if (finished) return;
          if (!seekOk && ts > 0.05) {
            continue;
          }
          await waitForFrameSettle(video);
          if (finished) return;

          let inspected = await renderAndInspect();
          if (inspected.quality.kind === "blank" && !kickedOnce) {
            kickedOnce = true;
            await kickDecoder();
            if (finished) return;
            if (Math.abs((video.currentTime || 0) - ts) > 0.35) {
              await seekTo(ts);
              if (finished) return;
              await waitForFrameSettle(video);
              if (finished) return;
            }
            inspected = await renderAndInspect();
          }

          if (inspected.quality.kind === "good" && inspected.frame) {
            succeed(inspected.frame);
            return;
          }
          if (inspected.quality.kind === "flat" && inspected.frame) {
            if (!bestFlat || inspected.quality.score > bestFlat.score) {
              bestFlat = { frame: inspected.frame, score: inspected.quality.score };
            }
          } else if (inspected.quality.kind === "noise") {
            sawNoise = true;
          } else if (inspected.quality.kind === "blank") {
            sawBlank = true;
          }
        }

        if (bestFlat) {
          succeed(bestFlat.frame);
          return;
        }
        if (sawNoise) {
          fail(
            new Error(
              "動画フレームの取得結果が破損しています（砂嵐検出）。設定画面で ffmpeg を導入すると安定して生成できます"
            )
          );
          return;
        }
        if (sawBlank) {
          fail(new Error("動画フレームを取得できませんでした"));
          return;
        }
        fail(new Error("動画の有効なフレームを取得できませんでした"));
      };

      video.onerror = () => fail(new Error("Video load error"));
      video.onloadeddata = () => {
        video.onloadeddata = null;
        void runCapture().catch(fail);
      };

      video.src = srcUrl;
    });
  }

  processPendingVideoThumbnail(
    itemId: string,
    mimeType: string,
    options?: { force?: boolean; timestampMs?: number; persist?: boolean }
  ): Promise<ItemDetail | null> {
    const persist = options?.persist ?? true;
    const isManualOrForce = Boolean(options?.force || options?.timestampMs !== undefined);
    if (isManualOrForce) {
      this.failedPendingThumbs.delete(itemId);
    } else if (this.failedPendingThumbs.has(itemId)) {
      return Promise.reject(new Error("Video thumbnail previously failed"));
    }

    if (!isManualOrForce) {
      const existing = this.inflightVideo.get(itemId);
      if (existing) return existing;
    }

    const currentEpoch = this.epoch;
    const runTask = async (): Promise<ItemDetail | null> => {
      if (this.epoch !== currentEpoch) {
        throw new Error("Vault closed");
      }

      if (persist) {
        const hostRes = await vaultApi.regenerateItemThumbnail(
          itemId,
          options?.timestampMs ?? null
        );
        if (this.epoch !== currentEpoch) {
          throw new Error("Vault closed");
        }
        if (hostRes.status === "updated" && hostRes.detail) {
          this.notifyThumbnailsUpdated([itemId]);
          return hostRes.detail;
        }
      }

      let frame: CapturedFrame;
      try {
        frame = await this.captureVideoFrame(
          this.getVaultMediaStreamUrl(itemId),
          true,
          options?.timestampMs
        );
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
          throw streamErr;
        }
        const bytes = await vaultApi.fetchImageBytes(itemId);
        if (this.epoch !== currentEpoch) {
          throw new Error("Vault closed");
        }
        const resolvedMime = mimeType || detail.mimeType;
        const blobUrl = URL.createObjectURL(new Blob([bytes], { type: resolvedMime }));
        try {
          frame = await this.captureVideoFrame(blobUrl, false, options?.timestampMs);
        } finally {
          URL.revokeObjectURL(blobUrl);
        }
      }

      if (this.epoch !== currentEpoch) {
        throw new Error("Vault closed");
      }

      if (!persist) {
        this.cacheLocalThumbnail(itemId, frame.pngBytes, "image/png");
        return null;
      }

      const updated = await vaultApi.saveMediaThumbnail(
        itemId,
        frame.pngBytes,
        frame.width,
        frame.height,
        frame.durationMs
      );
      this.notifyThumbnailsUpdated([itemId]);
      return updated;
    };

    const queued = this.videoQueueTail.catch(() => {}).then(runTask);
    this.videoQueueTail = queued.then(
      () => undefined,
      () => undefined
    );

    const promise = queued
      .catch((err) => {
        if (this.epoch === currentEpoch && !isManualOrForce) {
          this.failedPendingThumbs.add(itemId);
        }
        throw err;
      })
      .finally(() => {
        if (this.inflightVideo.get(itemId) === promise) {
          this.inflightVideo.delete(itemId);
        }
      });

    if (!isManualOrForce) {
      this.inflightVideo.set(itemId, promise);
    }
    return promise;
  }

  async regenerateItemThumbnail(
    itemId: string,
    mimeType: string,
    timestampMs?: number
  ): Promise<ItemDetail> {
    if (mimeType.startsWith("video/")) {
      const updated = await this.processPendingVideoThumbnail(itemId, mimeType, {
        force: true,
        timestampMs,
        persist: true,
      });
      if (!updated) throw new Error("サムネイルの再生成に失敗しました");
      return updated;
    }
    if (mimeType === "image/avif") {
      const updated = await this.processPendingAvifThumbnail(itemId, {
        force: true,
        persist: true,
      });
      if (!updated) throw new Error("サムネイルの再生成に失敗しました");
      return updated;
    }
    const res = await vaultApi.regenerateItemThumbnail(itemId, null);
    if (res.status === "updated" && res.detail) {
      this.notifyThumbnailsUpdated([itemId]);
      return res.detail;
    }
    throw new Error("サムネイルの再生成に失敗しました");
  }

  async captureVideoThumbnailAt(
    itemId: string,
    videoEl: HTMLVideoElement,
    timestampMs: number
  ): Promise<ItemDetail> {
    const currentEpoch = this.epoch;
    const hostRes = await vaultApi.regenerateItemThumbnail(itemId, timestampMs);
    if (this.epoch !== currentEpoch) {
      throw new Error("Vault closed");
    }
    if (hostRes.status === "updated" && hostRes.detail) {
      this.notifyThumbnailsUpdated([itemId]);
      return hostRes.detail;
    }

    const origW = videoEl.videoWidth;
    const origH = videoEl.videoHeight;
    if (!origW || !origH) {
      throw new Error("動画の現在のフレームを読み取れませんでした");
    }
    const durationMs =
      Number.isFinite(videoEl.duration) && videoEl.duration > 0
        ? Math.round(videoEl.duration * 1000)
        : 0;
    const { sx, sy, sw, sh, tw, th } = computeSmartThumbnailRect(origW, origH);

    const canvas = document.createElement("canvas");
    canvas.width = tw;
    canvas.height = th;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (!ctx) {
      throw new Error("Canvas context unavailable");
    }
    ctx.drawImage(videoEl, sx, sy, sw, sh, 0, 0, tw, th);
    const quality = analyzeCanvasFrame(ctx, tw, th);
    if (quality.kind === "blank") {
      throw new Error("動画フレームを取得できませんでした");
    }
    if (quality.kind === "noise") {
      throw new Error(
        "動画フレームの取得結果が破損しています（砂嵐検出）。設定画面で ffmpeg を導入すると安定して生成できます"
      );
    }

    const pngBytes = await canvasToPngBuffer(canvas);
    if (this.epoch !== currentEpoch) {
      throw new Error("Vault closed");
    }
    const updated = await vaultApi.saveMediaThumbnail(
      itemId,
      pngBytes,
      origW,
      origH,
      durationMs
    );
    this.notifyThumbnailsUpdated([itemId]);
    return updated;
  }

  invalidateItems(itemIds: string[]): void {
    for (const id of itemIds) {
      this.thumbs.delete(id);
      this.fulls.delete(id);
      this.inflightThumbs.delete(id);
      this.inflightDisplay.delete(id);
      this.failedPendingThumbs.delete(id);
      this.knownHasThumb.delete(id);
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
    this.inflightDisplay.clear();
    this.inflightAvif.clear();
    this.inflightVideo.clear();
    this.failedPendingThumbs.clear();
    this.thumbVersions.clear();
    this.knownHasThumb.clear();
    for (const wake of this.avifWaiters.splice(0)) {
      wake();
    }
    this.videoQueueTail = Promise.resolve();
  }
}

export const imageMemoryCache = new ImageMemoryCache();
