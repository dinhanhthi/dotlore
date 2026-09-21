import { useCallback, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";

import { cn } from "@/lib/utils";

import { TitleBar } from "./TitleBar";

const SIDEBAR_W = 240;
const TREE_MIN = 180;
const TREE_MAX = 480;
const TREE_DEFAULT = 280;
const TREE_WIDTH_KEY = "dotlore.treeWidth";

function readTreeWidth(): number {
  try {
    const raw = localStorage.getItem(TREE_WIDTH_KEY);
    const n = raw ? Number(raw) : TREE_DEFAULT;
    if (Number.isFinite(n)) return Math.min(TREE_MAX, Math.max(TREE_MIN, n));
  } catch {
    // Private mode.
  }
  return TREE_DEFAULT;
}

function writeTreeWidth(width: number): void {
  try {
    localStorage.setItem(TREE_WIDTH_KEY, String(width));
  } catch {
    // Quota or private-mode.
  }
}

type ShellProps = {
  hideTree?: boolean;
  sidebar: ReactNode;
  tree?: ReactNode;
  main: ReactNode;
  footer: ReactNode;
};

export function Shell({
  hideTree = false,
  sidebar,
  tree,
  main,
  footer,
}: ShellProps) {
  const [treeWidth, setTreeWidth] = useState(readTreeWidth);

  const onResize = useCallback((width: number) => {
    const next = Math.min(TREE_MAX, Math.max(TREE_MIN, width));
    setTreeWidth(next);
    writeTreeWidth(next);
  }, []);

  const showSidebar = sidebar != null;
  const columns = !showSidebar
    ? "1fr"
    : hideTree
      ? `${SIDEBAR_W}px 1fr`
      : `${SIDEBAR_W}px ${treeWidth}px 1fr`;

  return (
    <div
      className="grid h-svh w-full overflow-hidden bg-background [grid-template-rows:auto_1fr_auto]"
      style={{ gridTemplateColumns: columns }}
    >
      <TitleBar className="col-span-full" />
      {showSidebar ? (
        <aside className="flex min-h-0 flex-col overflow-hidden border-r border-sidebar-border bg-sidebar">
          {sidebar}
        </aside>
      ) : null}
      {!hideTree && (
        <section className="relative flex min-h-0 min-w-0 flex-col overflow-hidden bg-background">
          {tree}
          <ResizeHandle
            width={treeWidth}
            onResize={onResize}
          />
        </section>
      )}
      <main
        className={cn(
          "flex min-h-0 flex-col overflow-hidden bg-background",
          showSidebar && "border-l border-border",
        )}
      >
        {main}
      </main>
      <div className="col-span-full">{footer}</div>
    </div>
  );
}

function ResizeHandle({
  width,
  onResize,
}: {
  width: number;
  onResize: (width: number) => void;
}) {
  const onMouseDown = (event: ReactMouseEvent) => {
    event.preventDefault();
    const startX = event.clientX;
    const startW = width;

    const onMove = (ev: globalThis.MouseEvent) => {
      onResize(startW + ev.clientX - startX);
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize file tree"
      onMouseDown={onMouseDown}
      className={cn(
        "absolute inset-y-0 right-0 z-10 w-1.5 cursor-col-resize",
        "bg-transparent",
        "transition-[background-color,box-shadow] duration-[var(--dur-short)] ease-[var(--ease-out)]",
        "hover:bg-foreground/35 hover:shadow-[inset_-1px_0_0_0_var(--foreground)]",
        "active:bg-foreground/55",
      )}
    />
  );
}
