import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { SidebarItem } from "./SidebarItem";

describe("SidebarItem", () => {
  it("shows a link button with the exact aria-label on an unlinked row", () => {
    const html = renderToStaticMarkup(
      <SidebarItem
        label="Old Mac Notes"
        linked={false}
        conflictCount={2}
        count={3}
        onClick={() => {}}
        onLink={() => {}}
        onToggleStar={() => {}}
        onReveal={() => {}}
        onRemove={() => {}}
      />,
    );

    const conflict = html.indexOf('aria-label="2 conflicts"');
    const star = html.indexOf('aria-label="Star"');
    const link = html.indexOf('aria-label="Link to a local folder"');
    expect(conflict).toBeGreaterThan(-1);
    expect(star).toBeGreaterThan(conflict);
    expect(html.slice(conflict, star)).toContain(">3<");
    expect(star).toBeLessThan(link);
    expect(classTokens(html, 'aria-label="Star"')).toContain("hidden");
    expect(classTokens(html, 'aria-label="Link to a local folder"')).not.toContain("hidden");
    expect(html).not.toContain('aria-label="Go to location"');
    expect(html).not.toContain('aria-label="Remove"');
    expect(html).toContain('data-slot="context-menu-trigger"');
  });

  it("does not wrap a nav row that has no menu callbacks", () => {
    const html = renderToStaticMarkup(
      <SidebarItem label="Conflicts" conflictCount={4} onClick={() => {}} />,
    );

    expect(html).toContain('aria-label="4 conflicts"');
    expect(html).not.toContain("context-menu");
  });

  it("wraps a loading keep-all row in a context-menu trigger", () => {
    const html = renderToStaticMarkup(
      <SidebarItem label="Notes" keepAll="loading" onClick={() => {}} />,
    );

    expect(html).toContain('data-slot="context-menu-trigger"');
  });

  it("wraps a row when only the menu open callback is set", () => {
    const html = renderToStaticMarkup(
      <SidebarItem label="Notes" onClick={() => {}} onMenuOpenChange={() => {}} />,
    );

    expect(html).toContain('data-slot="context-menu-trigger"');
  });

  it("shows a spinner in the link button while linking", () => {
    const html = renderToStaticMarkup(
      <SidebarItem
        label="Old Mac Notes"
        linked={false}
        linking
        onClick={() => {}}
        onLink={() => {}}
      />,
    );

    expect(html).toContain('aria-busy="true"');
    expect(html).toContain("animate-spin");
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

/** Class tokens of the element and its still-open ancestors at `marker`. */
function classTokens(html: string, marker: string): string[] {
  const at = html.indexOf(marker);
  const stack: string[] = [];
  const tags = /<(\/?)([a-zA-Z][\w:-]*)([^>]*?)(\/?)>/g;
  let match: RegExpExecArray | null;
  while ((match = tags.exec(html)) && match.index < at) {
    const [, slash, , attrs, selfClosing] = match;
    if (selfClosing === "/") continue;
    if (slash === "/") {
      stack.pop();
      continue;
    }
    stack.push(/class="([^"]*)"/.exec(attrs)?.[1] ?? "");
  }
  return stack.flatMap((classes) => classes.split(/\s+/).filter(Boolean));
}
