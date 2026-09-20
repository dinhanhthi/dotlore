import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

import { TitleBar } from "./TitleBar";

type ShellProps = {
  /** Task 6: "All projects" drops the middle column. */
  hideTree?: boolean;
  sidebar: ReactNode;
  tree?: ReactNode;
  main: ReactNode;
  footer: ReactNode;
};

export function Shell({
  hideTree = false,
  sidebar,
  tree,
  main,
  footer,
}: ShellProps) {
  return (
    <div
      className={cn(
        "grid h-svh w-full overflow-hidden bg-background",
        "grid-rows-[auto_1fr_auto]",
        hideTree ? "grid-cols-[220px_1fr]" : "grid-cols-[220px_260px_1fr]",
      )}
    >
      <TitleBar className="col-span-full" />
      <aside className="flex min-h-0 flex-col overflow-hidden border-r border-sidebar-border bg-sidebar">
        {sidebar}
      </aside>
      {!hideTree && (
        <section className="flex min-h-0 flex-col overflow-hidden border-r border-border bg-background">
          {tree}
        </section>
      )}
      <main className="flex min-h-0 flex-col overflow-hidden bg-background">
        {main}
      </main>
      <div className="col-span-full">{footer}</div>
    </div>
  );
}
