import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { SidebarItem } from "./SidebarItem";

describe("SidebarItem", () => {
  it("shows a link button with the exact aria-label on an unlinked row", () => {
    const html = renderToStaticMarkup(
      <SidebarItem
        label="Old Mac Notes"
        linked={false}
        onClick={() => {}}
        onLink={() => {}}
      />,
    );

    expect(html).toContain('aria-label="Link to a local folder"');
  });

  it("does not show the link button on a linked row", () => {
    const html = renderToStaticMarkup(
      <SidebarItem
        label="dotlore"
        linked={true}
        onClick={() => {}}
        onLink={() => {}}
      />,
    );

    expect(html).not.toContain('aria-label="Link to a local folder"');
  });
});
