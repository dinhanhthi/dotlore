import type { ReactNode } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";

type SidebarSectionProps = {
  id: string;
  title: string;
  collapsed: boolean;
  onToggle: () => void;
  children: ReactNode;
};

export function SidebarSection({
  id,
  title,
  collapsed,
  onToggle,
  children,
}: SidebarSectionProps) {
  return (
    <section aria-labelledby={`sidebar-section-${id}`}>
      <button
        type="button"
        id={`sidebar-section-${id}`}
        aria-expanded={!collapsed}
        onClick={onToggle}
        className="flex h-row w-full items-center gap-1 px-pad-x text-label text-muted-foreground hover:text-foreground"
      >
        {collapsed ? (
          <ChevronRight aria-hidden className="size-3 shrink-0" />
        ) : (
          <ChevronDown aria-hidden className="size-3 shrink-0" />
        )}
        {title}
      </button>
      {!collapsed && <div>{children}</div>}
    </section>
  );
}
