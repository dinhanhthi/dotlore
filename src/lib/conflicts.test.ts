import { describe, expect, it } from "vitest";

import {
  cloudDevices,
  hasUnnamedCloud,
  keepAllFiles,
  viewsInScope,
} from "./conflicts";
import type { ConflictView } from "./types";

function view(overrides: Partial<ConflictView> & { live: string }): ConflictView {
  return {
    sibling: `${overrides.live}.sib`,
    loserId8: "11111111",
    loserName: "studio",
    loserIsMe: false,
    ...overrides,
  };
}

const mine = view({
  live: "notes/mine.md",
  sibling: "notes/mine.me.md",
  loserId8: "aaaaaaaa",
  loserName: "this-mac",
  loserIsMe: true,
});
const studio = view({
  live: "notes/studio.md",
  sibling: "notes/studio.studio.md",
  loserId8: "11111111",
  loserName: "studio",
});
const studioAgain = view({
  live: "notes/studio.md",
  sibling: "notes/studio.studio-2.md",
  loserId8: "11111111",
  loserName: "studio",
});
const laptopOnly = view({
  live: "notes/laptop.md",
  sibling: "notes/laptop.laptop.md",
  loserId8: "22222222",
  loserName: "laptop",
});
const sharedStudio = view({
  live: "notes/shared.md",
  sibling: "notes/shared.studio.md",
  loserId8: "11111111",
  loserName: "studio",
});
const sharedLaptop = view({
  live: "notes/shared.md",
  sibling: "notes/shared.laptop.md",
  loserId8: "22222222",
  loserName: "laptop",
});
const mixedMine = view({
  live: "notes/mixed.md",
  sibling: "notes/mixed.me.md",
  loserId8: "aaaaaaaa",
  loserName: "this-mac",
  loserIsMe: true,
});
const mixedStudio = view({
  live: "notes/mixed.md",
  sibling: "notes/mixed.studio.md",
  loserId8: "11111111",
  loserName: "studio",
});

describe("viewsInScope", () => {
  it("matches a file by its exact slashed live path", () => {
    const file = view({ live: "notes\\a.md" });
    const near = view({ live: "notes/a.md.bak" });
    expect(viewsInScope([file, near], { kind: "file", path: "notes/a.md" })).toEqual([
      file,
    ]);
  });

  it("does not treat a sibling folder name as inside the folder", () => {
    const exact = view({ live: "notes" });
    const child = view({ live: "notes/a.md" });
    const extra = view({ live: "notes-extra/a.md" });
    expect(viewsInScope([exact, child, extra], { kind: "folder", path: "notes" })).toEqual([
      exact,
      child,
    ]);
  });

  it("returns every view for a root", () => {
    const notes = view({ live: "notes/a.md" });
    const extra = view({ live: "notes-extra/a.md" });
    expect(viewsInScope([notes, extra], { kind: "root" })).toEqual([notes, extra]);
  });
});

describe("cloudDevices", () => {
  it("never lists a loserIsMe view", () => {
    expect(cloudDevices([mine])).toEqual([]);
  });

  it("collapses two siblings of one device and keeps the first sibling path", () => {
    expect(cloudDevices([studio, studioAgain, laptopOnly])).toEqual([
      {
        id8: "11111111",
        name: "studio",
        siblingRel: "notes/studio.studio.md",
      },
      {
        id8: "22222222",
        name: "laptop",
        siblingRel: "notes/laptop.laptop.md",
      },
    ]);
  });
});

describe("hasUnnamedCloud", () => {
  it("is true when a live path has only loserIsMe views", () => {
    expect(hasUnnamedCloud([mine])).toBe(true);
    expect(hasUnnamedCloud([studio])).toBe(false);
    expect(hasUnnamedCloud([mine, studio])).toBe(true);
  });
});

describe("keepAllFiles", () => {
  it("keeps the loserIsMe sibling when keeping this machine", () => {
    expect(
      keepAllFiles([studio, mixedStudio, mixedMine, mine], { kind: "root" }, "live"),
    ).toEqual([
      { rel: "notes/studio.md", keep: "live" },
      { rel: "notes/mixed.md", keep: "other", siblingRel: "notes/mixed.me.md" },
      { rel: "notes/mine.md", keep: "other", siblingRel: "notes/mine.me.md" },
    ]);
  });

  it("keeps live for the unnamed bucket", () => {
    expect(keepAllFiles([mine, studio], { kind: "root" }, "unnamed")).toEqual([
      { rel: "notes/mine.md", keep: "live" },
    ]);
  });

  it("puts a mixed file in the named device batch and not the unnamed bucket", () => {
    const views = [mixedMine, mixedStudio];
    expect(hasUnnamedCloud(views)).toBe(false);
    expect(keepAllFiles(views, { kind: "root" }, "unnamed")).toEqual([]);
    expect(keepAllFiles(views, { kind: "root" }, { deviceId: "11111111" })).toEqual([
      {
        rel: "notes/mixed.md",
        keep: "other",
        siblingRel: "notes/mixed.studio.md",
      },
    ]);
  });

  it("omits a laptop-only file from a studio batch and returns one entry when both devices share a file", () => {
    expect(
      keepAllFiles(
        [studio, studioAgain, laptopOnly, sharedStudio, sharedLaptop],
        { kind: "root" },
        { deviceId: "11111111" },
      ),
    ).toEqual([
      {
        rel: "notes/studio.md",
        keep: "other",
        siblingRel: "notes/studio.studio.md",
      },
      {
        rel: "notes/shared.md",
        keep: "other",
        siblingRel: "notes/shared.studio.md",
      },
    ]);
  });

  it("applies the scope before choosing files", () => {
    expect(
      keepAllFiles([mine, studio], { kind: "file", path: "notes/studio.md" }, "live"),
    ).toEqual([{ rel: "notes/studio.md", keep: "live" }]);
  });
});
