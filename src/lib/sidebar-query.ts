import { createContext, createElement, useContext, type ReactNode } from "react";

import type { RootRow } from "@/lib/types";

type SidebarQueryValue = {
  query: string;
  setQuery: (query: string) => void;
};

const SidebarQueryContext = createContext<SidebarQueryValue>({
  query: "",
  setQuery: () => {},
});

export function SidebarQueryProvider({
  query,
  setQuery,
  children,
}: SidebarQueryValue & { children: ReactNode }) {
  return createElement(
    SidebarQueryContext.Provider,
    { value: { query, setQuery } },
    children,
  );
}

export function useSidebarQuery(): SidebarQueryValue {
  return useContext(SidebarQueryContext);
}

/** Same match the sidebar uses: display name or slug, case-insensitive. */
export function matchesRootQuery(row: RootRow, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return (
    row.name.toLowerCase().includes(needle) ||
    row.slug.toLowerCase().includes(needle)
  );
}
