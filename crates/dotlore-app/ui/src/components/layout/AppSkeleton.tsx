/**
 * Placeholders for the first paint, while `list_roots` waits on the home lock.
 * A daemon cycle can hold that lock for seconds, and the window used to sit
 * empty for the whole wait. Geometry mirrors `Sidebar`, `FileTree` and
 * `EmptyState` so nothing shifts when the real rows arrive.
 */

const SIDEBAR_ROW_WIDTHS = ["70%", "55%", "80%", "45%", "65%", "50%"];

function Bar({ className, width }: { className: string; width?: string }) {
  return (
    <div
      aria-hidden
      className={`motion-safe:animate-pulse rounded-2xl ${className}`}
      style={width ? { width } : undefined}
    />
  );
}

export function SidebarSkeleton() {
  return (
    <div
      role="status"
      aria-label="Loading projects"
      className="flex h-full min-h-0 flex-col"
    >
      <div className="flex items-center gap-2 px-3 pt-3 pb-2">
        <Bar className="h-8 flex-1 bg-sidebar-accent" />
      </div>
      <div className="flex flex-col gap-0.5 px-1.5 pb-2">
        {SIDEBAR_ROW_WIDTHS.map((width, index) => (
          <div key={index} className="flex h-8 items-center px-2 mb-2">
            <Bar className="h-3.5 bg-sidebar-accent" width={width} />
          </div>
        ))}
      </div>
    </div>
  );
}

const TREE_ROW_WIDTHS = ["60%", "75%", "50%", "68%", "42%"];

export function TreeSkeleton() {
  return (
    <div
      role="status"
      aria-label="Loading files"
      className="flex h-full min-h-0 min-w-0 flex-col"
    >
      <div className="flex h-row shrink-0 items-center border-b border-border px-3">
        <Bar className="h-3.5 bg-muted" width="45%" />
      </div>
      <div className="flex items-center gap-2 px-3 pt-3 pb-2">
        <Bar className="h-8 flex-1 bg-muted" />
      </div>
      <div className="flex flex-col gap-0.5 px-1.5">
        {TREE_ROW_WIDTHS.map((width, index) => (
          <div key={index} className="flex h-8 items-center px-2">
            <Bar className="h-3.5 bg-muted" width={width} />
          </div>
        ))}
      </div>
    </div>
  );
}

/** Centred like `EmptyState`, which is what the main column shows on a launch
 * with no file selected. */
export function MainSkeleton() {
  return (
    <div
      role="status"
      aria-label="Loading"
      className="flex h-full items-center justify-center px-6"
    >
      <Bar className="h-3.5 w-full max-w-xs bg-muted" />
    </div>
  );
}
