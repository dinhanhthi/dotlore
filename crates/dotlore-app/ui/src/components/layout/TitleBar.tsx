import type { MouseEvent } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { cn } from "@/lib/utils";

import { TitleBarActions } from "./TitleBarActions";

/**
 * Overlay titlebar: drag strip on the left (traffic lights sit here),
 * actions pinned to the far right. WKWebView ignores `data-tauri-drag-region`;
 * `startDragging()` is the supported path.
 */
function handleMouseDown(e: MouseEvent) {
  const target = e.target as HTMLElement;
  if (target.closest('a, button, input, select, textarea, [role="button"]')) {
    return;
  }
  void getCurrentWindow()
    .startDragging()
    .catch((err: unknown) => {
      if (import.meta.env.DEV) {
        console.warn("[TitleBar] startDragging() rejected:", err);
      }
    });
}

export function TitleBar({ className }: { className?: string }) {
  return (
    <div
      className={cn(
        "flex h-titlebar w-full shrink-0 items-center border-b border-border bg-background",
        className,
      )}
    >
      <div
        data-tauri-drag-region
        onMouseDown={handleMouseDown}
        className="h-full min-w-0 flex-1"
      />
      <TitleBarActions />
    </div>
  );
}
