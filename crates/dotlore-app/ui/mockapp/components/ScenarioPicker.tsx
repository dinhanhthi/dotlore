import { scenarios } from "../scenarios/index";

type ScenarioPickerProps = {
  activeId: string;
  onApply: (id: string) => void;
};

export function ScenarioPicker({ activeId, onApply }: ScenarioPickerProps) {
  return (
    <aside className="flex h-full w-60 shrink-0 flex-col border-l border-neutral-200 bg-neutral-50 text-neutral-800 dark:border-neutral-800 dark:bg-neutral-950 dark:text-neutral-100">
      <header className="shrink-0 px-3 pt-3 pb-2">
        <p className="text-[11px] font-medium tracking-wide text-neutral-500 uppercase dark:text-neutral-400">
          Scenarios
        </p>
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
                  ? "rounded-md bg-neutral-200 px-2 py-1.5 text-left text-[13px] dark:bg-neutral-800"
                  : "rounded-md px-2 py-1.5 text-left text-[13px] hover:bg-neutral-200/70 dark:hover:bg-neutral-800/80"
              }
            >
              <span className="block font-medium">{scenario.label}</span>
              {scenario.detail ? (
                <span className="mt-0.5 block text-[11px] text-neutral-500 dark:text-neutral-400">
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
