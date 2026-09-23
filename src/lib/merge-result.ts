import {
  type ChangeSpec,
  type EditorState,
  StateEffect,
  StateField,
  Text,
  type TransactionSpec,
} from "@codemirror/state";

export type Side = "a" | "b";

export type ChunkLike = {
  fromA: number;
  toA: number;
  fromB: number;
  toB: number;
};

/** Where the newline separating a slot from its neighbours lives. */
export type SlotSep = "after" | "before" | "none";

/**
 * One conflict chunk in the Result doc. `to` is the end of the last line
 * (newline excluded); `empty` means zero lines, so `from === to` alone is
 * ambiguous with a single empty line. `a` / `b` are the chunk's source lines.
 */
export type Slot = {
  from: number;
  to: number;
  picks: Side[];
  handEdited: boolean;
  empty: boolean;
  sep: SlotSep;
  a: readonly string[];
  b: readonly string[];
};

export function chunkLineRanges(
  doc: Text,
  from: number,
  to: number,
): { start: number; end: number } {
  const len = doc.length;
  const start = doc.lineAt(Math.min(from, len)).number - 1;
  const end = to > len ? doc.lines : doc.lineAt(to).number - 1;
  return { start, end };
}

function linesOf(
  side: { a: readonly string[]; b: readonly string[] },
  picks: Side[],
): string[] {
  return picks.flatMap((s) => side[s]);
}

export function assembleResult(
  a: string,
  b: string,
  chunks: readonly ChunkLike[],
  picks: Side[][],
): { text: string; slots: Slot[] } {
  const aLines = a.split("\n");
  const bLines = b.split("\n");
  const aDoc = Text.of(aLines);
  const bDoc = Text.of(bLines);
  const out: string[] = [];
  const spans: { start: number; count: number }[] = [];
  const sources: { a: string[]; b: string[]; sep: SlotSep }[] = [];
  let prev = 0;
  chunks.forEach((c, i) => {
    const ra = chunkLineRanges(aDoc, c.fromA, c.toA);
    const rb = chunkLineRanges(bDoc, c.fromB, c.toB);
    out.push(...aLines.slice(prev, ra.start));
    const sep: SlotSep =
      ra.end < aLines.length ? "after" : ra.start > 0 ? "before" : "none";
    const src = {
      a: aLines.slice(ra.start, ra.end),
      b: bLines.slice(rb.start, rb.end),
      sep,
    };
    const lines = linesOf(src, picks[i] ?? []);
    spans.push({ start: out.length, count: lines.length });
    sources.push(src);
    out.push(...lines);
    prev = ra.end;
  });
  out.push(...aLines.slice(prev));

  const offsets: number[] = [];
  let pos = 0;
  for (const line of out) {
    offsets.push(pos);
    pos += line.length + 1;
  }
  offsets.push(pos);
  const text = out.join("\n");

  const slots = spans.map(({ start, count }, i): Slot => {
    const src = sources[i];
    const base = { picks: [...(picks[i] ?? [])], handEdited: false, ...src };
    if (count === 0) {
      const at = src.sep === "after" ? offsets[start] : text.length;
      return { ...base, from: at, to: at, empty: true };
    }
    const last = start + count - 1;
    return {
      ...base,
      from: offsets[start],
      to: offsets[last] + out[last].length,
      empty: false,
    };
  });
  return { text, slots };
}

/** Replaces a slot's lines; `from` / `to` is the new range after this change alone. */
export function replaceSlotSpec(
  doc: Text,
  slot: Slot,
  lines: string[],
): {
  changes: { from: number; to: number; insert: string };
  from: number;
  to: number;
} {
  const content = lines.join("\n");
  const none = lines.length === 0;
  let from = slot.from;
  let to = slot.to;
  let insert = content;
  let start = from;
  if (slot.sep === "after") {
    if (slot.empty) insert = none ? "" : content + "\n";
    else if (none && doc.sliceString(to, to + 1) === "\n") to += 1;
    else if (!none && to < doc.length && doc.sliceString(to, to + 1) !== "\n") {
      insert = content + "\n";
    }
  } else if (slot.sep === "before") {
    if (slot.empty) {
      insert = none ? "" : "\n" + content;
      start = none ? from : from + 1;
    } else if (none && doc.sliceString(from - 1, from) === "\n") {
      from -= 1;
      start = from;
    } else if (!none && from > 0 && doc.sliceString(from - 1, from) !== "\n") {
      insert = "\n" + content;
      start = from + 1;
    }
  }
  const end = none ? start : start + content.length;
  return { changes: { from, to, insert }, from: start, to: end };
}

export const setSlotEffect = StateEffect.define<{ index: number; slot: Slot }>();
export const keepAllEffect = StateEffect.define<Slot[]>();
/** Replaces every slot, e.g. after the caller rebuilt the Result doc. */
export const resetEffect = StateEffect.define<Slot[]>();

function touches(slot: Slot, fromA: number, toA: number): boolean {
  if (slot.empty) return fromA <= slot.from && toA >= slot.from;
  return toA >= slot.from && fromA <= slot.to;
}

// Initial value: `resultSlots.init(() => assembleResult(...).slots)`.
export const resultSlots = StateField.define<Slot[]>({
  create: () => [],
  update(slots, tr) {
    for (const e of tr.effects) {
      if (e.is(resetEffect) || e.is(keepAllEffect)) return e.value;
    }
    const set = new Map<number, Slot>();
    for (const e of tr.effects) {
      if (e.is(setSlotEffect)) set.set(e.value.index, e.value.slot);
    }
    if (!tr.docChanged && set.size === 0) return slots;
    return slots.map((slot, i) => {
      const next = set.get(i);
      if (next) return next;
      let from = tr.changes.mapPos(slot.from, -1);
      let to = tr.changes.mapPos(slot.to, 1);
      if (set.size > 0) return { ...slot, from, to };
      let touched = false;
      tr.changes.iterChangedRanges((fromA, toA) => {
        if (touches(slot, fromA, toA)) touched = true;
      });
      if (!touched) return { ...slot, from, to };
      // A newline the edit pushed to the slot edge is its separator when the
      // slot was just filled or has no other separator next to it.
      const doc = tr.newDoc;
      const at = (pos: number) => doc.sliceString(pos, pos + 1);
      const filled = slot.empty && to > from;
      let empty = !filled && to === from;
      if (slot.sep === "after") {
        if (to > from && at(to - 1) === "\n" && (filled || at(to) !== "\n")) {
          to -= 1;
        }
        empty = !filled && to === from && (slot.empty || at(from) !== "\n");
      } else if (slot.sep === "before") {
        const lone = () => from === 0 || at(from - 1) !== "\n";
        if (to > from && at(from) === "\n" && (filled || lone())) from += 1;
        empty = !filled && to === from && (slot.empty || lone());
      }
      return { ...slot, from, to, empty, handEdited: true };
    });
  },
});

export function toggleSlot(
  state: EditorState,
  index: number,
  side: Side,
): TransactionSpec {
  const slot = state.field(resultSlots)[index];
  const picks = slot.picks.includes(side)
    ? slot.picks.filter((s) => s !== side)
    : [...slot.picks, side];
  const lines = linesOf(slot, picks);
  const r = replaceSlotSpec(state.doc, slot, lines);
  return {
    changes: r.changes,
    effects: setSlotEffect.of({
      index,
      slot: {
        ...slot,
        from: r.from,
        to: r.to,
        picks,
        handEdited: false,
        empty: lines.length === 0,
      },
    }),
  };
}

export function keepAll(state: EditorState, side: Side): TransactionSpec {
  const changes: ChangeSpec[] = [];
  const next: Slot[] = [];
  let delta = 0;
  for (const slot of state.field(resultSlots)) {
    const lines = [...slot[side]];
    const r = replaceSlotSpec(state.doc, slot, lines);
    changes.push(r.changes);
    next.push({
      ...slot,
      from: r.from + delta,
      to: r.to + delta,
      picks: [side],
      handEdited: false,
      empty: lines.length === 0,
    });
    delta += r.changes.insert.length - (r.changes.to - r.changes.from);
  }
  return { changes, effects: keepAllEffect.of(next) };
}

export function slotSummary(state: EditorState): {
  total: number;
  resolved: boolean[];
  picks: Side[][];
} {
  const slots = state.field(resultSlots);
  return {
    total: slots.length,
    resolved: slots.map((s) => s.picks.length > 0 || s.handEdited),
    picks: slots.map((s) => s.picks),
  };
}

export function resultPosForA(
  state: EditorState,
  aDoc: Text,
  chunks: readonly ChunkLike[],
  aPos: number,
): number {
  const slots = state.field(resultSlots);
  const len = state.doc.length;
  const pos = Math.max(0, Math.min(aPos, aDoc.length));
  let prevToA = 0;
  let base = 0;
  for (let i = 0; i < chunks.length && i < slots.length; i++) {
    const c = chunks[i];
    const slot = slots[i];
    if (pos < c.fromA) return Math.min(base + pos - prevToA, slot.from, len);
    if (pos < c.toA) return slot.from;
    prevToA = c.toA;
    base = slot.empty ? slot.to : slot.to + 1;
  }
  return Math.min(base + pos - prevToA, len);
}

export function chunkAtHeight(
  tops: number[],
  bottoms: number[],
  h: number,
): number {
  const n = Math.min(tops.length, bottoms.length);
  if (n === 0) return -1;
  for (let i = 0; i < n; i++) {
    if (bottoms[i] > h) return i;
  }
  return n - 1;
}
