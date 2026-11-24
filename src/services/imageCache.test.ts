// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const fetchThumbnailBytes = vi.fn<(itemId: string) => Promise<ArrayBuffer>>();
const fetchImageBytes = vi.fn<(itemId: string) => Promise<ArrayBuffer>>();

vi.mock("./vaultApi", () => ({
  vaultApi: {
    fetchThumbnailBytes: (itemId: string) => fetchThumbnailBytes(itemId),
    fetchImageBytes: (itemId: string) => fetchImageBytes(itemId),
    saveMediaThumbnail: vi.fn(),
    getItemDetail: vi.fn(),
  },
}));

const isTauriRuntimeMock = vi.fn<() => boolean>(() => false);
vi.mock("./runtime", () => ({
  isTauriRuntime: () => isTauriRuntimeMock(),
}));

import { imageMemoryCache } from "./imageCache";

describe("imageMemoryCache", () => {
  let nextBlobId = 0;
  let createSpy: ReturnType<typeof vi.spyOn>;
  let revokeSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    nextBlobId = 0;
    fetchThumbnailBytes.mockReset();
    fetchImageBytes.mockReset();
    isTauriRuntimeMock.mockReturnValue(false);
    imageMemoryCache.revokeAll();
    createSpy = vi
      .spyOn(URL, "createObjectURL")
      .mockImplementation(() => `blob:test-${++nextBlobId}`);
    revokeSpy = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
  });

  afterEach(() => {
    imageMemoryCache.revokeAll();
    createSpy.mockRestore();
    revokeSpy.mockRestore();
  });

  it("caches thumbnail URLs, deduplicates in-flight fetches, and revokes on invalidate/clear", async () => {
    fetchThumbnailBytes.mockResolvedValue(new ArrayBuffer(8));

    const [u1, u2] = await Promise.all([
      imageMemoryCache.getThumbnailUrl("item-1"),
      imageMemoryCache.getThumbnailUrl("item-1"),
    ]);

    expect(u1).toBe("blob:test-1");
    expect(u2).toBe("blob:test-1");
    expect(fetchThumbnailBytes).toHaveBeenCalledTimes(1);
    expect(imageMemoryCache.getCachedThumbnailUrl("item-1")).toBe("blob:test-1");

    imageMemoryCache.invalidateItems(["item-1"]);
    expect(revokeSpy).toHaveBeenCalledWith("blob:test-1");
    expect(imageMemoryCache.getCachedThumbnailUrl("item-1")).toBeUndefined();
  });

  it("rejects late in-flight resolutions after clearAll bumps epoch", async () => {
    let resolveFetch!: (buf: ArrayBuffer) => void;
    fetchThumbnailBytes.mockImplementation(
      () =>
        new Promise<ArrayBuffer>((resolve) => {
          resolveFetch = resolve;
        })
    );

    const pending = imageMemoryCache.getThumbnailUrl("stale-item");
    imageMemoryCache.clearAll();
    resolveFetch(new ArrayBuffer(4));

    await expect(pending).rejects.toThrow("Vault closed");
    expect(imageMemoryCache.getCachedThumbnailUrl("stale-item")).toBeUndefined();
  });

  it("evicts least recently used full image when exceeding FULL_CACHE_LIMIT (12)", async () => {
    fetchImageBytes.mockImplementation(async () => new ArrayBuffer(4));

    for (let i = 1; i <= 12; i += 1) {
      await imageMemoryCache.getFullImageUrl(`full-${i}`, "image/png");
    }
    // Touch full-1 so full-2 becomes the LRU entry
    await imageMemoryCache.getFullImageUrl("full-1", "image/png");

    await imageMemoryCache.getFullImageUrl("full-13", "image/png");
    expect(revokeSpy).toHaveBeenCalledWith("blob:test-2");
    expect(imageMemoryCache.getCachedFullImageUrl("full-1")).toBe("blob:test-1");
    expect(imageMemoryCache.getCachedFullImageUrl("full-2")).toBeUndefined();
    expect(imageMemoryCache.getCachedFullImageUrl("full-13")).toBe("blob:test-13");
  });

  it("preserves LAN media token on clearAll and clears it on revokeAll", () => {
    imageMemoryCache.setLanMediaToken("secret-token");
    const urlBefore = imageMemoryCache.getVaultMediaStreamUrl("item 1");
    expect(urlBefore).toMatch(/^\/api\/media\/item%201\?e=\d+&mt=secret-token$/);

    imageMemoryCache.clearAll();
    const urlAfterClear = imageMemoryCache.getVaultMediaStreamUrl("item 1");
    expect(urlAfterClear).not.toBe(urlBefore);
    expect(urlAfterClear).toContain("&mt=secret-token");

    imageMemoryCache.revokeAll();
    const urlAfterRevoke = imageMemoryCache.getVaultMediaStreamUrl("item 1");
    expect(urlAfterRevoke).not.toContain("mt=");
  });
});
