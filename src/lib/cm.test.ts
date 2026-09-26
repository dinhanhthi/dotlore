import { ensureSyntaxTree } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { highlightTree, tags as t } from "@lezer/highlight";
import { describe, expect, it } from "vitest";

import { languageFor, linearHighlight } from "./cm";

function syntaxTree(rel: string, doc: string) {
  const state = EditorState.create({
    doc,
    extensions: languageFor(rel),
  });
  return ensureSyntaxTree(state, state.doc.length);
}

function styledSpans(rel: string, doc: string) {
  const tree = syntaxTree(rel, doc);
  const spans: { from: number; to: number; classes: string }[] = [];
  if (!tree) return spans;
  highlightTree(tree, linearHighlight, (from, to, classes) => {
    spans.push({ from, to, classes });
  });
  return spans;
}

type NamedNode = { name: string; parent: NamedNode | null };

function nodeNamesAt(rel: string, doc: string, pos: number): string[] {
  const tree = syntaxTree(rel, doc);
  if (!tree) return [];
  const names: string[] = [];
  let node: NamedNode | null = tree.resolveInner(pos, 1);
  while (node) {
    names.push(node.name);
    node = node.parent;
  }
  return names;
}

const markdownDoc = "# Title\n\n```ts\nconst n = 1\n```\n";

describe("languageFor", () => {
  it("highlights toml and leaves plain text unstyled", () => {
    const doc = 'model = "gpt"\n';
    expect(styledSpans("config.toml", doc).length).toBeGreaterThan(0);
    expect(styledSpans("notes.txt", doc)).toEqual([]);
  });

  it("highlights python", () => {
    const doc = "def run():\n    return 1\n";
    expect(styledSpans("hook.py", doc).length).toBeGreaterThan(0);
  });

  it("highlights shell", () => {
    const doc = 'echo "hi"\n';
    expect(styledSpans("run.sh", doc).length).toBeGreaterThan(0);
  });

  it("styles jsonc comments with the comment tag class", () => {
    const doc = '{\n  // note\n  "a": 1\n}\n';
    const commentClass = linearHighlight.style([t.comment]);
    expect(commentClass).toBeTruthy();
    const start = doc.indexOf("// note");
    const end = start + "// note".length;
    expect(
      styledSpans("opencode.jsonc", doc).some(
        (span) =>
          span.from <= start &&
          span.to >= end &&
          span.classes.split(" ").includes(commentClass!),
      ),
    ).toBe(true);
  });

  it("parses fenced typescript inside markdown and not plain text", () => {
    const at = markdownDoc.indexOf("n =");
    expect(nodeNamesAt("CLAUDE.md", markdownDoc, at)).toContain("VariableDefinition");
    expect(nodeNamesAt("notes.txt", markdownDoc, at)).not.toContain("VariableDefinition");
  });

  it("styles an html tag name with the tagName class", () => {
    const doc = '<p class="a">hi</p>\n';
    const tagClass = linearHighlight.style([t.tagName]);
    expect(tagClass).toBeTruthy();
    // Parent fallback to typeName must not count as the tagName rule.
    expect(tagClass).not.toBe(linearHighlight.style([t.typeName]));
    const start = doc.indexOf("p");
    expect(
      styledSpans("index.html", doc).some(
        (span) => span.from === start && span.to === start + 1 && span.classes === tagClass,
      ),
    ).toBe(true);
  });

  it("styles a python function name with a function variable class", () => {
    const doc = "def run():\n    return 1\n";
    const fnClass = linearHighlight.style([t.function(t.variableName)]);
    const defClass = linearHighlight.style([t.definition(t.function(t.variableName))]);
    const varClass = linearHighlight.style([t.variableName]);
    expect(fnClass).not.toBe(varClass);
    expect(defClass).not.toBe(varClass);
    const start = doc.indexOf("run");
    const end = start + "run".length;
    expect(
      styledSpans("hook.py", doc).some(
        (span) =>
          span.from <= start &&
          span.to >= end &&
          (span.classes === fnClass || span.classes === defClass),
      ),
    ).toBe(true);
  });

  it("highlights tsx, css, html, json, and yaml samples that contain a string", () => {
    const samples: [string, string][] = [
      ["page.tsx", 'const label = "hi";'],
      ["theme.css", 'a { content: "hi"; }'],
      ["index.html", '<p title="hi"></p>'],
      ["data.json", '{ "a": "hi" }'],
      ["config.yaml", 'a: "hi"'],
    ];
    for (const [rel, doc] of samples) {
      expect(styledSpans(rel, doc).length, rel).toBeGreaterThan(0);
    }
  });
});
