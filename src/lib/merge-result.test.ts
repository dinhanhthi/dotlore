import { Chunk } from "@codemirror/merge";
import { EditorState, Text } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import {
  assembleResult,
  chunkAtHeight,
  chunkLineRanges,
  keepAll,
  resultPosForA,
  resultSlots,
  type Side,
  slotSummary,
  toggleSlot,
} from "./merge-result";

const doc = (s: string) => Text.of(s.split("\n"));

function setup(a: string, b: string) {
  const chunks = Chunk.build(doc(a), doc(b));
  const { text, slots } = assembleResult(a, b, chunks, chunks.map(() => []));
  const state = EditorState.create({
    doc: text,
    extensions: resultSlots.init(() => slots),
  });
  return { chunks, state };
}

const toggle = (s: EditorState, i: number, side: Side) =>
  s.update(toggleSlot(s, i, side)).state;

const edit = (s: EditorState, from: number, to: number, insert: string) =>
  s.update({ changes: { from, to, insert } }).state;

const slots = (s: EditorState) => s.field(resultSlots);

const PAIRS: [string, string][] = [
  ["a\n", "a\nb"],
  ["a", "a\n"],
  ["a\n", "a"],
  ["", "y"],
  ["x", "y"],
  ["", ""],
  ["a\nb\n", "a\nb\nc\n"],
  ["a\nb", "a\nb\nc"],
  ["a\nb\nc", "a\nc"],
  ["a\n\nc", "a\nc"],
  ["x\na", "a"],
  ["a\nb", "a\nc"],
  ["a\nb\n", "a\nc"],
  ["a\n\n", "a\n"],
  ["\n", ""],
  ["\nx", "\ny"],
  ["x\n", "\n"],
  ["one\ntwo\nthree\nfour\nfive\nsix", "ONE\ntwo\n2b\nthree\nfive\nsix\nseven"],
  ["h\n\nx\n\ny\n", "h\n\nX\n\nY"],
];

describe("chunkLineRanges", () => {
  it("maps a chunk to 0-based lines, EOF included", () => {
    expect(chunkLineRanges(doc("a\nb"), 2, 4)).toEqual({ start: 1, end: 2 });
    expect(chunkLineRanges(doc("a\nc"), 2, 2)).toEqual({ start: 1, end: 1 });
    expect(chunkLineRanges(doc("a\nb\nc"), 2, 4)).toEqual({ start: 1, end: 2 });
  });
});

describe("slot round-trips", () => {
  it.each(PAIRS)("%j vs %j", (a, b) => {
    const { chunks, state } = setup(a, b);
    const common = assembleResult(a, b, chunks, chunks.map(() => [])).text;
    expect(state.doc.toString()).toBe(common);
    expect(state.update(keepAll(state, "a")).state.doc.toString()).toBe(a);
    expect(state.update(keepAll(state, "b")).state.doc.toString()).toBe(b);

    for (const first of ["a", "b"] as const) {
      const second = first === "a" ? "b" : "a";
      let s = state;
      for (let i = 0; i < chunks.length; i++) s = toggle(s, i, first);
      expect(s.doc.toString()).toBe(first === "a" ? a : b);
      for (let i = 0; i < chunks.length; i++) s = toggle(s, i, second);
      expect(s.doc.toString()).toBe(
        assembleResult(a, b, chunks, chunks.map(() => [first, second])).text,
      );
      for (let i = 0; i < chunks.length; i++) s = toggle(s, i, first);
      expect(s.doc.toString()).toBe(second === "a" ? a : b);
      for (let i = 0; i < chunks.length; i++) s = toggle(s, i, second);
      expect(s.doc.toString()).toBe(common);
    }

    const kept = state.update(keepAll(state, "b")).state;
    expect(kept.update(keepAll(kept, "a")).state.doc.toString()).toBe(a);
  });
});

describe("toggles", () => {
  const A = "one\ntwo\nthree\nfour\nfive";
  const B = "one\nTWO\nthree\nfive\nsix";

  it("starts with only the common lines, nothing resolved", () => {
    const { state } = setup(A, B);
    expect(state.doc.toString()).toBe("one\nthree");
    expect(slotSummary(state)).toEqual({
      total: 2,
      resolved: [false, false],
      picks: [[], []],
    });
  });

  it("keeps click order and untoggles", () => {
    let { state } = setup(A, B);
    state = toggle(state, 0, "a");
    state = toggle(state, 0, "b");
    expect(state.doc.toString()).toBe("one\ntwo\nTWO\nthree");
    state = toggle(state, 0, "a");
    expect(state.doc.toString()).toBe("one\nTWO\nthree");
    state = toggle(state, 0, "a");
    expect(state.doc.toString()).toBe("one\nTWO\ntwo\nthree");
    expect(slotSummary(state).picks[0]).toEqual(["b", "a"]);
    state = toggle(state, 0, "b");
    state = toggle(state, 0, "a");
    expect(state.doc.toString()).toBe("one\nthree");
    expect(slotSummary(state).resolved[0]).toBe(false);
  });

  it("picks the empty side of delete-only and insert-only chunks", () => {
    let { state } = setup("a\nb\nc\nd\ne", "a\nc\nd\nX\ne");
    expect(state.doc.toString()).toBe("a\nc\nd\ne");
    expect(slotSummary(state).total).toBe(2);
    state = toggle(state, 0, "b");
    state = toggle(state, 1, "a");
    expect(state.doc.toString()).toBe("a\nc\nd\ne");
    expect(slotSummary(state).resolved).toEqual([true, true]);
  });

  it("handles a chunk at line 1 and at EOF", () => {
    let { state } = setup("x\nm\ny", "X\nm\nY\n");
    expect(state.doc.toString()).toBe("m");
    state = toggle(state, 1, "b");
    state = toggle(state, 0, "a");
    expect(state.doc.toString()).toBe("x\nm\nY\n");
    state = toggle(state, 1, "b");
    expect(state.doc.toString()).toBe("x\nm");
  });
});

describe("hand edits", () => {
  const A = "one\ntwo\nthree\nfour\nfive";
  const B = "one\nTWO\nthree\nFOUR\nfive";

  it("marks a slot edited inside only", () => {
    let { state } = setup(A, B);
    state = toggle(state, 0, "a");
    state = toggle(state, 1, "b");
    expect(state.doc.toString()).toBe("one\ntwo\nthree\nFOUR\nfive");
    const inside = edit(state, 5, 5, "!");
    expect(inside.doc.toString()).toBe("one\nt!wo\nthree\nFOUR\nfive");
    expect(slots(inside).map((s) => s.handEdited)).toEqual([true, false]);
    const outside = edit(state, 9, 9, "!");
    expect(outside.doc.toString()).toBe("one\ntwo\nt!hree\nFOUR\nfive");
    expect(slots(outside).map((s) => s.handEdited)).toEqual([false, false]);
  });

  it("toggle after a hand edit overwrites only that slot", () => {
    let { state } = setup(A, B);
    state = toggle(state, 0, "a");
    state = toggle(state, 1, "a");
    state = edit(state, 4, 7, "2");
    state = edit(state, 0, 3, "ONE");
    state = edit(state, 12, 16, "4!");
    expect(state.doc.toString()).toBe("ONE\n2\nthree\n4!\nfive");
    expect(slots(state).map((s) => s.handEdited)).toEqual([true, true]);
    state = toggle(state, 0, "b");
    expect(state.doc.toString()).toBe("ONE\ntwo\nTWO\nthree\n4!\nfive");
    expect(slots(state)[0].handEdited).toBe(false);
    expect(slots(state)[1].handEdited).toBe(true);
    state = toggle(state, 1, "b");
    expect(state.doc.toString()).toBe(
      "ONE\ntwo\nTWO\nthree\nfour\nFOUR\nfive",
    );
  });

  it("typing into an empty slot fills it and marks it resolved", () => {
    let { state } = setup(A, B);
    expect(state.doc.toString()).toBe("one\nthree\nfive");
    state = edit(state, 4, 4, "mine\n");
    expect(state.doc.toString()).toBe("one\nmine\nthree\nfive");
    expect(slotSummary(state).resolved).toEqual([true, false]);
    state = toggle(state, 0, "b");
    expect(state.doc.toString()).toBe("one\nTWO\nthree\nfive");
  });

  it("keeps the separator when typing a line then Enter", () => {
    let { state } = setup(A, B);
    state = edit(state, 4, 4, "mine");
    state = edit(state, 8, 8, "\n");
    expect(state.doc.toString()).toBe("one\nmine\nthree\nfive");
    state = toggle(state, 0, "b");
    expect(state.doc.toString()).toBe("one\nTWO\nthree\nfive");
  });

  it("keeps a new line typed at the end of a slot", () => {
    let { state } = setup(A, B);
    state = toggle(state, 0, "a");
    state = edit(state, 7, 7, "\n");
    state = edit(state, 8, 8, "more");
    expect(state.doc.toString()).toBe("one\ntwo\nmore\nthree\nfive");
    expect(state.doc.sliceString(slots(state)[0].from, slots(state)[0].to))
      .toBe("two\nmore");
  });

  it("refills a slot whose lines were deleted", () => {
    let { state } = setup(A, B);
    state = toggle(state, 0, "a");
    state = edit(state, 4, 8, "");
    expect(state.doc.toString()).toBe("one\nthree\nfive");
    state = toggle(state, 0, "b");
    expect(state.doc.toString()).toBe("one\ntwo\nTWO\nthree\nfive");
  });

  it("typing into an empty EOF slot fills it", () => {
    let { state } = setup("a\nb", "a\nc");
    expect(state.doc.toString()).toBe("a");
    state = edit(state, 1, 1, "\nmine");
    expect(slotSummary(state).resolved).toEqual([true]);
    state = toggle(state, 0, "a");
    expect(state.doc.toString()).toBe("a\nb");
    state = toggle(state, 0, "a");
    expect(state.doc.toString()).toBe("a");
  });

  it("toggling after typing at the next line start keeps lines apart", () => {
    let { state } = setup(A, B);
    state = edit(state, 4, 4, "x");
    expect(state.doc.toString()).toBe("one\nxthree\nfive");
    state = toggle(state, 0, "b");
    expect(state.doc.toString()).toBe("one\nTWO\nthree\nfive");
    expect(state.doc.sliceString(slots(state)[0].from, slots(state)[0].to))
      .toBe("TWO");
  });

  it("toggling after typing into an empty EOF slot keeps lines apart", () => {
    let { state } = setup("a\nb", "a\nc");
    state = edit(state, 1, 1, "mine");
    expect(state.doc.toString()).toBe("amine");
    state = toggle(state, 0, "a");
    expect(state.doc.toString()).toBe("a\nb");
    expect(state.doc.sliceString(slots(state)[0].from, slots(state)[0].to))
      .toBe("b");
    state = toggle(state, 0, "a");
    expect(state.doc.toString()).toBe("a");
  });
});

describe("resultPosForA", () => {
  const A = "one\ntwo\nthree\nfour\nfive";
  const B = "one\nTWO\nthree\nFOUR\nfive";

  it("maps before, inside and after chunks", () => {
    const { chunks, state } = setup(A, B);
    const aDoc = doc(A);
    expect(state.doc.toString()).toBe("one\nthree\nfive");
    expect(resultPosForA(state, aDoc, chunks, 1)).toBe(1);
    expect(resultPosForA(state, aDoc, chunks, 3)).toBe(3);
    expect(resultPosForA(state, aDoc, chunks, 5)).toBe(4);
    expect(resultPosForA(state, aDoc, chunks, 9)).toBe(5);
    expect(resultPosForA(state, aDoc, chunks, 16)).toBe(10);
    expect(resultPosForA(state, aDoc, chunks, 20)).toBe(11);
    expect(resultPosForA(state, aDoc, chunks, A.length)).toBe(14);

    const picked = toggle(state, 0, "b");
    expect(resultPosForA(picked, aDoc, chunks, 9)).toBe(9);
    expect(resultPosForA(picked, aDoc, chunks, 20)).toBe(15);
  });
});

describe("chunkAtHeight", () => {
  it("returns the first chunk below h, the last past the end, -1 when none", () => {
    const tops = [0, 100, 200];
    const bottoms = [50, 150, 250];
    expect(chunkAtHeight([], [], 10)).toBe(-1);
    expect(chunkAtHeight(tops, bottoms, 0)).toBe(0);
    expect(chunkAtHeight(tops, bottoms, 50)).toBe(1);
    expect(chunkAtHeight(tops, bottoms, 149)).toBe(1);
    expect(chunkAtHeight(tops, bottoms, 1000)).toBe(2);
  });
});
