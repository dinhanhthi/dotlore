export type UnlistenFn = () => void;

const listeners = new Map<string, Set<(payload: unknown) => void>>();

export async function listen<T>(
  event: string,
  handler: (e: { event: string; payload: T }) => void,
): Promise<UnlistenFn> {
  if (!listeners.has(event)) listeners.set(event, new Set());
  const wrapped = (payload: unknown) =>
    handler({ event, payload: payload as T });
  listeners.get(event)!.add(wrapped);
  return () => {
    listeners.get(event)?.delete(wrapped);
  };
}

export async function once<T>(
  event: string,
  handler: (e: { event: string; payload: T }) => void,
): Promise<UnlistenFn> {
  let unlisten: UnlistenFn = () => {};
  const wrapper = (e: { event: string; payload: T }) => {
    handler(e);
    unlisten();
  };
  unlisten = await listen<T>(event, wrapper);
  return unlisten;
}

export async function emit(event: string, payload?: unknown): Promise<void> {
  __emit(event, payload);
}

export function __emit(event: string, payload?: unknown): void {
  const handlers = listeners.get(event);
  if (!handlers) return;
  for (const handler of handlers) handler(payload);
}

export function __clearListeners(): void {
  listeners.clear();
}
