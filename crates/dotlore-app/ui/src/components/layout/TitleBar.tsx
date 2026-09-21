import { useEffect, useState, type MouseEvent } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { cn } from "@/lib/utils";

import { TitleBarActions } from "./TitleBarActions";

const APP_VERSION = "0.1.0";

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
  const [version, setVersion] = useState(APP_VERSION);

  useEffect(() => {
    void getVersion()
      .then(setVersion)
      .catch(() => {
        setVersion(APP_VERSION);
      });
  }, []);

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
        className="flex h-full min-w-0 flex-1 items-center gap-2 pl-28"
      >
        <img
          src="/logo_256.png"
          alt=""
          className="pointer-events-none size-6"
        />
        <span className="pointer-events-none text-sm font-medium text-foreground">
          Dotlore
        </span>
        <span className="pointer-events-none text-xs text-muted-foreground">
          {version}
        </span>
      </div>
      <TitleBarActions />
    </div>
  );
}
