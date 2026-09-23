import { describe, expect, it } from "vitest";

import { composeLivePath, middleEllipsis } from "./path";

describe("composeLivePath", () => {
  it("joins a rel whose basename equals the root folder name", () => {
    expect(composeLivePath("/x/notes", "notes")).toBe("/x/notes/notes");
  });
});

describe("middleEllipsis", () => {
  it("leaves a path that already fits", () => {
    expect(middleEllipsis("/Users/thi/Documents", 46)).toBe("/Users/thi/Documents");
  });

  it("keeps both ends so the folder name survives", () => {
    const long =
      "/Users/thi/Library/CloudStorage/GoogleDrive-dinhanhthi@gmail.com/My Drive";
    const got = middleEllipsis(long, 46);
    expect(got).toHaveLength(46);
    expect(got.startsWith("/Users/thi/Library/")).toBe(true);
    expect(got.endsWith("/My Drive")).toBe(true);
    expect(got).toContain("…");
  });
});
