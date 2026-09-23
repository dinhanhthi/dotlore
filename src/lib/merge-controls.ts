import {
  type EditorState,
  type Extension,
  type Range,
  RangeSet,
  StateEffect,
  StateField,
} from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  gutter,
  GutterMarker,
  WidgetType,
} from "@codemirror/view";
import { type ChunkLike, resultSlots, type Side } from "@/lib/merge-result";

const SVG_NS = "http://www.w3.org/2000/svg";
// lucide `Plus` / `Check`.
const PLUS = ["M5 12h14", "M12 5v14"];
const CHECK = ["M20 6 9 17l-5-5"];

function icon(paths: string[]): SVGSVGElement {
  const svg = document.createElementNS(SVG_NS, "svg");
  const attrs: Record<string, string> = {
    width: "12",
    height: "12",
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    "stroke-width": "2",
    "stroke-linecap": "round",
    "stroke-linejoin": "round",
    "aria-hidden": "true",
  };
  for (const [k, v] of Object.entries(attrs)) svg.setAttribute(k, v);
  for (const d of paths) {
    const path = document.createElementNS(SVG_NS, "path");
    path.setAttribute("d", d);
    svg.appendChild(path);
  }
  return svg;
}

function toggleLabel(side: Side, picked: boolean): string {
  const what = side === "a" ? "this machine's lines" : "cloud lines";
  return `${picked ? "Remove" : "Add"} ${what}`;
}

/** Per-chunk "picked" flags for one MergeView side, pushed by the resolver. */
export const setSidePicks = StateEffect.define<boolean[]>();

export const sidePicks = StateField.define<boolean[]>({
  create: () => [],
  update(picks, tr) {
    for (const e of tr.effects) if (e.is(setSidePicks)) picks = e.value;
    return picks;
  },
});

class ToggleMarker extends GutterMarker {
  constructor(
    readonly side: Side,
    readonly index: number,
    readonly picked: boolean,
    readonly onToggle: (index: number) => void,
  ) {
    super();
  }

  eq(other: GutterMarker): boolean {
    return (
      other instanceof ToggleMarker &&
      other.index === this.index &&
      other.picked === this.picked
    );
  }

  toDOM(): Node {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "cm-chunk-toggle";
    btn.setAttribute("aria-pressed", String(this.picked));
    btn.setAttribute("aria-label", toggleLabel(this.side, this.picked));
    btn.appendChild(icon(this.picked ? CHECK : PLUS));
    // The side editors are read-only: keep focus where it is.
    btn.addEventListener("mousedown", (e) => e.preventDefault());
    btn.addEventListener("click", () => this.onToggle(this.index));
    return btn;
  }
}

/** A "+" / check toggle on the first line of every chunk of one MergeView side. */
export function chunkToggleGutter(
  side: Side,
  chunks: readonly ChunkLike[],
  onToggle: (index: number) => void,
): Extension {
  const build = (state: EditorState): RangeSet<GutterMarker> => {
    const picks = state.field(sidePicks);
    const len = state.doc.length;
    return RangeSet.of(
      chunks.map((c, i) => {
        const from = side === "a" ? c.fromA : c.fromB;
        const line = state.doc.lineAt(Math.min(from, len));
        return new ToggleMarker(side, i, picks[i] ?? false, onToggle).range(
          line.from,
        );
      }),
      true,
    );
  };
  const markers = StateField.define<RangeSet<GutterMarker>>({
    create: build,
    update(set, tr) {
      return tr.state.field(sidePicks) === tr.startState.field(sidePicks)
        ? set
        : build(tr.state);
    },
  });
  return [
    sidePicks,
    markers,
    gutter({
      class: "cm-chunk-toggle-gutter",
      markers: (view) => view.state.field(markers),
    }),
  ];
}

/** Index of the chunk the resolver considers current (scroll / ↑↓ nav). */
export const setCurrentChunk = StateEffect.define<number>();

const currentChunk = StateField.define<number>({
  create: () => 0,
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setCurrentChunk)) value = e.value;
    return value;
  },
});

class PlaceholderWidget extends WidgetType {
  constructor(
    readonly index: number,
    readonly total: number,
    readonly current: boolean,
    readonly resolved: boolean,
    readonly pos: number,
  ) {
    super();
  }

  eq(other: PlaceholderWidget): boolean {
    return (
      other.index === this.index &&
      other.total === this.total &&
      other.current === this.current &&
      other.resolved === this.resolved &&
      other.pos === this.pos
    );
  }

  toDOM(view: EditorView): HTMLElement {
    const el = document.createElement("div");
    el.className = this.current
      ? "cm-chunk-placeholder cm-chunk-placeholder-current"
      : "cm-chunk-placeholder";
    const hint = this.resolved ? "no lines kept" : "pick a side above";
    el.textContent = `Conflict ${this.index + 1} of ${this.total} · ${hint}`;
    el.addEventListener("mousedown", (e) => {
      e.preventDefault();
      view.dispatch({ selection: { anchor: this.pos } });
      view.focus();
    });
    return el;
  }
}

function buildDecorations(state: EditorState): DecorationSet {
  const slots = state.field(resultSlots);
  const current = state.field(currentChunk);
  const ranges: Range<Decoration>[] = [];
  slots.forEach((slot, i) => {
    const isCurrent = i === current;
    if (slot.empty) {
      const resolved = slot.picks.length > 0 || slot.handEdited;
      ranges.push(
        Decoration.widget({
          widget: new PlaceholderWidget(
            i,
            slots.length,
            isCurrent,
            resolved,
            slot.from,
          ),
          block: true,
          // An EOF slot sits at the end of the last line: draw it below.
          side: slot.sep === "before" ? 1 : -1,
        }).range(slot.from),
      );
      return;
    }
    if (!isCurrent) return;
    const first = state.doc.lineAt(slot.from).number;
    const last = state.doc.lineAt(slot.to).number;
    for (let n = first; n <= last; n++) {
      ranges.push(
        Decoration.line({ class: "cm-chunk-current" }).range(
          state.doc.line(n).from,
        ),
      );
    }
  });
  return Decoration.set(ranges, true);
}

const slotDecorations = StateField.define<DecorationSet>({
  create: buildDecorations,
  update(set, tr) {
    const changed =
      tr.state.field(resultSlots) !== tr.startState.field(resultSlots) ||
      tr.state.field(currentChunk) !== tr.startState.field(currentChunk);
    return changed ? buildDecorations(tr.state) : set.map(tr.changes);
  },
  provide: (f) => EditorView.decorations.from(f),
});

/**
 * Empty-slot placeholders and the current-slot highlight for the Result
 * editor. The caller adds `resultSlots.init(...)` next to this extension.
 */
export function resultDecorations(): Extension {
  return [currentChunk, slotDecorations];
}
