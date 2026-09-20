import { RootCard } from "@/components/main/RootCard";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useRoots } from "@/lib/roots";
import type { RootRow } from "@/lib/types";

function CardGrid({ rows }: { rows: RootRow[] }) {
  return (
    <div className="grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-4">
      {rows.map((row) => (
        <RootCard key={row.slug} row={row} />
      ))}
    </div>
  );
}

export function AllProjects() {
  const { roots } = useRoots();
  const agents = roots.filter((row) => row.is_agent);
  const projects = roots.filter((row) => !row.is_agent);

  if (roots.length === 0) {
    return (
      <div className="flex h-full items-center justify-center px-6">
        <p className="text-center text-muted-foreground">Nothing tracked yet.</p>
      </div>
    );
  }

  return (
    <ScrollArea className="h-full">
      <div className="flex flex-col gap-8 p-6">
        {agents.length > 0 && (
          <section>
            <h2 className="mb-3 text-label text-muted-foreground">Agents</h2>
            <CardGrid rows={agents} />
          </section>
        )}
        {projects.length > 0 && (
          <section>
            <h2 className="mb-3 text-label text-muted-foreground">Projects</h2>
            <CardGrid rows={projects} />
          </section>
        )}
      </div>
    </ScrollArea>
  );
}
