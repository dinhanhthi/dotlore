import { css, cssLanguage } from "@codemirror/lang-css";
import { html, htmlLanguage } from "@codemirror/lang-html";
import {
  javascript,
  javascriptLanguage,
  jsxLanguage,
  tsxLanguage,
  typescriptLanguage,
} from "@codemirror/lang-javascript";
import { json, jsonLanguage } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { python, pythonLanguage } from "@codemirror/lang-python";
import { yaml, yamlLanguage } from "@codemirror/lang-yaml";
import {
  HighlightStyle,
  LanguageSupport,
  StreamLanguage,
  syntaxHighlighting,
  type Language,
} from "@codemirror/language";
import { shell } from "@codemirror/legacy-modes/mode/shell";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { EditorState, type Extension } from "@codemirror/state";
import { drawSelection, EditorView, lineNumbers } from "@codemirror/view";
import { styleTags, tags as t } from "@lezer/highlight";
import { jsoncLanguage } from "@shopify/lang-jsonc";

/** Chrome + gutters — Linear tokens from `index.css`, not a third-party theme. */
export const linearTheme = EditorView.theme(
  {
    "&": {
      height: "100%",
      backgroundColor: "var(--editor-background)",
      color: "var(--editor-foreground)",
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
      backgroundColor: "var(--editor-background)",
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
  { tag: t.propertyName, color: "var(--editor-foreground)" },
  { tag: t.attributeName, color: "var(--primary)" },
  { tag: t.variableName, color: "var(--editor-foreground)" },
  { tag: t.typeName, color: "var(--primary)" },
  { tag: t.className, color: "var(--primary)" },
  { tag: t.heading, color: "var(--editor-foreground)", fontWeight: "bold" },
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
  { tag: t.tagName, color: "var(--primary)" },
  { tag: t.standard(t.tagName), color: "var(--primary)" },
  { tag: t.angleBracket, color: "var(--muted-foreground)" },
  { tag: t.attributeValue, color: "var(--status-synced)" },
  { tag: t.function(t.variableName), color: "var(--primary)" },
  { tag: t.definition(t.function(t.variableName)), color: "var(--primary)" },
]);

function fileExt(rel: string): string {
  const base = rel.replace(/\\/g, "/").split("/").pop() ?? "";
  const dot = base.lastIndexOf(".");
  if (dot <= 0) return "";
  return base.slice(dot).toLowerCase();
}

const shellSupport = new LanguageSupport(StreamLanguage.define(shell));
const tomlSupport = new LanguageSupport(StreamLanguage.define(toml));
// jsonc tags line comments as t.lineComment, which has its own class.
// Map them to t.comment so the comment style class covers `//`.
const jsoncSupport = new LanguageSupport(
  jsoncLanguage.configure({
    props: [styleTags({ LineComment: t.comment })],
  }),
);

function codeLanguage(info: string): Language | null {
  switch (info.trim().split(/\s+/)[0]?.toLowerCase()) {
    case "js":
    case "javascript":
      return javascriptLanguage;
    case "jsx":
      return jsxLanguage;
    case "ts":
    case "typescript":
      return typescriptLanguage;
    case "tsx":
      return tsxLanguage;
    case "py":
    case "python":
      return pythonLanguage;
    case "html":
      return htmlLanguage;
    case "css":
      return cssLanguage;
    case "json":
      return jsonLanguage;
    case "yml":
    case "yaml":
      return yamlLanguage;
    case "sh":
    case "bash":
    case "zsh":
    case "shell":
      return shellSupport.language;
    case "toml":
      return tomlSupport.language;
    case "jsonc":
      return jsoncSupport.language;
    default:
      return null;
  }
}

/** Include-list languages. Unknown extensions stay plain text. */
export function languageFor(rel: string): Extension[] {
  switch (fileExt(rel)) {
    case ".md":
    case ".markdown":
      return [markdown({ codeLanguages: codeLanguage })];
    case ".json":
      return [json()];
    case ".jsonc":
      return [jsoncSupport];
    case ".yml":
    case ".yaml":
      return [yaml()];
    case ".toml":
      return [tomlSupport];
    case ".sh":
    case ".bash":
    case ".zsh":
      return [shellSupport];
    case ".js":
    case ".mjs":
    case ".cjs":
      return [javascript()];
    case ".jsx":
      return [javascript({ jsx: true })];
    case ".ts":
    case ".mts":
    case ".cts":
      return [javascript({ typescript: true })];
    case ".tsx":
      return [javascript({ jsx: true, typescript: true })];
    case ".py":
      return [python()];
    case ".html":
    case ".htm":
      return [html()];
    case ".css":
      return [css()];
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

/** Editable result pane — same chrome as the viewer, without read-only. */
export function editorExtensions(rel: string): Extension[] {
  return [
    lineNumbers(),
    drawSelection(),
    linearTheme,
    syntaxHighlighting(linearHighlight),
    ...languageFor(rel),
  ];
}
