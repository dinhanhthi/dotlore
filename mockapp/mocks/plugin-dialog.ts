type OpenOptions = {
  directory?: boolean;
  multiple?: boolean;
};

/** Fixture paths that are files — identity, not basename or extension. */
export const MOCK_FILE_PATHS: Set<string> = new Set([
  "/Users/demo/Projects/new-root/CLAUDE.md",
  "/Users/demo/Projects/Makefile",
]);

export async function open(
  _options?: OpenOptions,
): Promise<string | string[] | null> {
  return "/Users/demo/Projects/new-root";
}

export async function save(_options?: unknown): Promise<string | null> {
  return null;
}

export async function message(msg: string, _options?: unknown): Promise<void> {
  console.info("[mockapp] dialog.message", msg);
}

export async function ask(_msg: string, _options?: unknown): Promise<boolean> {
  return false;
}

export async function confirm(
  _msg: string,
  _options?: unknown,
): Promise<boolean> {
  return false;
}
