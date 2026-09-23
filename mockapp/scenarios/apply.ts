import { LINKABLE_ROWS } from "../fixtures/roots";
import { resetStore, type MockState } from "../mocks/store";
import { defaultScenarioId, getScenario, scenarios } from "./index";

export const STORAGE_KEY = "dotlore.web-scenario";

export type ScenarioSeed = Partial<MockState>;

const seeds: Record<string, ScenarioSeed> = {
  setup: { providerDir: null, roots: [], files: {}, conflicts: {} },
  empty: {
    roots: [],
    files: {},
    conflicts: {},
    linkable: [...LINKABLE_ROWS],
  },
  populated: {},
  conflicts: {},
  "all-projects": {},
  "git-missing": { gitMissing: true },
  "unlinked-project": {},
  "include-list-editor": {
    pickerExtra: {
      dotlore: {
        "": [{ name: "NOTES.md", kind: "file", rel: "NOTES.md" }],
      },
    },
  },
  "oversized-entry": {},
};

function clickWhen(find: () => HTMLElement | null, timeoutMs = 2500): void {
  const started = Date.now();
  const tick = () => {
    const node = find();
    if (node) {
      node.click();
      return;
    }
    if (Date.now() - started > timeoutMs) return;
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

function buttonByText(text: string): HTMLElement | null {
  const buttons = document.querySelectorAll<HTMLElement>("button");
  for (const button of buttons) {
    if (button.textContent?.trim() === text) return button;
  }
  return null;
}

export function afterMountFor(id: string): (() => void) | undefined {
  if (id === "populated") {
    return () => {
      clickWhen(() =>
        document.querySelector<HTMLElement>("#sidebar-root-dotlore button"),
      );
      window.setTimeout(() => {
        clickWhen(() => buttonByText("CLAUDE.md"));
      }, 80);
    };
  }
  if (id === "conflicts") {
    return () => {
      clickWhen(() =>
        document.querySelector<HTMLElement>(
          '#sidebar-root-notes [aria-label="4 conflicts"]',
        ),
      );
    };
  }
  if (id === "all-projects") {
    return () => {
      clickWhen(() => buttonByText("All projects"));
    };
  }
  if (id === "unlinked-project") {
    return () => {
      clickWhen(() =>
        document.querySelector<HTMLElement>("#sidebar-root-old-mac-notes button"),
      );
    };
  }
  if (id === "include-list-editor") {
    return () => {
      clickWhen(() =>
        document.querySelector<HTMLElement>("#sidebar-root-dotlore button"),
      );
      window.setTimeout(() => {
        clickWhen(() =>
          document.querySelector<HTMLElement>('[aria-label="Settings"]'),
        );
      }, 80);
      window.setTimeout(() => {
        clickWhen(() => buttonByText("Patterns"));
      }, 160);
    };
  }
  if (id === "oversized-entry") {
    return () => {
      clickWhen(() =>
        document.querySelector<HTMLElement>("#sidebar-root-dotlore button"),
      );
      window.setTimeout(() => {
        clickWhen(() => buttonByText("dump.bin"));
      }, 80);
    };
  }
  return undefined;
}

export function applyScenario(id: string, remount: () => void): string {
  const scenario = getScenario(id) ?? getScenario(defaultScenarioId) ?? scenarios[0];
  if (!scenario) return id;
  resetStore(seeds[scenario.id] ?? {});
  try {
    localStorage.setItem(STORAGE_KEY, scenario.id);
  } catch {
    // Private mode.
  }
  const url = new URL(window.location.href);
  url.searchParams.set("scenario", scenario.id);
  window.history.replaceState(null, "", `${url.pathname}${url.search}${url.hash}`);
  remount();
  const after = scenario.afterMount ?? afterMountFor(scenario.id);
  if (after) window.setTimeout(after, 0);
  return scenario.id;
}
