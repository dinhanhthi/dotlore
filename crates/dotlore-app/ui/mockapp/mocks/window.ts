const noop = () => Promise.resolve();

class MockWindow {
  label = "main";
  minimize = noop;
  maximize = noop;
  unmaximize = noop;
  toggleMaximize = noop;
  close = noop;
  hide = noop;
  show = noop;
  setFullscreen = (_fullscreen: boolean) => Promise.resolve();
  isMaximized = () => Promise.resolve(false);
  isFullscreen = () => Promise.resolve(false);
  startDragging = noop;
  listen = (_event: string, _handler: unknown) => Promise.resolve(() => {});
  onResized = (_handler: unknown) => Promise.resolve(() => {});
  onMoved = (_handler: unknown) => Promise.resolve(() => {});
  onCloseRequested = (_handler: unknown) => Promise.resolve(() => {});
  onFocusChanged = (_handler: unknown) => Promise.resolve(() => {});
  setTitle = (_title: string) => Promise.resolve();
}

const current = new MockWindow();

export function getCurrentWindow(): MockWindow {
  return current;
}

export { MockWindow as Window };
