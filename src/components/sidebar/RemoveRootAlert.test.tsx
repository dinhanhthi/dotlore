import type { ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";

import { RemoveRootAlert } from "./RemoveRootAlert";

const { actions, forgetRoot, removeRoot } = vi.hoisted(() => ({
  actions: [] as Array<(event: { preventDefault: () => void }) => void>,
  forgetRoot: vi.fn(async () => {}),
  removeRoot: vi.fn(async () => {}),
}));

vi.mock("@/lib/ipc", () => ({
  BLOCKED: Symbol("blocked"),
  forgetRoot,
  removeRoot,
}));

// A closed dialog renders nothing; flatten it so its text and action show up.
vi.mock("@/components/ui/alert-dialog", () => {
  const Pass = ({ children }: { children?: ReactNode }) => <div>{children}</div>;
  return {
    AlertDialog: Pass,
    AlertDialogContent: Pass,
    AlertDialogHeader: Pass,
    AlertDialogFooter: Pass,
    AlertDialogTitle: Pass,
    AlertDialogDescription: Pass,
    AlertDialogCancel: Pass,
    AlertDialogAction: (props: {
      children: ReactNode;
      onClick: (event: { preventDefault: () => void }) => void;
    }) => {
      actions.push(props.onClick);
      return <button type="button">{props.children}</button>;
    },
  };
});

function render(): string {
  actions.length = 0;
  const value = {
    ...emptyRootsState,
    refreshRoots: async () => {},
    locked: false,
  } as unknown as RootsContextValue;
  return renderToStaticMarkup(
    <RootsContext.Provider value={value}>
      <RemoveRootAlert slug="notes" name="Notes" open onOpenChange={() => {}} />
    </RootsContext.Provider>,
  );
}

describe("RemoveRootAlert", () => {
  beforeEach(() => {
    forgetRoot.mockClear();
    removeRoot.mockClear();
  });

  it("says the project leaves this Mac but the cloud copy stays", () => {
    const html = render();
    expect(html).toContain("hide it on this Mac");
    expect(html).toContain("cloud");
  });

  it("removes and hides the project on confirm, not just unlinks it", () => {
    render();
    actions.at(-1)?.({ preventDefault: () => {} });
    expect(forgetRoot).toHaveBeenCalledExactlyOnceWith("notes");
    expect(removeRoot).not.toHaveBeenCalled();
  });
});
