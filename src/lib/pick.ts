import { open } from "@tauri-apps/plugin-dialog";

/** Native folder picker. `null` if cancelled. */
export async function pickLocalPath(): Promise<string | null> {
  const selected = await open({
    directory: true,
    multiple: false,
  });
  if (selected === null) return null;
  const path = Array.isArray(selected) ? selected[0] : selected;
  return path && path.length > 0 ? path : null;
}
