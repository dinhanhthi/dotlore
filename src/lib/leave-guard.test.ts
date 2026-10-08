import { describe, expect, it } from "vitest";

import {
  closeClean,
  isSameTarget,
  remoteResolveAction,
  shouldPromptLeave,
  type LeaveState,
  type LeaveTarget,
} from "./leave-guard";

const resolving: LeaveState = { selectedSlug: "proj", resolvingRel: "a/b.md" };
const other: LeaveTarget = { kind: "other" };

describe("shouldPromptLeave", () => {
  it("never prompts when no resolver is open", () => {
    const state = { selectedSlug: "proj", resolvingRel: null };
    expect(shouldPromptLeave(state, true, other)).toBe(false);
    expect(shouldPromptLeave(state, true, { kind: "selectRoot", slug: "x" })).toBe(false);
  });

  it("never prompts when the resolver is clean", () => {
    expect(shouldPromptLeave(resolving, false, other)).toBe(false);
    expect(
      shouldPromptLeave(resolving, false, { kind: "openResolver", slug: "x", rel: "y" }),
    ).toBe(false);
  });

  it("prompts on selectFile, show views and closeResolver when dirty", () => {
    expect(shouldPromptLeave(resolving, true, other)).toBe(true);
  });

  it("skips openResolver for the same slug and rel", () => {
    const target: LeaveTarget = { kind: "openResolver", slug: "proj", rel: "a/b.md" };
    expect(shouldPromptLeave(resolving, true, target)).toBe(false);
  });

  it("prompts openResolver for a different rel or slug", () => {
    expect(
      shouldPromptLeave(resolving, true, { kind: "openResolver", slug: "proj", rel: "c.md" }),
    ).toBe(true);
    expect(
      shouldPromptLeave(resolving, true, { kind: "openResolver", slug: "other", rel: "a/b.md" }),
    ).toBe(true);
  });

  it("skips selectRoot for the same slug, prompts for another", () => {
    expect(shouldPromptLeave(resolving, true, { kind: "selectRoot", slug: "proj" })).toBe(false);
    expect(shouldPromptLeave(resolving, true, { kind: "selectRoot", slug: "other" })).toBe(true);
  });
});

describe("isSameTarget", () => {
  it("treats other as always leaving", () => {
    expect(isSameTarget(resolving, other)).toBe(false);
  });
});

describe("closeClean", () => {
  it("resets dirty before closing, so the close does not prompt", () => {
    let dirty = true;
    const calls: string[] = [];
    let prompted: boolean | null = null;
    closeClean(
      (next) => {
        dirty = next;
        calls.push(`dirty:${next}`);
      },
      () => {
        calls.push("close");
        prompted = shouldPromptLeave(resolving, dirty, other);
      },
    );
    expect(calls).toEqual(["dirty:false", "close"]);
    expect(prompted).toBe(false);
  });
});

describe("remoteResolveAction", () => {
  it("keeps unsaved Result edits open with a notice", () => {
    expect(remoteResolveAction(false, false, true)).toBe("notice");
  });

  it("closes a clean resolver once the conflict is gone", () => {
    expect(remoteResolveAction(false, false, false)).toBe("close");
  });

  it("stays while the file is still conflicted or a local resolve runs", () => {
    expect(remoteResolveAction(true, false, false)).toBe("stay");
    expect(remoteResolveAction(false, true, false)).toBe("stay");
  });
});
