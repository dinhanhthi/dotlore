import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { SensitivePathList } from "./EntryPickerDialog";

describe("SensitivePathList", () => {
  it("keeps secret paths in a scrollable region and notes hidden paths", () => {
    const html = renderToStaticMarkup(
      <SensitivePathList paths={["notes/credentials.json", "notes/server.pem"]} more />,
    );
    expect(html).toContain("max-h-40");
    expect(html).toContain("overflow-y-auto");
    expect(html).toContain("notes/credentials.json");
    expect(html).toContain("notes/server.pem");
    expect(html).toContain("additional secret files not shown");
  });

  it("does not show the overflow note when all paths are listed", () => {
    const html = renderToStaticMarkup(
      <SensitivePathList paths={["notes/credentials.json"]} more={false} />,
    );
    expect(html).not.toContain("additional secret files not shown");
  });
});
