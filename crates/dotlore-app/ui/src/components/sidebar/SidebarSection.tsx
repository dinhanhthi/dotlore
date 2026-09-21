import type { ReactNode } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";

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
          onClick={onToggle}
          className="flex h-full min-w-0 flex-1 items-center gap-1 px-2 text-label text-muted-foreground hover:text-foreground"
        >
          {collapsed ? (
            <ChevronRight aria-hidden className="size-4 shrink-0" />
          ) : (
            <ChevronDown aria-hidden className="size-4 shrink-0" />
          )}
          {title}
        </button>
        {action}
      </div>
      {!collapsed && <div className="pl-4">{children}</div>}
    </section>
  );
}
