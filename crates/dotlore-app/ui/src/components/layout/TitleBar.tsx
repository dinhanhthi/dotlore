import { cn } from "@/lib/utils";

/** 28px strip under `titleBarStyle: Overlay` so traffic lights and drag work. */
export function TitleBar({ className }: { className?: string }) {
  return (
    <div
      data-tauri-drag-region
      className={cn("h-[28px] w-full shrink-0", className)}
    />
  );
}
