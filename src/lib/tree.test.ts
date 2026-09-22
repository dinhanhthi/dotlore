import { describe, expect, it } from "vitest";

import type { TrackedFile } from "./types";
import { buildTree, nodeWeight, type TreeNode } from "./tree";

function tracked(
  rel: string,
  bytes = 0,
  state: TrackedFile["state"] = "Synced",
): TrackedFile {
  return { rel, bytes, state };
}

function fileNode(
  path: string,
  bytes: number,
  state: TrackedFile["state"] = "Synced",
): TreeNode {
  return {
    name: path.split("/").at(-1) ?? path,
    path,
    kind: "file",
    children: [],
    bytes,
    state,
  };
}

describe("buildTree", () => {
  it("nests a 3-level path, folders before files, alphabetically", () => {
    const tree = buildTree([
      tracked("README.md"),
      tracked("docs/guide/intro.md"),
      tracked("docs/a.md"),
      tracked("CLAUDE.md"),
      tracked("docs/guide/setup.md"),
    ]);

    expect(tree).toEqual([
      {
        name: "docs",
        path: "docs",
        kind: "folder",
        bytes: 0,
        state: "Synced",
        children: [
          {
            name: "guide",
            path: "docs/guide",
            kind: "folder",
            bytes: 0,
            state: "Synced",
            children: [
              {
                name: "intro.md",
                path: "docs/guide/intro.md",
                kind: "file",
                bytes: 0,
                state: "Synced",
                children: [],
              },
              {
                name: "setup.md",
                path: "docs/guide/setup.md",
                kind: "file",
                bytes: 0,
                state: "Synced",
                children: [],
              },
            ],
          },
          {
            name: "a.md",
            path: "docs/a.md",
            kind: "file",
            bytes: 0,
            state: "Synced",
            children: [],
          },
        ],
      },
      {
        name: "CLAUDE.md",
        path: "CLAUDE.md",
        kind: "file",
        bytes: 0,
        state: "Synced",
        children: [],
      },
      {
        name: "README.md",
        path: "README.md",
        kind: "file",
        bytes: 0,
        state: "Synced",
        children: [],
      },
    ]);
  });

  it("buildTree sums child sizes into folder nodes", () => {
    const tree = buildTree([
      tracked("docs/a.md", 10),
      tracked("docs/b.md", 20),
      tracked("docs/guide/c.md", 5),
    ]);

    const docs = tree.find((node) => node.path === "docs");
    const guide = docs?.children.find((node) => node.path === "docs/guide");
    expect(guide?.bytes).toBe(5);
    expect(docs?.bytes).toBe(35);
  });
});

describe("nodeWeight", () => {
  const limit = 100;

  it("nodeWeight marks a file one byte over the limit as danger and one exactly at the limit as warning", () => {
    expect(nodeWeight(fileNode("over.md", limit + 1), limit)).toBe("danger");
    expect(nodeWeight(fileNode("at.md", limit), limit)).toBe("warning");
  });

  it("nodeWeight marks a file at half the limit as warning", () => {
    expect(nodeWeight(fileNode("half.md", limit / 2), limit)).toBe("warning");
  });

  it("nodeWeight gives a folder the worst weight among its descendants", () => {
    const tree = buildTree([
      tracked("bundle/small.md", 10),
      tracked("bundle/ok.md", 20),
      tracked("bundle/huge.md", limit + 1),
    ]);
    const folder = tree.find((node) => node.path === "bundle");
    expect(folder).toBeDefined();
    expect(nodeWeight(folder!, limit)).toBe("danger");
  });
});
