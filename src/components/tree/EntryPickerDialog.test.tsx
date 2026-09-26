import { isValidElement, type ReactElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import type { TrackConfirm } from "@/lib/ipc";
import type { PickerRow, Sensitivity } from "@/lib/types";

import {
  PickerNode,
  SensitivePathList,
  TrackConfirmActions,
  trackConfirmTitle,
} from "./EntryPickerDialog";

function renderRow(
  name: string,
  sensitivity: Sensitivity | null,
  kind: "file" | "directory" = "file",
): string {
  const row: PickerRow = { name, kind, rel: name, sensitivity };
  return renderToStaticMarkup(
    <PickerNode
      row={row}
      entries={[]}
      pending={{}}
      trackedRels={new Set()}
      expanded={{}}
      childrenByRel={{}}
      loadingRel={{}}
      needle=""
      onToggle={() => {}}
      onStage={() => {}}
    />,
  );
}

describe("PickerNode sensitivity", () => {
  it("shows the warning and the warning-colored name on a secret file", () => {
    const html = renderRow("credentials.json", "secret");
    expect(html).toContain('aria-label="Sensitive file — may contain secrets"');
    expect(html).toMatch(/<span class="[^"]*text-status-conflict[^"]*">credentials\.json<\/span>/);
    expect(html.indexOf('aria-label="Sensitive file')).toBeLessThan(
      html.indexOf('aria-label="Track credentials.json"'),
    );
  });

  it("shows the info hint on a tokenHint file", () => {
    const html = renderRow(".mcp.json", "tokenHint");
    expect(html).toContain('aria-label="May contain API tokens"');
    expect(html).not.toMatch(/text-status-conflict[^"]*">\.mcp\.json/);
  });

  it("shows neither mark on a plain file", () => {
    const html = renderRow("notes.md", null);
    expect(html).not.toContain('aria-label="Sensitive file');
    expect(html).not.toContain('aria-label="May contain API tokens"');
    expect(html).not.toMatch(/text-status-conflict[^"]*">notes\.md/);
  });

  it("shows no mark on a folder row", () => {
    const html = renderRow("secrets", "secret", "directory");
    expect(html).not.toContain('aria-label="Sensitive file');
    expect(html).not.toContain('aria-label="May contain API tokens"');
    expect(html).not.toContain("text-status-conflict");
  });
});

describe("SensitivePathList", () => {
  it("keeps secret paths in a scrollable region and notes hidden paths", () => {
    const html = renderToStaticMarkup(
      <SensitivePathList paths={["notes/credentials.json", "notes/server.pem"]} more />,
    );
    expect(html).toContain("max-h-40");
    expect(html).toContain("overflow-y-auto");
    expect(html).toContain("notes/credentials.json");
    expect(html).toContain("notes/server.pem");
    expect(html).toContain("additional secret files not shown");
  });

  it("does not show the overflow note when all paths are listed", () => {
    const html = renderToStaticMarkup(
      <SensitivePathList paths={["notes/credentials.json"]} more={false} />,
    );
    expect(html).not.toContain("additional secret files not shown");
  });
});

function sensitiveConfirm(rels: string[], canSkip: boolean): TrackConfirm {
  return { kind: "sensitive", rels, paths: rels, more: false, canSkip };
}

function buttonLabels(confirm: TrackConfirm): string[] {
  const html = renderToStaticMarkup(
    <TrackConfirmActions confirm={confirm} onAnswer={() => {}} />,
  );
  return [...html.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map((m) => m[1]!);
}

/** The footer's buttons as elements, so each `onClick` can be called directly. */
function buttonElements(confirm: TrackConfirm, onAnswer: () => void) {
  const root = TrackConfirmActions({ confirm, onAnswer }) as ReactElement<{
    children: ReactNode;
  }>;
  return ([root.props.children].flat(2) as ReactNode[]).filter(isValidElement) as ReactElement<{
    children: string;
    onClick: (event: { preventDefault: () => void }) => void;
  }>[];
}

describe("TrackConfirmActions", () => {
  it("shows Cancel, Skip sensitive and Track all when skipping is possible", () => {
    expect(buttonLabels(sensitiveConfirm(["a.pem", "notes.md"], true))).toEqual([
      "Cancel",
      "Skip sensitive",
      "Track all",
    ]);
  });

  it("hides Skip sensitive when every item is sensitive", () => {
    expect(buttonLabels(sensitiveConfirm(["a.pem"], false))).toEqual([
      "Cancel",
      "Track all",
    ]);
  });

  it("keeps Cancel and Track for the folder limit", () => {
    expect(
      buttonLabels({ kind: "folder_limit", rel: "big", bytes: 2, folderLimit: 1 }),
    ).toEqual(["Cancel", "Track"]);
  });

  it("answers the value matching each button", () => {
    const onAnswer = vi.fn();
    for (const button of buttonElements(sensitiveConfirm(["a.pem"], true), onAnswer)) {
      button.props.onClick({ preventDefault: () => {} });
    }
    expect(onAnswer.mock.calls).toEqual([["cancel"], ["skip"], ["all"]]);
  });
});

describe("trackConfirmTitle", () => {
  it("names the single sensitive item", () => {
    expect(trackConfirmTitle(sensitiveConfirm(["config/credentials.json"], false))).toBe(
      "Track config/credentials.json?",
    );
  });

  it("counts several sensitive items", () => {
    expect(trackConfirmTitle(sensitiveConfirm(["a.pem", "b.pem", "notes"], true))).toBe(
      "Track 3 items?",
    );
  });

  it("names the folder over the limit", () => {
    expect(
      trackConfirmTitle({ kind: "folder_limit", rel: "big", bytes: 2, folderLimit: 1 }),
    ).toBe("Track big?");
  });
});
