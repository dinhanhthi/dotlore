import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * Closing the window destroys the page, so the close waits for `guard`:
 * unsaved Result edits ask first, anything else closes at once.
 */
export function useCloseGuard(guard: (run: () => void) => void): void {
  useEffect(() => {
    const win = getCurrentWindow();
    const unlisten = win.onCloseRequested((event) => {
      event.preventDefault();
      guard(() => void win.destroy());
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [guard]);
}
