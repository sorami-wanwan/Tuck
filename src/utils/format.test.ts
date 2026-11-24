import { describe, expect, it } from "vitest";
import { formatBytes, formatProgressBytes } from "./format";

describe("formatBytes", () => {
  it("formats B, KB, MB, and GB ranges", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(1024 * 1024)).toBe("1.00 MB");
    expect(formatBytes(2 * 1024 * 1024 * 1024)).toBe("2.00 GB");
  });
});

describe("formatProgressBytes", () => {
  it("formats progress bytes with 0 MB fallback and MB/GB units", () => {
    expect(formatProgressBytes(0)).toBe("0 MB");
    expect(formatProgressBytes(-100)).toBe("0 MB");
    expect(formatProgressBytes(Number.NaN)).toBe("0 MB");
    expect(formatProgressBytes(5.5 * 1024 * 1024)).toBe("5.5 MB");
    expect(formatProgressBytes(1.5 * 1024 * 1024 * 1024)).toBe("1.50 GB");
  });
});
