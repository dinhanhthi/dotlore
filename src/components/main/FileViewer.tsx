import { Compartment, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { WrapText } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";

import { FileHeaderActions } from "@/components/layout/RootActions";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { viewerExtensions } from "@/lib/cm";
import { readFile } from "@/lib/ipc";
import { composeLivePath } from "@/lib/path";
import { useRoots } from "@/lib/roots";
import type { FileContent } from "@/lib/types";
import { cn } from "@/lib/utils";

const WORD_WRAP_KEY = "dotlore.wordWrap";

function readWordWrap(): boolean {
  try {
    return localStorage.getItem(WORD_WRAP_KEY) === "1";
  } catch {
    return false;
  }
}

function writeWordWrap(on: boolean): void {
  try {
    localStorage.setItem(WORD_WRAP_KEY, on ? "1" : "0");
  } catch {
    // Quota or private-mode.
  }
}

type FileViewerProps = {
  slug: string;
  rel: string;
};

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Could not read file";
}

function ReadOnlyEditor({
  rel,
  text,
  wrap,
}: {
  rel: string;
  text: string;
  wrap: boolean;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const wrapSlot = useRef(new Compartment());
  const wrapRef = useRef(wrap);
  wrapRef.current = wrap;

  useEffect(() => {
    const parent = parentRef.current;
    if (!parent) return;
    const view = new EditorView({
      state: EditorState.create({
        doc: text,
        extensions: [
          ...viewerExtensions(rel),
          wrapSlot.current.of(wrapRef.current ? EditorView.lineWrapping : []),
        ],
      }),
      parent,
    });
    viewRef.current = view;
    return () => {
      viewRef.current = null;
      view.destroy();
    };
  }, [rel, text]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: wrapSlot.current.reconfigure(wrap ? EditorView.lineWrapping : []),
    });
  }, [wrap]);

  return <div ref={parentRef} className="min-h-0 flex-1 overflow-hidden" />;
}

function WordWrapButton({
  pressed,
  disabled,
  onToggle,
}: {
  pressed: boolean;
  disabled: boolean;
  onToggle: () => void;
}) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            className={cn(
              "text-muted-foreground",
              pressed && "bg-muted text-foreground",
            )}
            disabled={disabled}
            aria-label="Word wrap"
            aria-pressed={pressed}
            onClick={onToggle}
          />
        }
      >
        <WrapText className="size-3.5" aria-hidden />
      </TooltipTrigger>
      <TooltipContent>Word wrap</TooltipContent>
    </Tooltip>
  );
}

export function FileViewer({ slug, rel }: FileViewerProps) {
  const { roots } = useRoots();
  const root = roots.find((row) => row.slug === slug) ?? null;

  const [content, setContent] = useState<FileContent | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [wrap, setWrap] = useState(readWordWrap);

  useEffect(() => {
    let cancelled = false;
    setContent(null);
    setError(null);
    void readFile(slug, rel)
      .then((file) => {
        if (!cancelled) setContent(file);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(errorMessage(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [slug, rel]);

  const livePath = root ? composeLivePath(root.path, rel) : null;

  let body: ReactNode;
  if (error) {
    body = <Message>{error}</Message>;
  } else if (!content) {
    body = <FileContentSkeleton />;
  } else if (content.too_large) {
    body = (
      <Message>{`File too large to preview (${content.bytes_len} bytes)`}</Message>
    );
  } else if (content.binary) {
    body = <Message>{`Binary file — ${content.bytes_len} bytes`}</Message>;
  } else if (content.text !== null) {
    body = <ReadOnlyEditor rel={rel} text={content.text} wrap={wrap} />;
  } else {
    body = <Message>Could not preview this file.</Message>;
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex h-row shrink-0 items-center gap-3 border-b border-border px-4">
        <span className="min-w-0 flex-1 truncate font-mono text-xs" title={rel}>
          {rel}
        </span>
        <span className="shrink-0 tabular-nums text-xs text-muted-foreground">
          {content ? formatBytes(content.bytes_len) : ""}
        </span>
        <div className="flex shrink-0 items-center gap-0.5">
          <WordWrapButton
            pressed={wrap}
            disabled={content?.text == null}
            onToggle={() => {
              setWrap((current) => {
                const next = !current;
                writeWordWrap(next);
                return next;
              });
            }}
          />
          <FileHeaderActions path={livePath} />
        </div>
      </header>
      {body}
    </div>
  );
}

const LINE_WIDTHS = [
  "92%",
  "78%",
  "88%",
  "64%",
  "96%",
  "71%",
  "84%",
  "55%",
  "90%",
  "68%",
  "76%",
  "48%",
];

function FileContentSkeleton() {
  return (
    <div
      role="status"
      aria-busy="true"
      aria-label="Loading file"
      className="flex min-h-0 flex-1 flex-col gap-3 overflow-hidden px-6 py-4"
    >
      {LINE_WIDTHS.map((width, index) => (
        <div
          key={index}
          aria-hidden
          className="h-3 motion-safe:animate-pulse rounded-2xl bg-muted"
          style={{ width }}
        />
      ))}
    </div>
  );
}

function Message({ children }: { children: ReactNode }) {
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center px-6">
      <p className="text-center text-muted-foreground">{children}</p>
    </div>
  );
}
