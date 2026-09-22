import { describe, expect, it } from "vitest";

import { MOCK_FILE_PATHS, open } from "./plugin-dialog";

describe("open", () => {
  it("always returns a folder, even when called as a file picker", async () => {
    await expect(open()).resolves.toBe("/Users/demo/Projects/new-root");
    await expect(open({ directory: false })).resolves.toBe(
      "/Users/demo/Projects/new-root",
    );
  });

  it("never returns a path listed as a file", async () => {
    const picked = await open({ directory: false });
    expect(typeof picked).toBe("string");
    expect(MOCK_FILE_PATHS.has(picked as string)).toBe(false);
  });
});
