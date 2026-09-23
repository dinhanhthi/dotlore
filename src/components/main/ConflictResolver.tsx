import { MergeView } from "@codemirror/merge";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { Maximize2, Minimize2 } from "lucide-react";
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";

import { Button } from "@/components/ui/button";
import { editorExtensions, viewerExtensions } from "@/lib/cm";
import {
  BLOCKED,
  closeResolution,
  openResolution,
  resolveBinary,
  resolveConflict,
} from "@/lib/ipc";
import { useRoots } from "@/lib/roots";
import type { ResolutionDto, ResolveResultDto, SiblingDto } from "@/lib/types";
import { cn } from "@/lib/utils";

const RESULT_DEFAULT = 180;
const RESULT_MIN = 80;

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
  const { locked, refreshRoots } = useRoots();
  const [resolving, setResolving] = useState(false);
  const [dto, setDto] = useState<ResolutionDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [siblingIndex, setSiblingIndex] = useState(0);
  const [discard, setDiscard] = useState<Record<string, boolean>>({});
  const [resultHeight, setResultHeight] = useState(RESULT_DEFAULT);
  const [seed, setSeed] = useState<{ key: string; text: string } | null>(null);
  const [expanded, setExpanded] = useState(false);

  const mergeParentRef = useRef<HTMLDivElement>(null);
  const resultParentRef = useRef<HTMLDivElement>(null);
  const resultViewRef = useRef<EditorView | null>(null);

  useEffect(() => {
    setDto(null);
    setError(null);
    setNotice(null);
    setSiblingIndex(0);
    setDiscard({});
    setSeed(null);
  }, [slug, rel]);

  useEffect(() => {
    let cancelled = false;
    void openResolution(slug, rel)
      .then((resolution) => {
        if (cancelled) return;
        setDto(resolution);
        setSeed((current) => {
          const key = `${slug}:${rel}`;
          return current?.key === key
            ? current
            : { key, text: resolution.live_text ?? "" };
        });
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
    const parent = mergeParentRef.current;
    if (!parent || !dto || dto.binary) return;
    const view = new MergeView({
      a: {
        doc: dto.live_text ?? "",
        extensions: [...viewerExtensions(rel), EditorView.lineWrapping],
      },
      b: {
        doc: sibling?.text ?? "",
        extensions: [...viewerExtensions(rel), EditorView.lineWrapping],
      },
      parent,
      highlightChanges: true,
      gutter: true,
    });
    return () => view.destroy();
  }, [dto, rel, sibling]);

  useEffect(() => {
    const parent = resultParentRef.current;
    if (!parent || !seed) return;
    const view = new EditorView({
      state: EditorState.create({
        doc: seed.text,
        extensions: editorExtensions(rel),
      }),
      parent,
    });
    resultViewRef.current = view;
    return () => {
      view.destroy();
      resultViewRef.current = null;
    };
  }, [rel, seed]);

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
      onClose();
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
              label="ON THIS MAC"
              device="this Mac"
              bytes={dto.live_bytes_len}
              actionLabel="Keep this Mac"
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
            <div className="px-pad-x py-1">ON THIS MAC</div>
            <div className="border-l border-border px-pad-x py-1">FROM CLOUD</div>
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
          <div className="shrink-0 px-pad-x py-1 text-label text-muted-foreground">
            Result
          </div>
          <div ref={resultParentRef} className="min-h-0 flex-1 overflow-hidden" />
        </div>
      ) : null}

      <footer className="flex shrink-0 flex-col gap-1.5 border-t border-border px-pad-x py-2">
        {!dto?.binary ? (
          <>
            {sibling ? (
              <label className="flex items-center gap-2 text-foreground">
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
            ) : null}
            <p className="text-sm text-muted-foreground">
              Unchecked siblings stay as files next to {liveName(rel)}.
            </p>
          </>
        ) : (
          <p className="text-sm text-muted-foreground">
            Keeping one side discards every sibling of {liveName(rel)}.
          </p>
        )}
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="sm" onClick={onClose} disabled={resolving}>
            Cancel
          </Button>
          {!dto?.binary ? (
            <Button
              size="sm"
              onClick={() => {
                void handleResolve();
              }}
              disabled={locked || !dto}
            >
              Resolve
            </Button>
          ) : null}
        </div>
      </footer>
    </div>
  );
}
