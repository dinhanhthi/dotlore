import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { progressLabel, UpdateBadge } from "./UpdateBadge";

describe("update badge", () => {
  it("labels every install phase", () => {
    expect(progressLabel({ phase: "downloading", percent: null })).toBe(
      "Downloading update…",
    );
    expect(progressLabel({ phase: "downloading", percent: 42 })).toBe(
      "Downloading update 42%",
    );
    expect(progressLabel({ phase: "installing" })).toBe("Installing update…");
  });

  it("gives the button back after a failed install", () => {
    expect(progressLabel({ phase: "idle" })).toBeNull();
  });

  it("renders nothing until a check finds an update", () => {
    expect(renderToStaticMarkup(<UpdateBadge />)).toBe("");
  });
});
