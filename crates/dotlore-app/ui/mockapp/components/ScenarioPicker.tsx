import { scenarios } from "../scenarios/index";

type ScenarioPickerProps = {
  activeId: string;
  onApply: (id: string) => void;
};

export function ScenarioPicker({ activeId, onApply }: ScenarioPickerProps) {
  return (
    <div className="fixed right-3 bottom-3 z-[100] flex items-center gap-2 rounded-md border border-neutral-300 bg-white/95 px-2 py-1.5 text-xs text-neutral-800 shadow-md dark:border-neutral-700 dark:bg-neutral-900/95 dark:text-neutral-100">
      <label className="flex items-center gap-2">
        <span className="text-neutral-500 dark:text-neutral-400">Scenario</span>
        <select
          value={activeId}
          onChange={(event) => onApply(event.target.value)}
          className="rounded border border-neutral-300 bg-transparent px-1.5 py-0.5 dark:border-neutral-600"
        >
          {scenarios.map((scenario) => (
            <option key={scenario.id} value={scenario.id}>
              {scenario.label}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}
