import { describe, expect, it } from "vitest";

import type { EntryView } from "@/lib/types";

import {
  hasTrackedInside,
  isShownTracked,
  orderedPendingOps,
  stagePending,
  stagedTrackTotal,
  type PendingMap,
} from "./picker";

const docs: EntryView = { key: "docs/", kind: "directory", covering: [] };
const claude: EntryView = { key: "CLAUDE.md", kind: "file", covering: [] };
const synced = new Set(["CLAUDE.md", "docs/architecture.md", "docs/video.bin"]);

describe("isShownTracked", () => {
  it("shows a synced file as tracked and a hole-punched file as not", () => {
    expect(isShownTracked("CLAUDE.md", "file", [docs, claude], {}, synced)).toBe(
      true,
    );
    expect(
      isShownTracked("docs/secret.md", "file", [docs, claude], {}, synced),
    ).toBe(false);
  });

  it("shows a folder that still contains a synced file as tracked", () => {
    expect(isShownTracked("docs", "directory", [docs], {}, synced)).toBe(true);
    expect(isShownTracked("docs/notes", "directory", [docs], {}, synced)).toBe(
      false,
    );
  });

  it("lets a staged parent track or untrack cover its children", () => {
    const trackParent: PendingMap = { "docs/": "track" };
    expect(
      isShownTracked("docs/secret.md", "file", [], trackParent, new Set()),
    ).toBe(true);
    const untrackParent: PendingMap = { "docs/": "untrack" };
    expect(
      isShownTracked("docs/architecture.md", "file", [docs], untrackParent, synced),
    ).toBe(false);
    expect(
      isShownTracked("docs/readme.md", "file", [docs, {
        key: "docs/readme.md",
        kind: "file",
        covering: ["docs/"],
      }], untrackParent, synced),
    ).toBe(true);
  });
});

describe("hasTrackedInside", () => {
  const settings: EntryView = {
    key: ".claude/settings.json",
    kind: "file",
    covering: [],
  };
  const files = new Set([".claude/settings.json"]);

  it("marks a folder holding a tracked file without tracking the folder", () => {
    expect(isShownTracked(".claude", "directory", [settings], {}, files)).toBe(
      false,
    );
    expect(hasTrackedInside(".claude", [settings], {}, files)).toBe(true);
    expect(hasTrackedInside("docs", [settings], {}, files)).toBe(false);
  });

  it("follows staged marks inside the folder", () => {
    expect(
      hasTrackedInside(".claude", [settings], { ".claude/settings.json": "untrack" }, files),
    ).toBe(false);
    expect(
      hasTrackedInside("docs", [], { "docs/notes/": "track" }, new Set()),
    ).toBe(true);
  });
});

describe("stagePending", () => {
  it("drops a mark that returns the row to its current state", () => {
    const untracked = stagePending({}, "CLAUDE.md", "file", "untrack", [claude], synced);
    expect(untracked).toEqual({ "CLAUDE.md": "untrack" });
    expect(
      stagePending(untracked, "CLAUDE.md", "file", "track", [claude], synced),
    ).toEqual({});
  });
});

describe("orderedPendingOps", () => {
  it("tracks a parent before untracking a path inside it", () => {
    expect(
      orderedPendingOps({
        "docs/": "track",
        "docs/secret.md": "untrack",
      }).map((op) => `${op.action} ${op.rel}`),
    ).toEqual(["track docs", "untrack docs/secret.md"]);
  });

  it("untracks a parent before tracking a path inside it", () => {
    expect(
      orderedPendingOps({
        "docs/": "untrack",
        "docs/readme.md": "track",
      }).map((op) => `${op.action} ${op.rel}`),
    ).toEqual(["untrack docs", "track docs/readme.md"]);
  });
});

describe("stagedTrackTotal", () => {
  it("sums staged tracks and counts a folder instead of the tracks inside it", () => {
    expect(
      stagedTrackTotal(
        {
          "docs/": "track",
          "docs/a.md": "track",
          "docs/sub/": "track",
          "notes.md": "track",
          "old.md": "untrack",
        },
        {
          "docs/": 100,
          "docs/a.md": 40,
          "docs/sub/": 30,
          "notes.md": 5,
          "old.md": 9,
        },
      ),
    ).toEqual({ bytes: 105, complete: true });
  });

  it("counts a track inside a folder that is only staged to untrack", () => {
    expect(
      stagedTrackTotal(
        { "docs/": "untrack", "docs/readme.md": "track" },
        { "docs/": 100, "docs/readme.md": 40 },
      ),
    ).toEqual({ bytes: 40, complete: true });
  });

  it("stays incomplete until every included track has a size", () => {
    expect(
      stagedTrackTotal({ "docs/": "track", "docs/a.md": "track" }, { "docs/a.md": 40 }),
    ).toEqual({ bytes: 0, complete: false });
  });

  it("skips a measurement that failed", () => {
    expect(
      stagedTrackTotal(
        { "a.md": "track", "b.md": "track" },
        { "a.md": 4, "b.md": null },
      ),
    ).toEqual({ bytes: 4, complete: true });
  });

  it("is zero when nothing is staged to track", () => {
    expect(stagedTrackTotal({}, {})).toEqual({ bytes: 0, complete: true });
  });
});
