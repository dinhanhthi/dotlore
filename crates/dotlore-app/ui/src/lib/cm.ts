import { json } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { yaml } from "@codemirror/lang-yaml";
import {
  HighlightStyle,
  syntaxHighlighting,
} from "@codemirror/language";
import { EditorState, type Extension } from "@codemirror/state";
import { drawSelection, EditorView, lineNumbers } from "@codemirror/view";
import { tags as t } from "@lezer/highlight";

/** Chrome + gutters — Linear tokens from `index.css`, not a third-party theme. */
export const linearTheme = EditorView.theme(
  {
    "&": {
      height: "100%",
      backgroundColor: "var(--background)",
      color: "var(--foreground)",
      fontSize: "12px",
      fontFamily: "var(--font-mono)",
    },
    "&.cm-focused": {
      outline: "none",
    },
    ".cm-scroller": {
      overflow: "auto",
      fontFamily: "var(--font-mono)",
    },
    ".cm-content": {
      caretColor: "var(--primary)",
    },
    ".cm-gutters": {
      backgroundColor: "var(--background)",
      color: "var(--muted-foreground)",
      border: "none",
      borderRight: "1px solid var(--border)",
    },
    ".cm-lineNumbers .cm-gutterElement": {
      padding: "0 8px 0 12px",
    },
    ".cm-activeLine, .cm-activeLineGutter": {
      backgroundColor: "transparent",
    },
    ".cm-selectionBackground, &.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground":
      {
        backgroundColor: "color-mix(in srgb, var(--primary) 35%, transparent)",
      },
    ".cm-cursor, .cm-dropCursor": {
      borderLeftColor: "var(--primary)",
    },
  },
  { dark: true },
);

export const linearHighlight = HighlightStyle.define([
  { tag: t.comment, color: "var(--muted-foreground)", fontStyle: "italic" },
  { tag: t.lineComment, color: "var(--muted-foreground)", fontStyle: "italic" },
  { tag: t.blockComment, color: "var(--muted-foreground)", fontStyle: "italic" },
  { tag: t.keyword, color: "var(--primary)" },
  { tag: t.atom, color: "var(--primary)" },
  { tag: t.bool, color: "var(--status-conflict)" },
  { tag: t.null, color: "var(--status-conflict)" },
  { tag: t.number, color: "var(--status-conflict)" },
  { tag: t.string, color: "var(--status-synced)" },
  { tag: t.special(t.string), color: "var(--status-synced)" },
  { tag: t.propertyName, color: "var(--foreground)" },
  { tag: t.attributeName, color: "var(--primary)" },
  { tag: t.variableName, color: "var(--foreground)" },
  { tag: t.typeName, color: "var(--primary)" },
  { tag: t.className, color: "var(--primary)" },
  { tag: t.heading, color: "var(--foreground)", fontWeight: "bold" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strong, fontWeight: "bold" },
  { tag: t.link, color: "var(--primary)" },
  { tag: t.url, color: "var(--primary)" },
  { tag: t.meta, color: "var(--muted-foreground)" },
  { tag: t.punctuation, color: "var(--muted-foreground)" },
  { tag: t.separator, color: "var(--muted-foreground)" },
  { tag: t.operator, color: "var(--muted-foreground)" },
  { tag: t.processingInstruction, color: "var(--muted-foreground)" },
  { tag: t.monospace, fontFamily: "var(--font-mono)" },
]);

function fileExt(rel: string): string {
  const base = rel.replace(/\\/g, "/").split("/").pop() ?? "";
  const dot = base.lastIndexOf(".");
  if (dot <= 0) return "";
  return base.slice(dot).toLowerCase();
}

/** Markdown / JSON / YAML only; everything else stays plain text. */
export function languageFor(rel: string): Extension[] {
  switch (fileExt(rel)) {
    case ".md":
    case ".markdown":
      return [markdown()];
    case ".json":
      return [json()];
    case ".yml":
    case ".yaml":
      return [yaml()];
    default:
      return [];
  }
}

export function viewerExtensions(rel: string): Extension[] {
  return [
    lineNumbers(),
    drawSelection(),
    EditorState.readOnly.of(true),
    EditorView.editable.of(false),
    linearTheme,
    syntaxHighlighting(linearHighlight),
    ...languageFor(rel),
  ];
}
