import { Chunk } from "@codemirror/merge";
import { EditorState, Text } from "@codemirror/state";
import { type DecorationSet, EditorView } from "@codemirror/view";
import { describe, expect, it } from "vitest";

import {
  chunkToggleGutter,
  resultDecorations,
  setCurrentChunk,
  setSidePicks,
  sidePicks,
} from "./merge-controls";
import { assembleResult, resultSlots, toggleSlot } from "./merge-result";

const doc = (s: string) => Text.of(s.split("\n"));

function setup(a: string, b: string) {
  const chunks = Chunk.build(doc(a), doc(b));
  const { text, slots } = assembleResult(a, b, chunks, chunks.map(() => []));
  const state = EditorState.create({
    doc: text,
    extensions: [resultSlots.init(() => slots), resultDecorations()],
  });
  return { chunks, state };
}

function decos(state: EditorState) {
  const out: { from: number; line: boolean }[] = [];
  for (const d of state.facet(EditorView.decorations)) {
    if (typeof d === "function") continue;
    (d as DecorationSet).between(0, state.doc.length, (from, _to, v) => {
      out.push({ from, line: v.spec.class === "cm-chunk-current" });
    });
  }
  return out;
}

describe("resultDecorations", () => {
  const a = "x\na1\ny\na2\nz";
  const b = "x\nb1\ny\nb2\nz";

  it("places one block widget per empty slot", () => {
    const { state } = setup(a, b);
    expect(decos(state)).toEqual([
      { from: 2, line: false },
      { from: 4, line: false },
    ]);
  });

  it("highlights the lines of the current filled slot", () => {
    let { state } = setup(a, b);
    state = state.update(toggleSlot(state, 1, "b")).state;
    state = state.update({ effects: setCurrentChunk.of(1) }).state;
    expect(state.doc.toString()).toBe("x\ny\nb2\nz");
    expect(decos(state)).toEqual([
      { from: 2, line: false },
      { from: 4, line: true },
    ]);
    state = state.update({ effects: setCurrentChunk.of(0) }).state;
    expect(decos(state)).toEqual([{ from: 2, line: false }]);
  });
});

describe("chunkToggleGutter", () => {
  it("tracks per-side picks", () => {
    const a = "x\na1\ny";
    const chunks = Chunk.build(doc(a), doc("x\nb1\ny"));
    let state = EditorState.create({
      doc: a,
      extensions: chunkToggleGutter("a", chunks, () => {}),
    });
    expect(state.field(sidePicks)).toEqual([]);
    state = state.update({ effects: setSidePicks.of([true]) }).state;
    expect(state.field(sidePicks)).toEqual([true]);
  });
});
