import { createRoot } from "react-dom/client";

import { App } from "./App";
import { applyTheme, readTheme } from "./lib/theme";
import "./index.css";

applyTheme(readTheme());

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(<App />);
}
