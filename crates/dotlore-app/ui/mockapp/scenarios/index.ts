import type { Scenario } from "./types";

export const scenarios: Scenario[] = [
  { id: "setup", label: "Setup", detail: "Choose a cloud folder" },
  { id: "empty", label: "Empty", detail: "Provider set, no roots" },
  { id: "populated", label: "Populated", detail: "File viewer" },
  { id: "conflicts", label: "Conflicts", detail: "Resolver" },
  { id: "all-projects", label: "All projects", detail: "Project grid" },
  { id: "git-missing", label: "Git missing", detail: "Install-git banner" },
  { id: "unlinked-project", label: "Unlinked project", detail: "Cloud project not yet bound" },
  { id: "include-list-editor", label: "Include-list editor", detail: "Default patterns and Add to track" },
  { id: "oversized-entry", label: "Oversized entry", detail: "TooLarge file weight" },
];

export const defaultScenarioId = "populated";

export function getScenario(id: string): Scenario | undefined {
  return scenarios.find((scenario) => scenario.id === id);
}
