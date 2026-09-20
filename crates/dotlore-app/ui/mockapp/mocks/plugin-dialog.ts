type OpenOptions = {
  directory?: boolean;
  multiple?: boolean;
};

export async function open(
  options?: OpenOptions,
): Promise<string | string[] | null> {
  if (options?.directory) return "/Users/demo/Projects/new-root";
  return "/Users/demo/Projects/new-root/CLAUDE.md";
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
