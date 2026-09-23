import { describe, expect, it } from "vitest";

import {
  pickSiblingPath,
  quickResolveCopy,
  quickResolveItems,
} from "./quick-resolve";
import type { ConflictView, ResolutionDto, SiblingDto } from "@/lib/types";

const studio: ConflictView = {
  live: "notes/CLAUDE.md",
  sibling: "notes/CLAUDE.dotlore-conflict-1a2b3c4d.md",
  loserId8: "1a2b3c4d",
  loserName: "studio",
  loserIsMe: false,
};
const laptop: ConflictView = {
  live: "notes/CLAUDE.md",
  sibling: "notes\\CLAUDE.dotlore-conflict-5e6f7a8b.md",
  loserId8: "5e6f7a8b",
  loserName: "laptop",
  loserIsMe: false,
};

function sibling(path: string, device_name: string): SiblingDto {
  return { path, device_name, is_me: false, text: "", bytes_len: 0 };
}

const dto: ResolutionDto = {
  slug: "notes",
  live: "notes/CLAUDE.md",
  live_text: "",
  binary: false,
  live_bytes_len: 0,
  siblings: [
    sibling("notes/CLAUDE.dotlore-conflict-1a2b3c4d.md", "studio"),
    sibling("notes\\CLAUDE.dotlore-conflict-5e6f7a8b.md", "laptop"),
  ],
};

describe("quickResolveItems", () => {
  it("labels the live item as this machine", () => {
    expect(quickResolveItems([studio]).live.label).toBe("Keep this machine");
  });

  it("names the device inline for a single sibling", () => {
    expect(quickResolveItems([studio]).cloud).toEqual([
      {
        label: "Keep cloud (studio)",
        siblingRel: "notes/CLAUDE.dotlore-conflict-1a2b3c4d.md",
        device: "studio",
      },
    ]);
  });

  it("lists one submenu item per device for several siblings", () => {
    expect(quickResolveItems([studio, laptop]).cloud).toEqual([
      {
        label: "studio",
        siblingRel: "notes/CLAUDE.dotlore-conflict-1a2b3c4d.md",
        device: "studio",
      },
      {
        label: "laptop",
        siblingRel: "notes/CLAUDE.dotlore-conflict-5e6f7a8b.md",
        device: "laptop",
      },
    ]);
  });
});

describe("quickResolveCopy", () => {
  it("counts every discarded device version when keeping this machine", () => {
    expect(quickResolveCopy("live", "notes/CLAUDE.md", [studio, laptop])).toEqual({
      title: "Keep this machine's version of CLAUDE.md?",
      description: "Discards all 2 versions from other devices.",
      action: "Keep this machine",
    });
  });

  it("uses the singular for one discarded version", () => {
    expect(quickResolveCopy("live", "notes/CLAUDE.md", [studio]).description).toBe(
      "Discards the version from the other device.",
    );
  });

  it("names the kept device and warns other versions are discarded", () => {
    expect(
      quickResolveCopy("other", "notes/CLAUDE.md", [studio, laptop], "laptop"),
    ).toEqual({
      title: "Keep laptop's version of CLAUDE.md?",
      description: "This machine's version and any other device versions are discarded.",
      action: "Keep cloud",
    });
  });

  it("falls back to the only sibling's device", () => {
    const copy = quickResolveCopy("other", "notes\\CLAUDE.md", [studio]);
    expect(copy.title).toBe("Keep studio's version of CLAUDE.md?");
    expect(copy.description).toBe("This machine's version is discarded.");
  });

  it("never says Mac", () => {
    const texts = [
      quickResolveCopy("live", "a.md", [studio]),
      quickResolveCopy("live", "a.md", [studio, laptop]),
      quickResolveCopy("other", "a.md", [studio]),
      quickResolveCopy("other", "a.md", [studio, laptop], "laptop"),
    ].flatMap((copy) => Object.values(copy));
    for (const text of texts) expect(text).not.toMatch(/\bMac\b/);
  });
});

describe("pickSiblingPath", () => {
  it("returns the snapshot path for a matching sibling", () => {
    expect(pickSiblingPath(dto, "notes/CLAUDE.dotlore-conflict-1a2b3c4d.md")).toBe(
      "notes/CLAUDE.dotlore-conflict-1a2b3c4d.md",
    );
  });

  it("matches across path separators", () => {
    expect(pickSiblingPath(dto, "notes/CLAUDE.dotlore-conflict-5e6f7a8b.md")).toBe(
      "notes\\CLAUDE.dotlore-conflict-5e6f7a8b.md",
    );
  });

  it("returns null when the sibling is no longer in the snapshot", () => {
    expect(pickSiblingPath(dto, "notes/CLAUDE.dotlore-conflict-00000000.md")).toBeNull();
  });
});
