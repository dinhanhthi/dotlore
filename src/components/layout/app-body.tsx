import { createContext, useContext, useRef, type ReactNode, type RefObject } from "react";

const AppBodyRefContext = createContext<RefObject<HTMLDivElement | null> | null>(
  null,
);

export function AppBodyProvider({ children }: { children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  return (
    <AppBodyRefContext.Provider value={ref}>{children}</AppBodyRefContext.Provider>
  );
}

/** Portal target for dialogs: the layer that covers the shell body. */
export function useAppBodyRef(): RefObject<HTMLDivElement | null> | null {
  return useContext(AppBodyRefContext);
}
