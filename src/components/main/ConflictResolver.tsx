import { Chunk, MergeView } from "@codemirror/merge";
import { EditorState, Text } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { ChevronDown, ChevronUp, CircleHelp, Maximize2, Minimize2 } from "lucide-react";
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";

import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { editorExtensions, viewerExtensions } from "@/lib/cm";
import {
  BLOCKED,
  closeResolution,
  openResolution,
  resolveBinary,
  resolveConflict,
} from "@/lib/ipc";
import { closeClean } from "@/lib/leave-guard";
import {
  chunkToggleGutter,
  resultDecorations,
  setCurrentChunk,
  setSidePicks,
} from "@/lib/merge-controls";
import {
  assembleResult,
  chunkAtHeight,
  keepAll,
  resultPosForA,
  resultSlots,
  type Side,
  slotSummary,
  toggleSlot,
} from "@/lib/merge-result";
import { useRoots } from "@/lib/roots";
import type { ResolutionDto, ResolveResultDto, SiblingDto } from "@/lib/types";
import { cn } from "@/lib/utils";

const RESULT_DEFAULT = 180;
const RESULT_MIN = 80;
/** Quiet period after ↑/↓ before scroll events may recompute `current`. */
const NAV_SETTLE_MS = 150;

type Summary = ReturnType<typeof slotSummary>;

const EMPTY_SUMMARY: Summary = { total: 0, resolved: [], picks: [] };

type ConflictResolverProps = {
  slug: string;
  rel: string;
  onClose: () => void;
};

function liveName(rel: string): string {
  return rel.replace(/\\/g, "/").split("/").pop() ?? rel;
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

function BinaryCard({
  label,
  device,
  bytes,
  actionLabel,
  disabled,
  onKeep,
}: {
  label: string;
  device: string;
  bytes: number;
  actionLabel: string;
  disabled: boolean;
  onKeep: () => void;
}) {
  return (
    <div className="flex min-h-[160px] flex-1 flex-col justify-between rounded-2xl bg-card p-4 ring-1 ring-foreground/5">
      <div className="flex flex-col gap-1">
        <span className="text-label text-muted-foreground">{label}</span>
        <span className="text-foreground">{device}</span>
        <span className="tabular-nums text-muted-foreground">
          {formatBytes(bytes)}
        </span>
      </div>
      <Button
        size="sm"
        className="mt-4 self-start"
        disabled={disabled}
        onClick={onKeep}
      >
        {actionLabel}
      </Button>
    </div>
  );
}

function errorMessage(error: unknown): string {
  if (typeof error === "string" && error.length > 0) return error;
  if (error instanceof Error && error.message.length > 0) return error.message;
  return "Could not open conflict";
}

export function ConflictResolver({ slug, rel, onClose }: ConflictResolverProps) {
  const { locked, refreshRoots, setResolverDirty } = useRoots();
  const [resolving, setResolving] = useState(false);
  const [dto, setDto] = useState<ResolutionDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [siblingIndex, setSiblingIndex] = useState(0);
  const [discard, setDiscard] = useState<Record<string, boolean>>({});
  const [resultHeight, setResultHeight] = useState(RESULT_DEFAULT);
  const [summary, setSummary] = useState<Summary>(EMPTY_SUMMARY);
  const [expanded, setExpanded] = useState(false);
  const [current, setCurrent] = useState(0);

  const mergeParentRef = useRef<HTMLDivElement>(null);
  const resultParentRef = useRef<HTMLDivElement>(null);
  const mergeViewRef = useRef<MergeView | null>(null);
  const resultViewRef = useRef<EditorView | null>(null);
  const initialResultRef = useRef("");
  const currentRef = useRef(0);
  const navigateRef = useRef<((index: number) => void) | null>(null);

  useEffect(() => {
    setDto(null);
    setError(null);
    setNotice(null);
    setSiblingIndex(0);
    setDiscard({});
    setSummary(EMPTY_SUMMARY);
  }, [slug, rel]);

  useEffect(() => {
    let cancelled = false;
    void openResolution(slug, rel)
      .then((resolution) => {
        if (cancelled) return;
        setDto(resolution);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(errorMessage(cause));
      });
    return () => {
      cancelled = true;
      void closeResolution(slug, rel);
    };
  }, [slug, rel]);

  useEffect(() => {
    if (!dto) return;
    setDiscard((current) => {
      const next = { ...current };
      for (const sibling of dto.siblings) {
        if (next[sibling.path] === undefined) next[sibling.path] = true;
      }
      return next;
    });
  }, [dto]);

  const sibling: SiblingDto | undefined = dto?.siblings[siblingIndex] ?? dto?.siblings[0];

  useEffect(() => {
    const mergeParent = mergeParentRef.current;
    const resultParent = resultParentRef.current;
    if (!mergeParent || !resultParent || !dto || dto.binary) return;
    const live = dto.live_text ?? "";
    const other = sibling?.text ?? "";
    // Both sides get the same `Text` as `assembleResult` (split on "\n" only),
    // so these chunks match `view.chunks` even for CRLF files.
    const aDoc = Text.of(live.split("\n"));
    const bDoc = Text.of(other.split("\n"));
    const chunks = Chunk.build(aDoc, bDoc);
    const onToggle = (side: Side) => (index: number) => {
      const result = resultViewRef.current;
      if (result) result.dispatch(toggleSlot(result.state, index, side));
    };
    const view = new MergeView({
      a: {
        doc: aDoc,
        extensions: [
          ...viewerExtensions(rel),
          EditorView.lineWrapping,
          chunkToggleGutter("a", chunks, onToggle("a")),
        ],
      },
      b: {
        doc: bDoc,
        extensions: [
          ...viewerExtensions(rel),
          EditorView.lineWrapping,
          chunkToggleGutter("b", chunks, onToggle("b")),
        ],
      },
      parent: mergeParent,
      highlightChanges: true,
      gutter: true,
    });
    mergeViewRef.current = view;

    const { text, slots } = assembleResult(live, other, chunks, []);
    initialResultRef.current = text;
    let dirty = false;
    setResolverDirty(false);
    const result = new EditorView({
      state: EditorState.create({
        doc: text,
        extensions: [
          editorExtensions(rel),
          resultSlots.init(() => slots),
          resultDecorations(),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) {
              const next = update.state.doc.toString() !== initialResultRef.current;
              if (next !== dirty) {
                dirty = next;
                setResolverDirty(next);
              }
            }
            if (update.state.field(resultSlots) === update.startState.field(resultSlots)) {
              return;
            }
            const next = slotSummary(update.state);
            setSummary(next);
            for (const side of ["a", "b"] as const) {
              view[side].dispatch({
                effects: setSidePicks.of(next.picks.map((p) => p.includes(side))),
              });
            }
          }),
        ],
      }),
      parent: resultParent,
    });
    resultViewRef.current = result;
    setSummary(slotSummary(result.state));
    currentRef.current = 0;
    setCurrent(0);

    // The two top panes scroll as one unit inside `.cm-mergeView`.
    const container = mergeParent.querySelector<HTMLElement>(".cm-mergeView");
    const a = view.a;
    let frame = 0;
    let suppress = false;
    let settleTimer: ReturnType<typeof setTimeout> | undefined;

    function scrolledHeight(el: HTMLElement): number {
      return Math.max(0, el.getBoundingClientRect().top - a.documentTop);
    }

    function currentAt(el: HTMLElement, h: number): number {
      const len = aDoc.length;
      const tops = chunks.map((c) => a.lineBlockAt(Math.min(c.fromA, len)).top);
      const bottoms = chunks.map(
        (c) => a.lineBlockAt(Math.min(Math.max(c.fromA, c.toA - 1), len)).bottom,
      );
      const index = chunkAtHeight(tops, bottoms, h);
      // At the bottom, chunks that cannot reach the top stay current while
      // they are still on screen (e.g. after ↓ to the last chunk).
      const prev = currentRef.current;
      const atBottom = el.scrollTop + el.clientHeight >= el.scrollHeight - 1;
      const prevVisible = tops[prev] !== undefined && tops[prev] < h + el.clientHeight;
      if (atBottom && index < prev && prevVisible) return prev;
      return index;
    }

    function sync() {
      frame = 0;
      if (!container || suppress) return;
      const h = scrolledHeight(container);
      const index = currentAt(container, h);
      if (index >= 0) setCurrent(index);
      const pos = resultPosForA(result.state, aDoc, chunks, a.lineBlockAtHeight(h).from);
      result.scrollDOM.scrollTop = result.lineBlockAt(pos).top;
    }

    function settle() {
      clearTimeout(settleTimer);
      settleTimer = setTimeout(() => {
        suppress = false;
      }, NAV_SETTLE_MS);
    }

    function onScroll() {
      // Programmatic scrolls from ↑/↓ keep pushing the quiet period out.
      if (suppress) {
        settle();
        return;
      }
      if (!frame) frame = requestAnimationFrame(sync);
    }

    navigateRef.current = (index: number) => {
      const chunk = chunks[index];
      const slot = result.state.field(resultSlots)[index];
      if (!container || !chunk || !slot) return;
      currentRef.current = index;
      setCurrent(index);
      suppress = true;
      if (frame) {
        cancelAnimationFrame(frame);
        frame = 0;
      }
      settle();
      const top = a.lineBlockAt(Math.min(chunk.fromA, aDoc.length)).top;
      container.scrollTop += top + a.documentTop - container.getBoundingClientRect().top;
      // `lineBlockAt` includes an empty slot's placeholder widget above the line.
      result.scrollDOM.scrollTop = result.lineBlockAt(slot.from).top;
    };

    container?.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      container?.removeEventListener("scroll", onScroll);
      if (frame) cancelAnimationFrame(frame);
      clearTimeout(settleTimer);
      navigateRef.current = null;
      result.destroy();
      view.destroy();
      resultViewRef.current = null;
      mergeViewRef.current = null;
      setResolverDirty(false);
    };
  }, [dto, rel, sibling, setResolverDirty]);

  useEffect(() => {
    currentRef.current = current;
    resultViewRef.current?.dispatch({ effects: setCurrentChunk.of(current) });
  }, [current]);

  function handleKeepAll(side: Side) {
    const result = resultViewRef.current;
    if (result) result.dispatch(keepAll(result.state, side));
  }

  function onResizePointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    event.preventDefault();
    const startY = event.clientY;
    const startH = resultHeight;
    function move(ev: PointerEvent) {
      setResultHeight(Math.max(RESULT_MIN, startH + (startY - ev.clientY)));
    }
    function up() {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    }
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  async function applyOutcome(result: ResolveResultDto) {
    if (result.outcome === "applied") {
      await refreshRoots().catch(() => {
        // Tree/status still refresh from the status event.
      });
      // Resolved: nothing left to discard, so `onClose` must not ask.
      closeClean(setResolverDirty, onClose);
      return;
    }
    if (result.outcome === "stale") {
      setDto(result.refreshed);
      setNotice(
        "Refreshed — the file changed on another device. Review and resolve again.",
      );
      return;
    }
    setNotice("Sync has not finished yet. Try again in a moment.");
  }

  async function handleResolve() {
    if (!dto || dto.binary) return;
    const content =
      resultViewRef.current?.state.doc.toString() ?? dto.live_text ?? "";
    const discardSiblings = dto.siblings
      .filter((item) => discard[item.path] !== false)
      .map((item) => item.path);
    setResolving(true);
    try {
      const result = await resolveConflict(slug, rel, discardSiblings, content);
      if (result === BLOCKED) return;
      await applyOutcome(result);
    } catch {
      // Banner is set by `runTask()`.
    } finally {
      setResolving(false);
    }
  }

  async function handleBinary(keep: "live" | "other") {
    if (!dto || !dto.binary) return;
    if (keep === "other" && !sibling) return;
    setResolving(true);
    try {
      const result = await resolveBinary(
        slug,
        rel,
        keep,
        keep === "other" ? (sibling?.path ?? null) : null,
      );
      if (result === BLOCKED) return;
      await applyOutcome(result);
    } catch {
      // Banner is set by `runTask()`.
    } finally {
      setResolving(false);
    }
  }

  const unresolved = summary.resolved.filter((done) => !done).length;
  const total = summary.total;
  const checked = sibling ? discard[sibling.path] !== false : true;
  const device = sibling?.device_name ?? "other device";

  return (
    <div
      className={cn(
        "flex min-h-0 flex-col",
        expanded
          ? "fixed top-titlebar right-0 bottom-0 left-0 z-40 bg-background"
          : "h-full",
      )}
    >
      <header className="flex h-row shrink-0 items-center gap-2 border-b border-border px-pad-x">
        <span className="min-w-0 truncate font-mono text-xs" title={rel}>
          {rel}
        </span>
        <span className="shrink-0 text-muted-foreground">from {device}</span>
        <div className="ml-auto flex min-w-0 items-center gap-1">
          {dto && dto.siblings.length > 1
            ? dto.siblings.map((item, index) => (
                <button
                  key={item.path}
                  type="button"
                  onClick={() => setSiblingIndex(index)}
                  className={cn(
                    "h-6 shrink-0 rounded-4xl px-2.5 text-xs",
                    index === siblingIndex
                      ? "bg-muted text-foreground"
                      : "text-muted-foreground hover:text-foreground",
                  )}
                >
                  {item.device_name}
                </button>
              ))
            : null}
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={expanded ? "Exit full window" : "Expand to full window"}
            onClick={() => setExpanded((current) => !current)}
          >
            {expanded ? <Minimize2 /> : <Maximize2 />}
          </Button>
        </div>
      </header>

      {notice ? (
        <div
          role="status"
          className="shrink-0 border-b border-border bg-muted px-pad-x py-1.5 text-muted-foreground"
        >
          {notice}
        </div>
      ) : null}

      {error ? (
        <div className="flex min-h-0 flex-1 items-center justify-center px-6">
          <p className="text-center text-muted-foreground">{error}</p>
        </div>
      ) : !dto ? (
        <div className="flex min-h-0 flex-1 items-center justify-center px-6">
          <p className="text-center text-muted-foreground">Loading…</p>
        </div>
      ) : dto.binary ? (
        <div className="flex min-h-0 flex-1 items-center justify-center px-6">
          <div className="flex w-full max-w-lg gap-3">
            <BinaryCard
              label="THIS MACHINE"
              device="this machine"
              bytes={dto.live_bytes_len}
              actionLabel="Keep this machine"
              disabled={locked}
              onKeep={() => {
                void handleBinary("live");
              }}
            />
            <BinaryCard
              label="FROM CLOUD"
              device={device}
              bytes={sibling?.bytes_len ?? 0}
              actionLabel="Keep from cloud"
              disabled={locked || !sibling}
              onKeep={() => {
                void handleBinary("other");
              }}
            />
          </div>
        </div>
      ) : (
        <>
          <div className="grid shrink-0 grid-cols-2 border-b border-border text-label text-muted-foreground">
            <div className="flex items-center justify-between gap-2 px-pad-x py-1">
              THIS MACHINE
              <Button variant="ghost" size="xs" onClick={() => handleKeepAll("a")}>
                Keep all
              </Button>
            </div>
            <div className="flex items-center justify-between gap-2 border-l border-border px-pad-x py-1">
              FROM CLOUD
              <Button variant="ghost" size="xs" onClick={() => handleKeepAll("b")}>
                Keep all
              </Button>
            </div>
          </div>
          <div ref={mergeParentRef} className="cm-merge-host min-h-0 flex-1 overflow-hidden" />
        </>
      )}

      {!error && dto && !dto.binary ? (
        <div
          className="relative flex shrink-0 flex-col border-t border-border"
          style={{ height: resultHeight }}
        >
          <div
            role="separator"
            aria-orientation="horizontal"
            aria-label="Resize result editor"
            onPointerDown={onResizePointerDown}
            className="absolute inset-x-0 top-0 z-10 h-1.5 cursor-ns-resize"
          />
          <div className="grid shrink-0 grid-cols-[1fr_auto_1fr] items-center px-pad-x py-1 text-label text-muted-foreground">
            <span>Result</span>
            <div className="flex items-center gap-1">
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Previous conflict"
                disabled={total === 0 || current <= 0}
                onClick={() => navigateRef.current?.(current - 1)}
              >
                <ChevronUp />
              </Button>
              <span className="min-w-8 text-center tabular-nums">
                {total === 0 ? "0/0" : `${current + 1}/${total}`}
              </span>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Next conflict"
                disabled={total === 0 || current >= total - 1}
                onClick={() => navigateRef.current?.(current + 1)}
              >
                <ChevronDown />
              </Button>
            </div>
          </div>
          <div ref={resultParentRef} className="min-h-0 flex-1 overflow-hidden" />
        </div>
      ) : null}

      <footer className="flex shrink-0 items-center gap-2 border-t border-border px-pad-x py-2">
        {!dto?.binary ? (
          sibling ? (
            <div className="flex items-center gap-1">
              <label className="flex items-center gap-2 text-xs text-foreground">
                <input
                  type="checkbox"
                  checked={checked}
                  onChange={(event) => {
                    const path = sibling.path;
                    const next = event.target.checked;
                    setDiscard((current) => ({ ...current, [path]: next }));
                  }}
                  className="size-3.5 accent-primary"
                />
                Discard this sibling file
              </label>
              <Tooltip>
                <TooltipTrigger
                  render={
                    <Button
                      variant="ghost"
                      size="icon-xs"
                      className="text-muted-foreground"
                      aria-label="What is a sibling file?"
                    />
                  }
                >
                  <CircleHelp className="size-3.5" aria-hidden />
                </TooltipTrigger>
                <TooltipContent side="top" align="start" className="flex-col items-start gap-1.5">
                  <p>
                    Two devices changed {liveName(rel)}. The newer version stayed in{" "}
                    {liveName(rel)}; the other version was saved next to it as a sibling
                    file, {liveName(sibling.path)}.
                  </p>
                  <p>Checked: Resolve saves the Result and deletes the sibling file.</p>
                  <p>Unchecked: Resolve saves the Result and keeps the sibling file on disk.</p>
                </TooltipContent>
              </Tooltip>
            </div>
          ) : null
        ) : (
          <p className="text-sm text-muted-foreground">
            Keeping one side discards every sibling of {liveName(rel)}.
          </p>
        )}
        <div className="ml-auto flex items-center gap-2">
          <Button variant="ghost" size="xs" onClick={onClose} disabled={resolving}>
            Cancel
          </Button>
          {!dto?.binary ? (
            unresolved > 0 ? (
              <Tooltip>
                <TooltipTrigger render={<span className="inline-flex" />}>
                  <Button size="xs" disabled>
                    Resolve
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="top" align="end">
                  {unresolved === 1
                    ? "1 conflict still needs a side. Pick one above or edit it in Result."
                    : `${unresolved} conflicts still need a side. Pick one above or edit them in Result.`}
                </TooltipContent>
              </Tooltip>
            ) : (
              <Button
                size="xs"
                onClick={() => {
                  void handleResolve();
                }}
                disabled={locked || !dto || resolving}
              >
                Resolve
              </Button>
            )
          ) : null}
        </div>
      </footer>
    </div>
  );
}
