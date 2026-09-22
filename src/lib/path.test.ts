import { describe, expect, it } from "vitest";

import { composeLivePath } from "./path";

describe("composeLivePath", () => {
  it("joins a rel whose basename equals the root folder name", () => {
    expect(composeLivePath("/x/notes", "notes")).toBe("/x/notes/notes");
  });
});
