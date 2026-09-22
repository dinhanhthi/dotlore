import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { SidebarSection } from "./SidebarSection";

describe("SidebarSection", () => {
  it("keeps a collapsed list mounted so its height can ease closed", () => {
    const html = renderToStaticMarkup(
      <SidebarSection id="agents" title="Agents" collapsed onToggle={() => {}}>
        <span>Claude</span>
      </SidebarSection>,
    );

    expect(html).toContain("Claude");
    expect(html).toContain('data-collapsed="true"');
    expect(html).toContain("grid-rows-[0fr]");
    expect(html).toContain("transition-[grid-template-rows]");
  });
});
