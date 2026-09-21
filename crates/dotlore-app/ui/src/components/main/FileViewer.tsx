import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { useEffect, useRef, useState, type ReactNode } from "react";

import { RevealInFinderButton } from "@/components/layout/RevealInFinderButton";
import { viewerExtensions } from "@/lib/cm";
import { readFile } from "@/lib/ipc";
import { composeLivePath } from "@/lib/path";
import { useRoots } from "@/lib/roots";
import type { FileContent } from "@/lib/types";

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

function ReadOnlyEditor({ rel, text }: { rel: string; text: string }) {
  const parentRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const parent = parentRef.current;
    if (!parent) return;
    const view = new EditorView({
      state: EditorState.create({
        doc: text,
        extensions: viewerExtensions(rel),
      }),
      parent,
    });
    return () => view.destroy();
  }, [rel, text]);

  return <div ref={parentRef} className="min-h-0 flex-1 overflow-hidden" />;
}

export function FileViewer({ slug, rel }: FileViewerProps) {
  const { roots } = useRoots();
  const root = roots.find((row) => row.slug === slug) ?? null;

  const [content, setContent] = useState<FileContent | null>(null);
  const [error, setError] = useState<string | null>(null);

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
    body = <Message>Loading…</Message>;
  } else if (content.too_large) {
    body = (
      <Message>{`File too large to preview (${content.bytes_len} bytes)`}</Message>
    );
  } else if (content.binary) {
    body = <Message>{`Binary file — ${content.bytes_len} bytes`}</Message>;
  } else if (content.text !== null) {
    body = <ReadOnlyEditor rel={rel} text={content.text} />;
  } else {
    body = <Message>Could not preview this file.</Message>;
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex h-row shrink-0 items-center gap-3 border-b border-border px-4">
        <span className="min-w-0 flex-1 truncate font-path" title={rel}>
          {rel}
        </span>
        <span className="shrink-0 tabular-nums text-xs text-muted-foreground">
          {content ? formatBytes(content.bytes_len) : ""}
        </span>
        <RevealInFinderButton path={livePath} />
      </header>
      {body}
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
