import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";

import { App } from "@/App";
import { applyTheme, readTheme } from "@/lib/theme";

import { ScenarioPicker } from "./components/ScenarioPicker";
import { TrafficLights } from "./components/TrafficLights";
import { applyScenario, STORAGE_KEY } from "./scenarios/apply";
import { defaultScenarioId, getScenario } from "./scenarios/index";
import "./styles.css";

applyTheme(readTheme());

function resolveInitialScenarioId(): string {
  const fromUrl = new URLSearchParams(location.search).get("scenario");
  if (fromUrl && getScenario(fromUrl)) return fromUrl;
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored && getScenario(stored)) return stored;
  } catch {
    // Private mode.
  }
  return defaultScenarioId;
}

function Root() {
  const [remountKey, setRemountKey] = useState(0);
  const [activeId, setActiveId] = useState(() =>
    applyScenario(resolveInitialScenarioId(), () => {}),
  );

  function handleApply(id: string) {
    const resolved = applyScenario(id, () => setRemountKey((key) => key + 1));
    setActiveId(resolved);
  }

  return (
    <>
      <App key={remountKey} />
      <TrafficLights />
      <ScenarioPicker activeId={activeId} onApply={handleApply} />
    </>
  );
}

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <Root />
    </StrictMode>,
  );
}
