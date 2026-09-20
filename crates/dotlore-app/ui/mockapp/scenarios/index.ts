import type { Scenario } from "./types";

export const scenarios: Scenario[] = [
  { id: "setup", label: "Setup — choose cloud folder" },
  { id: "empty", label: "Empty — no roots" },
  { id: "populated", label: "Populated — file viewer" },
  { id: "conflicts", label: "Conflicts — resolver" },
  { id: "all-projects", label: "All projects" },
  { id: "git-missing", label: "Git missing banner" },
];

export const defaultScenarioId = "populated";

export function getScenario(id: string): Scenario | undefined {
  return scenarios.find((scenario) => scenario.id === id);
}
