import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
import { SensitivePathList } from "@/components/tree/EntryPickerDialog";
import {
  answerTrackConfirm,
  applyTrackBatch,
  BLOCKED,
  clearErrors,
  dismissError,
  getWorkSnapshot,
  listLinkable,
  removeRoot,
  reportError,
  runTask,
  sensitivePatterns,
  setSensitivePatterns,
  subscribeWork,
  trackEntry,
  trackedFiles,
  type TrackConfirm,
} from "./ipc";
import type { InspectedEntryDto, TrackResultDto } from "./types";

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  clearErrors();
});

describe("trackedFiles", () => {
  it("returns TrackedFile records", async () => {
    const files = [
      { rel: "CLAUDE.md", bytes: 12, state: "Synced" },
      { rel: "docs/a.md", bytes: 4, state: "Pending" },
    ];
    invokeMock.mockResolvedValue(files);
    await expect(trackedFiles("dotlore")).resolves.toEqual(files);
  });
});

describe("trackEntry", () => {
  it("sends the sensitive confirmation flag", async () => {
    invokeMock.mockResolvedValue({ outcome: "done" });
    await trackEntry("proj", "config/credentials.json", { confirmedSensitive: true });
    expect(invokeMock).toHaveBeenCalledWith("track_entry", {
      slug: "proj",
      rel: "config/credentials.json",
      confirmedFolderBytes: null,
      confirmedSensitive: true,
    });
  });
});

describe("sensitive patterns", () => {
  it("reads the list through sensitive_patterns", async () => {
    invokeMock.mockResolvedValue(["*.pem"]);
    await expect(sensitivePatterns()).resolves.toEqual(["*.pem"]);
    expect(invokeMock).toHaveBeenCalledWith("sensitive_patterns");
  });

  it("saves the list through set_sensitive_patterns", async () => {
    invokeMock.mockResolvedValue(undefined);
    await setSensitivePatterns(["*.secret", "!x.secret"]);
    expect(invokeMock).toHaveBeenCalledWith("set_sensitive_patterns", {
      patterns: ["*.secret", "!x.secret"],
    });
  });
});

describe("listLinkable", () => {
  it("returns LinkableRow records", async () => {
    const rows = [
      {
        slug: "old-mac-notes",
        display_name: "Old Mac Notes",
        is_agent: false,
      },
    ];
    invokeMock.mockResolvedValue(rows);
    await expect(listLinkable()).resolves.toEqual(rows);
  });
});

describe("error log", () => {
  it("logs a failing run() op exactly once", async () => {
    invokeMock.mockRejectedValue(new Error("disk full"));
    await expect(setSensitivePatterns(["*.pem"])).rejects.toThrow("disk full");
    expect(getWorkSnapshot().errors.map((e) => e.message)).toEqual(["disk full"]);
    expect(getWorkSnapshot().banner).toBe("disk full");
  });

  it("logs a failing runTask() once", async () => {
    await expect(
      runTask("Linking x…", () => Promise.reject(new Error("boom"))),
    ).rejects.toThrow("boom");
    expect(getWorkSnapshot().errors.map((e) => e.message)).toEqual(["boom"]);
  });

  it("keeps the newest 50 and drops the oldest", () => {
    for (let i = 0; i < 51; i++) reportError(`e${i}`);
    const messages = getWorkSnapshot().errors.map((e) => e.message);
    expect(messages).toHaveLength(50);
    expect(messages[0]).toBe("e50");
    expect(messages).not.toContain("e0");
  });

  it("dismisses one entry by id", () => {
    reportError("a");
    reportError("b");
    const [b] = getWorkSnapshot().errors;
    dismissError(b!.id);
    expect(getWorkSnapshot().errors.map((e) => e.message)).toEqual(["a"]);
  });

  it("clears every entry", () => {
    reportError("a");
    reportError("b");
    clearErrors();
    expect(getWorkSnapshot().errors).toEqual([]);
  });
});

describe("runTask", () => {
  it("carries the label while it runs and clears it afterwards", async () => {
    let seen: string | null = null;
    await runTask("Wiping cloud data…", async () => {
      seen = getWorkSnapshot().taskLabel;
    });
    expect(seen).toBe("Wiping cloud data…");
    expect(getWorkSnapshot().taskLabel).toBeNull();
  });

  it("clears the label when the task throws", async () => {
    await expect(
      runTask("Linking x…", () => Promise.reject(new Error("boom"))),
    ).rejects.toThrow("boom");
    expect(getWorkSnapshot().taskLabel).toBeNull();
    expect(getWorkSnapshot().banner).toBe("boom");
  });

  it("refuses a second task while one holds the slot, and does not run it", async () => {
    const second = vi.fn(async () => "ran");
    const first = runTask("Rebuilding x…", async () => {
      return await runTask("Wiping cloud data…", second);
    });
    await expect(first).resolves.toBe(BLOCKED);
    expect(second).not.toHaveBeenCalled();
    expect(getWorkSnapshot().banner).toBe("Wait for the current task to finish");
    expect(getWorkSnapshot().taskLabel).toBeNull();
  });

  /**
   * Tauri serializes a command returning `()` as `null`, so a `null` refusal
   * sentinel made a successful void write look refused and every caller
   * skipped its follow-up refresh.
   */
  it("does not mistake a command's null payload for a refusal", async () => {
    invokeMock.mockResolvedValue(null);
    await expect(removeRoot("dotlore")).resolves.not.toBe(BLOCKED);
    expect(invokeMock).toHaveBeenCalledWith("remove_root", { slug: "dotlore" });
  });
});


type InspectFields = Pick<
  InspectedEntryDto,
  "sensitivity" | "secret_descendants" | "secret_descendants_more"
>;

const PLAIN: InspectFields = {
  sensitivity: null,
  secret_descendants: [],
  secret_descendants_more: false,
};
const SECRET: InspectFields = { ...PLAIN, sensitivity: "secret" };

function secretFolder(paths: string[], more = false): InspectFields {
  return { ...PLAIN, secret_descendants: paths, secret_descendants_more: more };
}

/**
 * Route `invoke` by command. `inspect` answers the pre-scan (default plain);
 * `track` answers each `track_entry` call in order (default `done`).
 */
function routeInvoke(
  inspect: Record<string, InspectFields>,
  track: TrackResultDto[] = [],
): void {
  const queue = [...track];
  invokeMock.mockImplementation(async (cmd, args) => {
    const rel = (args as { rel: string }).rel;
    if (cmd === "inspect_entry") {
      return {
        kind: "file",
        bytes: 1,
        folder_limit: 1_000_000,
        confirmation_required: false,
        skipped_too_large: [],
        ...(inspect[rel] ?? PLAIN),
      };
    }
    if (cmd === "track_entry") return queue.shift() ?? { outcome: "done" };
    return [];
  });
}

function callsOf(cmd: string): Record<string, unknown>[] {
  return invokeMock.mock.calls
    .filter(([name]) => name === cmd)
    .map(([, args]) => args as Record<string, unknown>);
}

/** Count prompts shown while `batch` runs. */
function countPrompts(): { stop: () => number } {
  let count = 0;
  let open = false;
  const stop = subscribeWork(() => {
    const now = getWorkSnapshot().trackConfirm !== null;
    if (now && !open) count += 1;
    open = now;
  });
  return {
    stop: () => {
      stop();
      return count;
    },
  };
}

async function nextConfirm(kind: TrackConfirm["kind"]): Promise<TrackConfirm> {
  await vi.waitFor(() => {
    expect(getWorkSnapshot().trackConfirm?.kind).toBe(kind);
  });
  return getWorkSnapshot().trackConfirm!;
}

describe("applyTrackBatch", () => {
  it("asks once for three secret files and tracks them all confirmed", async () => {
    routeInvoke({ "a.pem": SECRET, "b.pem": SECRET, "c.pem": SECRET });
    const prompts = countPrompts();

    const batch = applyTrackBatch("proj", [
      { rel: "a.pem", action: "track" },
      { rel: "b.pem", action: "track" },
      { rel: "c.pem", action: "track" },
    ]);
    expect(await nextConfirm("sensitive")).toEqual({
      kind: "sensitive",
      rels: ["a.pem", "b.pem", "c.pem"],
      paths: ["a.pem", "b.pem", "c.pem"],
      more: false,
      canSkip: false,
    });
    answerTrackConfirm("all");
    await batch;

    expect(prompts.stop()).toBe(1);
    expect(callsOf("track_entry")).toEqual(
      ["a.pem", "b.pem", "c.pem"].map((rel) => ({
        slug: "proj",
        rel,
        confirmedFolderBytes: null,
        confirmedSensitive: true,
      })),
    );
    expect(getWorkSnapshot().banner).toBeNull();
  });

  it("skips the sensitive items and tracks the rest", async () => {
    routeInvoke({ "config/credentials.json": SECRET });

    const batch = applyTrackBatch("proj", [
      { rel: "config/credentials.json", action: "track" },
      { rel: "notes.md", action: "track" },
    ]);
    expect(await nextConfirm("sensitive")).toMatchObject({ canSkip: true });
    answerTrackConfirm("skip");
    await batch;

    expect(callsOf("track_entry")).toEqual([
      {
        slug: "proj",
        rel: "notes.md",
        confirmedFolderBytes: null,
        confirmedSensitive: false,
      },
    ]);
    expect(getWorkSnapshot().banner).toBe("config/credentials.json was not tracked");
    expect(getWorkSnapshot().errors).toEqual([]);
  });

  it("changes nothing when the prompt is cancelled", async () => {
    routeInvoke({ notes: secretFolder(["notes/server.pem"]) });

    const batch = applyTrackBatch("proj", [
      { rel: "notes", action: "track" },
      { rel: "old.md", action: "untrack" },
      { rel: "plain.md", action: "track" },
    ]);
    await nextConfirm("sensitive");
    answerTrackConfirm("cancel");
    await batch;

    expect(callsOf("track_entry")).toEqual([]);
    expect(callsOf("untrack_entry")).toEqual([]);
    expect(getWorkSnapshot().banner).toBe("Nothing was changed");
    expect(getWorkSnapshot().errors).toEqual([]);
  });

  it("offers no skip when every item is sensitive", async () => {
    routeInvoke({
      notes: secretFolder(["notes/server.pem"]),
      "a.pem": SECRET,
    });

    const batch = applyTrackBatch("proj", [
      { rel: "notes", action: "track" },
      { rel: "a.pem", action: "track" },
    ]);
    expect(await nextConfirm("sensitive")).toEqual({
      kind: "sensitive",
      rels: ["notes", "a.pem"],
      paths: ["a.pem", "notes/server.pem"],
      more: false,
      canSkip: false,
    });
    answerTrackConfirm("cancel");
    await batch;
  });

  it("counts an untrack as a skippable item", async () => {
    routeInvoke({ "a.pem": SECRET });

    const batch = applyTrackBatch("proj", [
      { rel: "a.pem", action: "track" },
      { rel: "old.md", action: "untrack" },
    ]);
    expect(await nextConfirm("sensitive")).toMatchObject({ canSkip: true });
    answerTrackConfirm("skip");
    await batch;

    expect(callsOf("track_entry")).toEqual([]);
    expect(callsOf("untrack_entry")).toEqual([{ slug: "proj", rel: "old.md" }]);
  });

  it("falls back to one prompt when a secret appears after the scan", async () => {
    routeInvoke({}, [
      { outcome: "confirm_sensitive", paths: ["notes/server.pem"], more: false },
    ]);
    const prompts = countPrompts();

    const batch = applyTrackBatch("proj", [
      { rel: "notes", action: "track" },
      { rel: "plain.md", action: "track" },
    ]);
    expect(await nextConfirm("sensitive")).toEqual({
      kind: "sensitive",
      rels: ["notes"],
      paths: ["notes/server.pem"],
      more: false,
      canSkip: false,
    });
    answerTrackConfirm("skip");
    await batch;

    expect(prompts.stop()).toBe(1);
    expect(callsOf("track_entry").map((args) => args.rel)).toEqual([
      "notes",
      "plain.md",
    ]);
    expect(getWorkSnapshot().banner).toBe("notes was not tracked");
  });

  it("stops on a pre-scan failure without running any op", async () => {
    invokeMock.mockImplementation(async (cmd) => {
      if (cmd === "inspect_entry") throw new Error("unsafe path ../x");
      return { outcome: "done" };
    });

    await applyTrackBatch("proj", [
      { rel: "old.md", action: "untrack" },
      { rel: "../x", action: "track" },
    ]);

    expect(callsOf("track_entry")).toEqual([]);
    expect(callsOf("untrack_entry")).toEqual([]);
    expect(getWorkSnapshot().banner).toBe("unsafe path ../x");
    expect(getWorkSnapshot().errors.map((e) => e.message)).toEqual([
      "unsafe path ../x",
    ]);
  });

  it("completes both confirmations for an oversized sensitive folder", async () => {
    routeInvoke({ notes: secretFolder(["notes/server.pem"]) }, [
      {
        outcome: "needs_confirmation",
        bytes: 2_000_000,
        folder_limit: 1_000_000,
        confirmation_required: true,
        skipped_too_large: [],
      },
    ]);

    const batch = applyTrackBatch("proj", [{ rel: "notes", action: "track" }]);
    await nextConfirm("sensitive");
    answerTrackConfirm("all");
    await nextConfirm("folder_limit");
    answerTrackConfirm("all");
    await batch;

    expect(callsOf("track_entry")).toEqual([
      {
        slug: "proj",
        rel: "notes",
        confirmedFolderBytes: null,
        confirmedSensitive: true,
      },
      {
        slug: "proj",
        rel: "notes",
        confirmedFolderBytes: 2_000_000,
        confirmedSensitive: true,
      },
    ]);
    expect(getWorkSnapshot().banner).toBeNull();
  });

  it("caps the listed paths at 20 across the batch", async () => {
    const paths = (dir: string) =>
      Array.from({ length: 12 }, (_, n) => `${dir}/credentials-${n}.json`);
    routeInvoke({ a: secretFolder(paths("a")), b: secretFolder(paths("b")) });

    const batch = applyTrackBatch("proj", [
      { rel: "a", action: "track" },
      { rel: "b", action: "track" },
    ]);
    const confirm = await nextConfirm("sensitive");
    expect(confirm).toMatchObject({ more: true });
    if (confirm.kind !== "sensitive") throw new Error("expected sensitive prompt");
    expect(confirm.paths).toEqual([...paths("a"), ...paths("b")].sort().slice(0, 20));
    answerTrackConfirm("cancel");
    await batch;
  });

  it("preserves the additional-files warning for a 21-secret folder", async () => {
    routeInvoke({
      notes: secretFolder(
        Array.from({ length: 20 }, (_, n) => `notes/credentials-${n}.json`),
        true,
      ),
    });

    const batch = applyTrackBatch("proj", [{ rel: "notes", action: "track" }]);
    const confirm = await nextConfirm("sensitive");
    expect(confirm).toMatchObject({ more: true });
    if (confirm.kind !== "sensitive") throw new Error("expected sensitive prompt");
    const html = renderToStaticMarkup(
      createElement(SensitivePathList, { paths: confirm.paths, more: confirm.more }),
    );
    expect(html).toContain("overflow-y-auto");
    expect(html).toContain("max-h-");
    expect(html).toContain("break-all");
    expect(html).toContain("additional secret files not shown");
    expect(html.match(/<li\b/g)).toHaveLength(20);
    answerTrackConfirm("cancel");
    await batch;
    expect(callsOf("track_entry")).toEqual([]);
  });
});
