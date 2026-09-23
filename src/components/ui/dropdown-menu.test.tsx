import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { seen } = vi.hoisted(() => ({
  seen: [] as { kind: string; closeOnClick: unknown }[],
}));

vi.mock("@base-ui/react/menu", async () => {
  const actual =
    await vi.importActual<typeof import("@base-ui/react/menu")>(
      "@base-ui/react/menu",
    );
  const spy =
    (kind: "RadioItem" | "CheckboxItem") =>
    (props: { closeOnClick?: unknown; children?: React.ReactNode }) => {
      seen.push({ kind, closeOnClick: props.closeOnClick });
      return <div>{props.children}</div>;
    };
  return {
    Menu: {
      ...actual.Menu,
      RadioItem: spy("RadioItem"),
      CheckboxItem: spy("CheckboxItem"),
      RadioItemIndicator: () => null,
      CheckboxItemIndicator: () => null,
    },
  };
});

import {
  DropdownMenuCheckboxItem,
  DropdownMenuRadioItem,
} from "./dropdown-menu";

describe("dropdown selection items", () => {
  beforeEach(() => {
    seen.length = 0;
  });

  it("close the menu once a radio item is picked", () => {
    renderToStaticMarkup(<DropdownMenuRadioItem value="a">A</DropdownMenuRadioItem>);
    expect(seen).toEqual([{ kind: "RadioItem", closeOnClick: true }]);
  });

  it("close the menu once a checkbox item is toggled", () => {
    renderToStaticMarkup(<DropdownMenuCheckboxItem>A</DropdownMenuCheckboxItem>);
    expect(seen).toEqual([{ kind: "CheckboxItem", closeOnClick: true }]);
  });

  it("still lets a caller keep the menu open", () => {
    renderToStaticMarkup(
      <DropdownMenuRadioItem value="a" closeOnClick={false}>
        A
      </DropdownMenuRadioItem>,
    );
    expect(seen).toEqual([{ kind: "RadioItem", closeOnClick: false }]);
  });
});
