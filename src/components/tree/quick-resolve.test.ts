import { afterEach, describe, expect, it, vi } from "vitest";

import { BLOCKED } from "@/lib/ipc";
import type { ConflictView, ResolutionDto, SiblingDto } from "@/lib/types";

import {
  applyKeepAll,
  pickSiblingPath,
  quickResolveCopy,
  quickResolveItems,
  quickResolveTargetCopy,
  resetKeepAllBusyForTest,
  type KeepAllOps,
} from "./quick-resolve";

const studio: ConflictView = {
  live: "notes/CLAUDE.md",
  sibling: "notes/CLAUDE.dotlore-conflict-1a2b3c4d.md",
  loserId8: "1a2b3c4d",
  loserName: "studio",
  loserIsMe: false,
};
const studioAgain: ConflictView = {
  ...studio,
  sibling: "notes/CLAUDE.dotlore-conflict-1a2b3c4d-2.md",
};
const studioTwin: ConflictView = {
  live: "notes/CLAUDE.md",
  sibling: "notes/CLAUDE.dotlore-conflict-bbbbbbbb.md",
  loserId8: "bbbbbbbb",
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
const mine: ConflictView = {
  live: "notes/LOCAL.md",
  sibling: "notes/LOCAL.dotlore-conflict-aaaaaaaa.md",
  loserId8: "aaaaaaaa",
  loserName: "this-machine",
  loserIsMe: true,
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
    expect(quickResolveItems([studio]).live.label).toBe("Keep all from this machine");
  });

  it("labels a single cloud device as keep all from cloud", () => {
    expect(quickResolveItems([studio, studioAgain])).toEqual({
      live: { label: "Keep all from this machine" },
      cloudMenuLabel: "Keep all from cloud",
      cloud: [
        {
          label: "Keep all from cloud",
          choice: { deviceId: "1a2b3c4d" },
          device: "studio",
        },
      ],
    });
  });

  it("lists one submenu item per cloud device", () => {
    expect(quickResolveItems([studio, laptop])).toEqual({
      live: { label: "Keep all from this machine" },
      cloudMenuLabel: "Keep all from cloud",
      cloud: [
        {
          label: "studio",
          choice: { deviceId: "1a2b3c4d" },
          device: "studio",
        },
        {
          label: "laptop",
          choice: { deviceId: "5e6f7a8b" },
          device: "laptop",
        },
      ],
    });
  });

  it("appends the id only on cloud items whose names collide", () => {
    expect(quickResolveItems([studio, studioTwin, laptop]).cloud).toEqual([
      {
        label: "studio (1a2b3c4d)",
        choice: { deviceId: "1a2b3c4d" },
        device: "studio",
      },
      {
        label: "studio (bbbbbbbb)",
        choice: { deviceId: "bbbbbbbb" },
        device: "studio",
      },
      {
        label: "laptop",
        choice: { deviceId: "5e6f7a8b" },
        device: "laptop",
      },
    ]);
  });

  it("never lists this machine as a cloud item", () => {
    const mixed = { ...mine, live: studio.live };
    expect(quickResolveItems([mixed, studio]).cloud).toEqual([
      {
        label: "Keep all from cloud",
        choice: { deviceId: "1a2b3c4d" },
        device: "studio",
      },
    ]);
  });

  it("uses the unnamed bucket when a file has only this machine's lost copy", () => {
    expect(quickResolveItems([mine]).cloud).toEqual([
      {
        label: "Keep all from cloud",
        choice: "unnamed",
        device: "cloud",
      },
    ]);
  });

  it("labels the unnamed bucket Noname cloud beside a named device", () => {
    expect(quickResolveItems([studio, mine]).cloud).toEqual([
      {
        label: "studio",
        choice: { deviceId: "1a2b3c4d" },
        device: "studio",
      },
      {
        label: "Noname cloud",
        choice: "unnamed",
        device: "cloud",
      },
    ]);
  });
});

describe("quickResolveCopy", () => {
  it("counts every discarded device version when keeping this machine", () => {
    expect(
      quickResolveCopy({
        keep: "live",
        rel: "notes/CLAUDE.md",
        views: [studio, laptop],
        batchSize: 1,
        scopeCount: 1,
      }),
    ).toEqual({
      title: "Keep all from this machine for CLAUDE.md?",
      description: "Discards all 2 versions from other devices.",
      action: "Keep all from this machine",
    });
  });

  it("uses the singular for one discarded version", () => {
    expect(
      quickResolveCopy({
        keep: "live",
        rel: "notes/CLAUDE.md",
        views: [studio],
        batchSize: 1,
        scopeCount: 1,
      }),
    ).toEqual({
      title: "Keep all from this machine for CLAUDE.md?",
      description: "Discards the version from the other device.",
      action: "Keep all from this machine",
    });
  });

  it("counts the live winner and the named sibling when this machine lost", () => {
    expect(
      quickResolveCopy({
        keep: "live",
        rel: "notes/CLAUDE.md",
        views: [{ ...mine, live: studio.live }, studio],
        batchSize: 1,
        scopeCount: 1,
      }).description,
    ).toBe("Discards all 2 versions from other devices.");
  });

  it("counts only the live winner when this machine lost and no named device remains", () => {
    expect(
      quickResolveCopy({
        keep: "live",
        rel: "notes/LOCAL.md",
        views: [mine],
        batchSize: 1,
        scopeCount: 1,
      }).description,
    ).toBe("Discards the version from the other device.");
  });

  it("names the kept device and warns other versions of this file are discarded", () => {
    expect(
      quickResolveCopy({
        keep: "other",
        rel: "notes/CLAUDE.md",
        views: [studio, laptop],
        device: "laptop",
        batchSize: 1,
        scopeCount: 1,
      }),
    ).toEqual({
      title: "Keep all from laptop for CLAUDE.md?",
      description:
        "This machine's version and any other device versions of this file are discarded.",
      action: "Keep all from cloud",
    });
  });

  it("falls back to the only sibling's device", () => {
    const copy = quickResolveCopy({
      keep: "other",
      rel: "notes\\CLAUDE.md",
      views: [studio],
      batchSize: 1,
      scopeCount: 1,
    });
    expect(copy.title).toBe("Keep all from studio for CLAUDE.md?");
    expect(copy.description).toBe("This machine's version is discarded.");
    expect(copy.action).toBe("Keep all from cloud");
  });

  it("does not treat two siblings of one device as other devices", () => {
    expect(
      quickResolveCopy({
        keep: "other",
        rel: "notes/CLAUDE.md",
        views: [studio, studioAgain],
        device: "studio",
        batchSize: 1,
        scopeCount: 1,
      }).description,
    ).toBe("This machine's version is discarded.");
  });

  it("describes a multi-file keep from this machine", () => {
    expect(
      quickResolveCopy({
        keep: "live",
        batchSize: 3,
        scopeCount: 3,
      }),
    ).toEqual({
      title: "Keep all from this machine for 3 files?",
      description: "Discards every other device's version of these 3 files.",
      action: "Keep all from this machine",
    });
  });

  it("describes a partial multi-file cloud keep", () => {
    expect(
      quickResolveCopy({
        keep: "other",
        device: "studio",
        batchSize: 2,
        scopeCount: 5,
      }),
    ).toEqual({
      title: "Keep all from studio for 2 files?",
      description:
        "Resolves 2 files that have a version from studio. Each of those files is fully resolved, so this machine's version and every other device's version of that file are discarded. The other 3 conflicted files stay unresolved.",
      action: "Keep all from cloud",
    });
  });

  it("describes a cloud keep that covers every conflicted file", () => {
    expect(
      quickResolveCopy({
        keep: "other",
        device: "studio",
        batchSize: 4,
        scopeCount: 4,
      }),
    ).toEqual({
      title: "Keep all from studio for 4 files?",
      description:
        "Resolves all 4 conflicted files. Each file is fully resolved: this machine's version and every other device's version of that file are discarded.",
      action: "Keep all from cloud",
    });
  });

  it("uses cloud for the unnamed bucket", () => {
    const one = quickResolveCopy({
      keep: "other",
      rel: "notes/LOCAL.md",
      device: "cloud",
      views: [mine],
      batchSize: 1,
      scopeCount: 1,
    });
    expect(one).toEqual({
      title: "Keep all from cloud for LOCAL.md?",
      description: "This machine's version is discarded.",
      action: "Keep all from cloud",
    });

    const partial = quickResolveCopy({
      keep: "other",
      device: "cloud",
      batchSize: 2,
      scopeCount: 5,
    });
    expect(partial.title).toBe("Keep all from cloud for 2 files?");
    expect(partial.description).toBe(
      "Resolves 2 files that have a version from cloud. Each of those files is fully resolved, so this machine's version and every other device's version of that file are discarded. The other 3 conflicted files stay unresolved.",
    );
  });

  it("never says Mac", () => {
    const texts = [
      quickResolveCopy({
        keep: "live",
        rel: "a.md",
        views: [studio],
        batchSize: 1,
        scopeCount: 1,
      }),
      quickResolveCopy({
        keep: "live",
        rel: "a.md",
        views: [studio, laptop],
        batchSize: 1,
        scopeCount: 1,
      }),
      quickResolveCopy({
        keep: "other",
        rel: "a.md",
        views: [studio],
        batchSize: 1,
        scopeCount: 1,
      }),
      quickResolveCopy({
        keep: "other",
        rel: "a.md",
        views: [studio, laptop],
        device: "laptop",
        batchSize: 1,
        scopeCount: 1,
      }),
      quickResolveCopy({
        keep: "other",
        rel: "a.md",
        views: [mine],
        device: "cloud",
        batchSize: 1,
        scopeCount: 1,
      }),
      quickResolveCopy({ keep: "live", batchSize: 2, scopeCount: 2 }),
      quickResolveCopy({
        keep: "other",
        device: "studio",
        batchSize: 2,
        scopeCount: 4,
      }),
      quickResolveCopy({
        keep: "other",
        device: "studio",
        batchSize: 2,
        scopeCount: 2,
      }),
    ].flatMap((copy) => Object.values(copy));
    for (const text of texts) expect(text).not.toMatch(/\bMac\b/);
  });
});

const STALE = "The file changed on another device. Review and try again.";
const PENDING = "Sync has not finished yet. Try again in a moment.";
const MISSING = "That version is no longer available. Refresh and try again.";

function resolution(live: string, siblings: SiblingDto[] = []): ResolutionDto {
  return {
    slug: "notes",
    live,
    live_text: "",
    binary: false,
    live_bytes_len: 0,
    siblings,
  };
}

type ResolveReply =
  | { outcome: "applied" }
  | { outcome: "stale"; refreshed: ResolutionDto }
  | { outcome: "pending" }
  | typeof BLOCKED;

function trackOps(options?: {
  siblings?: (rel: string) => SiblingDto[];
  onOpen?: (rel: string) => Promise<ResolutionDto>;
  onResolve?: (rel: string) => ResolveReply;
}): { steps: string[] } & KeepAllOps {
  const steps: string[] = [];
  const openResolution = vi.fn(async (_slug: string, rel: string) => {
    steps.push(`open ${rel}`);
    if (options?.onOpen) return options.onOpen(rel);
    return resolution(rel, options?.siblings?.(rel) ?? []);
  });
  const resolveBinary = vi.fn(
    async (
      _slug: string,
      rel: string,
      keep: "live" | "other",
      sibling?: string | null,
    ) => {
      steps.push(
        sibling === undefined || sibling === null
          ? `resolve ${rel} ${keep}`
          : `resolve ${rel} ${keep} ${sibling}`,
      );
      const result = options?.onResolve?.(rel);
      if (result === undefined) return { outcome: "applied" as const };
      return result;
    },
  );
  const closeResolution = vi.fn(async (_slug: string, rel: string) => {
    steps.push(`close ${rel}`);
  });
  return { steps, openResolution, resolveBinary, closeResolution };
}

describe("quickResolveTargetCopy", () => {
  it("keeps the one-file this-machine copy when that file resolves as other", () => {
    expect(
      quickResolveTargetCopy({
        choice: "live",
        scopeCount: 1,
        files: [
          {
            rel: "notes/LOCAL.md",
            keep: "other",
            siblingRel: mine.sibling,
            views: [mine],
          },
        ],
      }),
    ).toEqual({
      title: "Keep all from this machine for LOCAL.md?",
      description: "Discards the version from the other device.",
      action: "Keep all from this machine",
    });
  });

  it("keeps the cloud copy when an unnamed file resolves as live", () => {
    expect(
      quickResolveTargetCopy({
        choice: "unnamed",
        device: "cloud",
        scopeCount: 1,
        files: [{ rel: "notes/LOCAL.md", keep: "live", views: [mine] }],
      }),
    ).toEqual({
      title: "Keep all from cloud for LOCAL.md?",
      description: "This machine's version is discarded.",
      action: "Keep all from cloud",
    });
  });

  it("passes the file count and the scope count into a batch copy", () => {
    expect(
      quickResolveTargetCopy({
        choice: { deviceId: "1a2b3c4d" },
        device: "studio",
        scopeCount: 5,
        files: [
          { rel: "a.md", keep: "other", siblingRel: "a.sib", views: [studio] },
          { rel: "b.md", keep: "other", siblingRel: "b.sib", views: [studio] },
        ],
      }),
    ).toEqual({
      title: "Keep all from studio for 2 files?",
      description:
        "Resolves 2 files that have a version from studio. Each of those files is fully resolved, so this machine's version and every other device's version of that file are discarded. The other 3 conflicted files stay unresolved.",
      action: "Keep all from cloud",
    });
  });
});

describe("applyKeepAll", () => {
  afterEach(() => {
    resetKeepAllBusyForTest();
  });

  it("opens, resolves, and closes one file at a time and stops on a stale file", async () => {
    const { steps, resolveBinary, closeResolution, ...rest } = trackOps({
      onResolve: (rel) =>
        rel === "b.md"
          ? { outcome: "stale", refreshed: resolution("b.md") }
          : { outcome: "applied" },
    });
    const result = await applyKeepAll(
      "notes",
      [
        { rel: "a.md", keep: "live" },
        { rel: "b.md", keep: "live" },
        { rel: "c.md", keep: "live" },
      ],
      { resolveBinary, closeResolution, ...rest },
    );

    expect(steps).toEqual([
      "open a.md",
      "resolve a.md live",
      "close a.md",
      "open b.md",
      "resolve b.md live",
      "close b.md",
    ]);
    expect(resolveBinary).toHaveBeenNthCalledWith(1, "notes", "a.md", "live");
    expect(closeResolution).toHaveBeenCalledWith("notes", "a.md");
    expect(result).toEqual({
      status: "stopped",
      applied: ["a.md"],
      failedRel: "b.md",
      message: `${STALE} b.md`,
    });
  });

  it("stops on pending and names the file", async () => {
    const { steps, ...ops } = trackOps({
      onResolve: (rel) => (rel === "b.md" ? { outcome: "pending" } : { outcome: "applied" }),
    });
    const result = await applyKeepAll(
      "notes",
      [
        { rel: "a.md", keep: "live" },
        { rel: "b.md", keep: "live" },
        { rel: "c.md", keep: "live" },
      ],
      ops,
    );
    expect(steps).toEqual([
      "open a.md",
      "resolve a.md live",
      "close a.md",
      "open b.md",
      "resolve b.md live",
      "close b.md",
    ]);
    expect(result).toEqual({
      status: "stopped",
      applied: ["a.md"],
      failedRel: "b.md",
      message: `${PENDING} b.md`,
    });
  });

  it("stops when the sibling is missing and does not resolve that file", async () => {
    const { steps, resolveBinary, ...rest } = trackOps();
    const result = await applyKeepAll(
      "notes",
      [
        { rel: "a.md", keep: "live" },
        { rel: "b.md", keep: "other", siblingRel: "notes/b.conflict.md" },
        { rel: "c.md", keep: "live" },
      ],
      { resolveBinary, ...rest },
    );
    expect(resolveBinary).toHaveBeenCalledTimes(1);
    expect(steps).toEqual([
      "open a.md",
      "resolve a.md live",
      "close a.md",
      "open b.md",
      "close b.md",
    ]);
    expect(result).toEqual({
      status: "stopped",
      applied: ["a.md"],
      failedRel: "b.md",
      message: `${MISSING} b.md`,
    });
  });

  it("resolves a loserIsMe file as other with the snapshot sibling", async () => {
    const siblingRel = "notes/LOCAL.dotlore-conflict-aaaaaaaa.md";
    const snapshotPath = "notes\\LOCAL.dotlore-conflict-aaaaaaaa.md";
    const { resolveBinary, ...rest } = trackOps({
      siblings: () => [sibling(snapshotPath, "this-machine")],
    });
    const result = await applyKeepAll(
      "notes",
      [{ rel: "notes/LOCAL.md", keep: "other", siblingRel }],
      { resolveBinary, ...rest },
    );
    expect(result).toEqual({ status: "done", applied: ["notes/LOCAL.md"] });
    expect(resolveBinary).toHaveBeenCalledWith(
      "notes",
      "notes/LOCAL.md",
      "other",
      snapshotPath,
    );
  });

  it("stops on the first thrown error after closing that snapshot", async () => {
    const { steps, ...ops } = trackOps({
      onResolve: (rel) => {
        if (rel === "b.md") throw new Error("disk full");
        return { outcome: "applied" };
      },
    });
    const result = await applyKeepAll(
      "notes",
      [
        { rel: "a.md", keep: "live" },
        { rel: "b.md", keep: "live" },
        { rel: "c.md", keep: "live" },
      ],
      ops,
    );
    expect(steps).toEqual([
      "open a.md",
      "resolve a.md live",
      "close a.md",
      "open b.md",
      "resolve b.md live",
      "close b.md",
    ]);
    expect(result).toEqual({
      status: "stopped",
      applied: ["a.md"],
      failedRel: "b.md",
      message: "disk full",
    });
  });

  it("stops on BLOCKED without opening a later file", async () => {
    const { steps, ...ops } = trackOps({
      onResolve: (rel) => (rel === "b.md" ? BLOCKED : { outcome: "applied" }),
    });
    const result = await applyKeepAll(
      "notes",
      [
        { rel: "a.md", keep: "live" },
        { rel: "b.md", keep: "live" },
        { rel: "c.md", keep: "live" },
      ],
      ops,
    );
    expect(steps).toEqual([
      "open a.md",
      "resolve a.md live",
      "close a.md",
      "open b.md",
      "resolve b.md live",
      "close b.md",
    ]);
    expect(result).toEqual({
      status: "blocked",
      applied: ["a.md"],
      failedRel: "b.md",
    });
  });

  it("returns busy without opening a snapshot while a keep-all is running", async () => {
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const ops = trackOps({
      onOpen: async (rel) => {
        await gate;
        return resolution(rel);
      },
    });
    const first = applyKeepAll("notes", [{ rel: "a.md", keep: "live" }], ops);
    const second = await applyKeepAll("notes", [{ rel: "b.md", keep: "live" }], ops);
    expect(second).toEqual({
      status: "busy",
      applied: [],
      message: "Another keep-all is still running.",
    });
    expect(ops.openResolution).toHaveBeenCalledTimes(1);
    release();
    await expect(first).resolves.toEqual({ status: "done", applied: ["a.md"] });
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
