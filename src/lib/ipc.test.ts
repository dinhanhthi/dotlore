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
  getWorkSnapshot,
  listLinkable,
  removeRoot,
  runTask,
  sensitivePatterns,
  setSensitivePatterns,
  trackEntry,
  trackedFiles,
} from "./ipc";

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
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

describe("applyTrackBatch", () => {
  it("pauses for a sensitive file and tracks it after confirmation", async () => {
    invokeMock
      .mockResolvedValueOnce({
        outcome: "confirm_sensitive",
        paths: ["config/credentials.json"],
        more: false,
      })
      .mockResolvedValueOnce({ outcome: "done" });

    const batch = applyTrackBatch("proj", [
      { rel: "config/credentials.json", action: "track" },
    ]);
    await vi.waitFor(() => {
      expect(getWorkSnapshot().trackConfirm?.kind).toBe("sensitive");
    });
    answerTrackConfirm(true);
    await batch;

    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(invokeMock).toHaveBeenLastCalledWith("track_entry", {
      slug: "proj",
      rel: "config/credentials.json",
      confirmedFolderBytes: null,
      confirmedSensitive: true,
    });
    expect(getWorkSnapshot().banner).toBeNull();
  });

  it("leaves a sensitive folder untracked when confirmation is declined", async () => {
    invokeMock.mockResolvedValueOnce({
      outcome: "confirm_sensitive",
      paths: ["notes/server.pem"],
      more: false,
    });

    const batch = applyTrackBatch("proj", [{ rel: "notes", action: "track" }]);
    await vi.waitFor(() => {
      expect(getWorkSnapshot().trackConfirm?.kind).toBe("sensitive");
    });
    answerTrackConfirm(false);
    await batch;

    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(getWorkSnapshot().banner).toContain("notes was not tracked");
  });

  it("completes both confirmations for an oversized sensitive folder", async () => {
    invokeMock
      .mockResolvedValueOnce({
        outcome: "confirm_sensitive",
        paths: ["notes/server.pem"],
        more: false,
      })
      .mockResolvedValueOnce({
        outcome: "needs_confirmation",
        bytes: 2_000_000,
        folder_limit: 1_000_000,
        confirmation_required: true,
        skipped_too_large: [],
      })
      .mockResolvedValueOnce({ outcome: "done" });

    const batch = applyTrackBatch("proj", [{ rel: "notes", action: "track" }]);
    await vi.waitFor(() => {
      expect(getWorkSnapshot().trackConfirm?.kind).toBe("sensitive");
    });
    answerTrackConfirm(true);
    await vi.waitFor(() => {
      expect(getWorkSnapshot().trackConfirm?.kind).toBe("folder_limit");
    });
    answerTrackConfirm(true);
    await batch;

    expect(invokeMock).toHaveBeenCalledTimes(3);
    expect(invokeMock).toHaveBeenLastCalledWith("track_entry", {
      slug: "proj",
      rel: "notes",
      confirmedFolderBytes: 2_000_000,
      confirmedSensitive: true,
    });
    expect(getWorkSnapshot().banner).toBeNull();
  });

  it("preserves the additional-files warning for a 21-secret folder", async () => {
    invokeMock.mockResolvedValueOnce({
      outcome: "confirm_sensitive",
      paths: Array.from({ length: 20 }, (_, n) => `notes/credentials-${n}.json`),
      more: true,
    });

    const batch = applyTrackBatch("proj", [{ rel: "notes", action: "track" }]);
    await vi.waitFor(() => {
      expect(getWorkSnapshot().trackConfirm?.kind).toBe("sensitive");
    });
    const confirm = getWorkSnapshot().trackConfirm;
    expect(confirm).toMatchObject({ more: true });
    if (confirm?.kind !== "sensitive") throw new Error("expected sensitive prompt");
    const html = renderToStaticMarkup(
      createElement(SensitivePathList, { paths: confirm.paths, more: confirm.more }),
    );
    expect(html).toContain("overflow-y-auto");
    expect(html).toContain("max-h-");
    expect(html).toContain("break-all");
    expect(html).toContain(
      "additional secret files not shown",
    );
    expect(html.match(/<li\b/g)).toHaveLength(20);
    answerTrackConfirm(false);
    await batch;
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });
});
