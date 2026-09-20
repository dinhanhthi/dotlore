import { listen } from "@tauri-apps/api/event";
import { createRoot } from "react-dom/client";

// Temporary verify hook — Phase 3 replaces this.
void listen("dotlore://status", (event) => {
  console.log(event.payload);
});

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(<div>Dotlore</div>);
}
