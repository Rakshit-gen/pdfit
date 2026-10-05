import { describe, expect, it } from "vitest";
import { formatBytes, heaviestPage, pageList, parseLimit, percent, smallName } from "./format";

describe("parseLimit", () => {
  it("reads the ways people type a limit", () => {
    expect(parseLimit("2")).toBe(2_000_000);
    expect(parseLimit("1.5 MB")).toBe(1_500_000);
    expect(parseLimit("1,5mb")).toBe(1_500_000);
    expect(parseLimit("500 KB")).toBe(500_000);
    expect(parseLimit("300k")).toBe(300_000);
  });
  it("refuses nonsense and limits too small to hold a PDF", () => {
    expect(parseLimit("")).toBeNull();
    expect(parseLimit("two")).toBeNull();
    expect(parseLimit("2 GB")).toBeNull();
    expect(parseLimit("5 KB")).toBeNull();
  });
});

describe("formatting", () => {
  it("uses decimal megabytes", () => {
    expect(formatBytes(5_350_000)).toBe("5.35 MB");
    expect(formatBytes(2_000_000)).toBe("2 MB");
    expect(formatBytes(1_400_000)).toBe("1.40 MB");
    expect(formatBytes(494_806)).toBe("495 KB");
    expect(formatBytes(512)).toBe("512 bytes");
  });
  it("never rounds a small share down to nothing", () => {
    expect(percent(1, 1000)).toBe("<1%");
    expect(percent(0, 1000)).toBe("0%");
    expect(percent(283_446, 837_945)).toBe("34%");
  });
  it("names pages and files", () => {
    expect(pageList([3])).toBe("3");
    expect(pageList([1, 2, 5])).toBe("1-5");
    expect(smallName("Visa scan.PDF")).toBe("Visa scan.small.pdf");
    expect(smallName("notes")).toBe("notes.small.pdf");
  });
});

describe("heaviestPage", () => {
  it("picks the page with the most image bytes, counting shared images on each page", () => {
    const images = [
      { pages: [1], bytes: 300 },
      { pages: [2, 3], bytes: 200 },
      { pages: [3], bytes: 150 },
    ];
    expect(heaviestPage(images)).toBe(3);
    expect(heaviestPage([])).toBe(1);
  });
});
