import type { ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { SidebarItem } from "./SidebarItem";

const { items } = vi.hoisted(() => ({
  items: new Map<string, { onClick?: () => void; disabled?: boolean }>(),
}));

// A closed menu renders nothing; flatten it so its items show up.
vi.mock("@/components/ui/context-menu", () => ({
  ContextMenu: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  ContextMenuTrigger: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  ContextMenuContent: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  ContextMenuSeparator: () => <hr />,
  ContextMenuItem: (props: {
    children: ReactNode;
    onClick?: () => void;
    disabled?: boolean;
  }) => {
    if (typeof props.children === "string") items.set(props.children, props);
    return <div role="menuitem">{props.children}</div>;
  },
}));

function renderMenu(props: Partial<Parameters<typeof SidebarItem>[0]> = {}) {
  items.clear();
  return renderToStaticMarkup(
    <SidebarItem label="Notes" onClick={() => {}} onRemove={() => {}} {...props} />,
  );
}

describe("SidebarItem context menu", () => {
  beforeEach(() => items.clear());

  it("offers Unlink between Go to location and Remove when it has a handler", () => {
    const onUnlink = vi.fn();
    const html = renderMenu({ onReveal: () => {}, onUnlink });
    const reveal = html.indexOf(">Go to location<");
    const unlink = html.indexOf(">Unlink<");
    expect(unlink).toBeGreaterThan(reveal);
    expect(html.indexOf(">Remove<")).toBeGreaterThan(unlink);
    items.get("Unlink")?.onClick?.();
    expect(onUnlink).toHaveBeenCalledOnce();
  });

  it("disables Unlink while writes are blocked", () => {
    renderMenu({ onUnlink: () => {}, writeDisabled: true });
    expect(items.get("Unlink")?.disabled).toBe(true);
  });

  it("leaves Unlink out without a handler", () => {
    expect(renderMenu()).not.toContain(">Unlink<");
  });
});
