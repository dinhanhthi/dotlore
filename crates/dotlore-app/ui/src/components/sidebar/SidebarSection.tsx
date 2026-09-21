import type { ReactNode } from "react";
import { ChevronRight } from "lucide-react";

import { cn } from "@/lib/utils";

type SidebarSectionProps = {
  id: string;
  title: string;
  collapsed: boolean;
  onToggle: () => void;
  action?: ReactNode;
  children: ReactNode;
};

export function SidebarSection({
  id,
  title,
  collapsed,
  onToggle,
  action,
  children,
}: SidebarSectionProps) {
  return (
    <section aria-labelledby={`sidebar-section-${id}`}>
      <div className="flex h-row items-center pr-2">
        <button
          type="button"
          id={`sidebar-section-${id}`}
          aria-expanded={!collapsed}
          aria-controls={`sidebar-section-panel-${id}`}
          onClick={onToggle}
          className="flex h-full min-w-0 flex-1 items-center gap-1 px-2 text-label text-muted-foreground hover:text-foreground"
        >
          <ChevronRight
            aria-hidden
            className={cn(
              "size-4 shrink-0 transition-transform duration-[var(--dur-short)] ease-[var(--ease-out)] motion-reduce:transition-none",
              !collapsed && "rotate-90",
            )}
          />
          {title}
        </button>
        {action}
      </div>
      <div
        id={`sidebar-section-panel-${id}`}
        data-collapsed={collapsed ? "true" : "false"}
        inert={collapsed ? true : undefined}
        className={cn(
          "grid transition-[grid-template-rows] duration-[var(--dur-short)] ease-[var(--ease-out)] motion-reduce:transition-none",
          collapsed ? "grid-rows-[0fr]" : "grid-rows-[1fr]",
        )}
      >
        <div className="min-h-0 overflow-hidden">
          <div className="pl-4">{children}</div>
        </div>
      </div>
    </section>
  );
}
