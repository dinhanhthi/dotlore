import { beforeEach, describe, expect, it, vi } from "vitest";

import { useCloseGuard } from "./close-guard";

const { win, closeHandlers } = vi.hoisted(() => {
  const closeHandlers: ((event: { preventDefault: () => void }) => void)[] =
    [];
  return {
    closeHandlers,
    win: {
      destroy: vi.fn(async (): Promise<void> => {}),
      onCloseRequested: vi.fn(
        (handler: (event: { preventDefault: () => void }) => void) => {
          closeHandlers.push(handler);
          return Promise.resolve(() => {});
        },
      ),
    },
  };
});

vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => win }));

// No renderer here: run the effect inline when the hook registers it.
vi.mock("react", async () => {
  const actual = await vi.importActual<typeof import("react")>("react");
  return { ...actual, useEffect: (effect: () => void) => effect() };
});

function requestClose(): { preventDefault: ReturnType<typeof vi.fn> } {
  const event = { preventDefault: vi.fn() };
  closeHandlers.at(-1)?.(event);
  return event;
}

describe("useCloseGuard", () => {
  beforeEach(() => {
    closeHandlers.length = 0;
    win.destroy.mockClear();
  });

  it("closes at once when the guard lets it through", () => {
    useCloseGuard((run) => run());
    const event = requestClose();
    expect(event.preventDefault).toHaveBeenCalled();
    expect(win.destroy).toHaveBeenCalledTimes(1);
  });

  it("keeps the window until the guard runs the close", () => {
    let pending: (() => void) | null = null;
    useCloseGuard((run) => {
      pending = run;
    });
    const event = requestClose();
    expect(event.preventDefault).toHaveBeenCalled();
    expect(win.destroy).not.toHaveBeenCalled();
    pending!();
    expect(win.destroy).toHaveBeenCalledTimes(1);
  });
});
