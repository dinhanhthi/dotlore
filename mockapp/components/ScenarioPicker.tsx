import { scenarios } from "../scenarios/index";

type ScenarioPickerProps = {
  activeId: string;
  onApply: (id: string) => void;
};

export function ScenarioPicker({ activeId, onApply }: ScenarioPickerProps) {
  return (
    <aside className="flex h-full w-60 shrink-0 flex-col border-l border-sidebar-border bg-sidebar text-sidebar-foreground">
      <header className="shrink-0 px-3 pt-3 pb-2">
        <p className="text-label text-muted-foreground">Scenarios</p>
      </header>
      <nav
        aria-label="Scenarios"
        className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-2 pb-3"
      >
        {scenarios.map((scenario) => {
          const active = scenario.id === activeId;
          return (
            <button
              key={scenario.id}
              type="button"
              onClick={() => onApply(scenario.id)}
              className={
                active
                  ? "rounded-2xl bg-sidebar-accent px-2.5 py-1.5 text-left text-sm"
                  : "rounded-2xl px-2.5 py-1.5 text-left text-sm hover:bg-sidebar-accent/80"
              }
            >
              <span className="block font-medium">{scenario.label}</span>
              {scenario.detail ? (
                <span className="mt-0.5 block text-xs text-muted-foreground">
                  {scenario.detail}
                </span>
              ) : null}
            </button>
          );
        })}
      </nav>
    </aside>
  );
}
