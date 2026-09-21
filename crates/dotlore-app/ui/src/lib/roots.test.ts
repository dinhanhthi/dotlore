import { describe, expect, it } from "vitest";

import { applyRootDiscovery, mergeRootsBySlug } from "./roots";
import type { LinkableRow, RootRow } from "./types";

function linkedRow(overrides: Partial<RootRow> = {}): RootRow {
  return {
    slug: "notes",
    path: "/Users/thi/notes",
    name: "notes",
    is_agent: false,
    linked: true,
    status: { kind: "Synced" },
    ...overrides,
  };
}

function cloudRow(overrides: Partial<LinkableRow> = {}): LinkableRow {
  return {
    slug: "old-mac-notes",
    display_name: "Old Mac Notes",
    is_agent: false,
    ...overrides,
  };
}

describe("mergeRootsBySlug", () => {
  it("deduplicates a slug that appears in both local and cloud lists", () => {
    const local = [linkedRow({ slug: "notes" })];
    const cloud = [cloudRow({ slug: "notes", display_name: "Cloud Notes" })];

    const merged = mergeRootsBySlug(local, cloud);

    expect(merged.map((row) => row.slug)).toEqual(["notes"]);
  });

  it("keeps the linked local row when a cloud row shares its slug", () => {
    const local = [
      linkedRow({
        slug: "claude",
        name: ".claude",
        path: "/Users/thi/.claude",
        is_agent: true,
        status: { kind: "Synced" },
      }),
    ];
    const cloud = [
      cloudRow({
        slug: "claude",
        display_name: "Claude from other Mac",
        is_agent: true,
      }),
    ];

    expect(mergeRootsBySlug(local, cloud)).toEqual(local);
  });
});

function unlinkedRow(overrides: Partial<RootRow> = {}): RootRow {
  return {
    slug: "old-mac-notes",
    path: "",
    name: "Old Mac Notes",
    is_agent: false,
    linked: false,
    status: { kind: "Pending" },
    ...overrides,
  };
}

describe("applyRootDiscovery", () => {
  it("rejects a response from a previous provider or refresh", () => {
    const applied = applyRootDiscovery({
      generation: 1,
      latestGeneration: 2,
      local: { ok: true, rows: [linkedRow()] },
      cloud: { ok: true, rows: [cloudRow()] },
      previous: [],
    });

    expect(applied).toBeNull();
  });

  it("retains previous unlinked rows and linked local rows when the cloud list fails", () => {
    const local = [linkedRow({ slug: "notes" })];
    const previous = [linkedRow({ slug: "notes" }), unlinkedRow()];

    const applied = applyRootDiscovery({
      generation: 1,
      latestGeneration: 1,
      local: { ok: true, rows: local },
      cloud: { ok: false, message: "Could not list cloud projects" },
      previous,
    });

    expect(applied).toEqual({
      roots: previous,
      discoveryError: "Could not list cloud projects",
    });
  });

  it("retains previous linked rows when the local list fails", () => {
    const previous = [linkedRow({ slug: "notes" }), unlinkedRow()];

    const applied = applyRootDiscovery({
      generation: 1,
      latestGeneration: 1,
      local: { ok: false, message: "Could not list configured projects" },
      cloud: { ok: true, rows: [cloudRow(), cloudRow({ slug: "fresh" })] },
      previous,
    });

    expect(applied?.roots.map((row) => row.slug)).toEqual([
      "notes",
      "old-mac-notes",
      "fresh",
    ]);
    expect(applied?.roots.find((row) => row.slug === "notes")?.linked).toBe(true);
    expect(applied?.discoveryError).toBe("Could not list configured projects");
  });
});
