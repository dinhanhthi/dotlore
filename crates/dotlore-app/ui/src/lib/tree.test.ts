import { describe, expect, it } from "vitest";

import { buildTree } from "./tree";

describe("buildTree", () => {
  it("nests a 3-level path, folders before files, alphabetically", () => {
    const tree = buildTree([
      "README.md",
      "docs/guide/intro.md",
      "docs/a.md",
      "CLAUDE.md",
      "docs/guide/setup.md",
    ]);

    expect(tree).toEqual([
      {
        name: "docs",
        path: "docs",
        kind: "folder",
        children: [
          {
            name: "guide",
            path: "docs/guide",
            kind: "folder",
            children: [
              {
                name: "intro.md",
                path: "docs/guide/intro.md",
                kind: "file",
                children: [],
              },
              {
                name: "setup.md",
                path: "docs/guide/setup.md",
                kind: "file",
                children: [],
              },
            ],
          },
          {
            name: "a.md",
            path: "docs/a.md",
            kind: "file",
            children: [],
          },
        ],
      },
      {
        name: "CLAUDE.md",
        path: "CLAUDE.md",
        kind: "file",
        children: [],
      },
      {
        name: "README.md",
        path: "README.md",
        kind: "file",
        children: [],
      },
    ]);
  });
});
