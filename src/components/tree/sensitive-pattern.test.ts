import { describe, expect, it } from "vitest";

import { sensitivePatternFor, withSensitivePattern } from "./sensitive-pattern";

describe("sensitivePatternFor", () => {
  it("anchors a plain rel at the root", () => {
    expect(sensitivePatternFor("CLAUDE.md")).toBe("/CLAUDE.md");
    expect(sensitivePatternFor(".claude/a.json")).toBe("/.claude/a.json");
  });

  it("escapes glob metacharacters", () => {
    expect(sensitivePatternFor("a/b[1]*.json")).toBe("/a/b\\[1\\]\\*.json");
    expect(sensitivePatternFor("what?.md")).toBe("/what\\?.md");
    expect(sensitivePatternFor("a\\b.md")).toBe("/a\\\\b.md");
  });

  it("escapes braces so they are not read as alternation", () => {
    expect(sensitivePatternFor("a{b,c}.md")).toBe("/a\\{b,c\\}.md");
  });

  it("wraps each trailing whitespace character in a brace group so trimming keeps it", () => {
    expect(sensitivePatternFor("x ")).toBe("/x{ }");
    expect(sensitivePatternFor("a b  ")).toBe("/a b{ }{ }");
    expect(sensitivePatternFor("t\t\u00a0")).toBe("/t{\t}{\u00a0}");
    expect(sensitivePatternFor("n\u0085")).toBe("/n{\u0085}");
  });

  it("escapes metacharacters before trailing whitespace without re-escaping the group", () => {
    expect(sensitivePatternFor("[x] ")).toBe("/\\[x\\]{ }");
  });
});

describe("withSensitivePattern", () => {
  it("appends a new pattern at the end", () => {
    expect(withSensitivePattern(["*.key"], "/a.json")).toEqual(["*.key", "/a.json"]);
  });

  it("returns the same list when the pattern is already present", () => {
    const patterns = ["*.key", "  /a.json  "];
    expect(withSensitivePattern(patterns, "/a.json")).toBe(patterns);
  });
});
