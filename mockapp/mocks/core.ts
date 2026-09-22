import { route } from "./invokeRouter";

export async function invoke<T = unknown>(
  cmd: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  return route(cmd, args) as Promise<T>;
}

export function convertFileSrc(path: string): string {
  if (path.startsWith("data:") || path.startsWith("http")) return path;
  return "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
}
