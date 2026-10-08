import { describe, expect, it } from "vitest";

import { keepArgs, otherTitle, pairSibling, resolverSides } from "./resolver-sides";
import type { ResolutionDto, SiblingDto } from "./types";

function sibling(overrides: Partial<SiblingDto> & { path: string }): SiblingDto {
  return {
    device_name: "studio",
    is_me: false,
    text: "studio text",
    bytes_len: 11,
    ...overrides,
  };
}

function dto(siblings: SiblingDto[]): ResolutionDto {
  return {
    slug: "proj",
    live: "CLAUDE.md",
    live_text: "live text",
    binary: false,
    live_bytes_len: 9,
    siblings,
  };
}

describe("resolverSides", () => {
  it("puts the live file on this machine's side when this machine won", () => {
    const sides = resolverSides(dto([sibling({ path: "CLAUDE.conflict-11111111-abc1234.md" })]));
    expect(sides.mine).toEqual({
      source: null,
      label: "this machine",
      text: "live text",
      bytesLen: 9,
    });
    expect(sides.cloud.map((s) => [s.source, s.label, s.text])).toEqual([
      ["CLAUDE.conflict-11111111-abc1234.md", "studio", "studio text"],
    ]);
  });

  it("puts this machine's sibling on its side and the live file on the cloud's when it lost", () => {
    const sides = resolverSides(
      dto([
        sibling({
          path: "CLAUDE.conflict-aaaaaaaa-def5678.md",
          device_name: "laptop",
          is_me: true,
          text: "my text",
          bytes_len: 7,
        }),
      ]),
    );
    expect(sides.mine).toEqual({
      source: "CLAUDE.conflict-aaaaaaaa-def5678.md",
      label: "this machine",
      text: "my text",
      bytesLen: 7,
    });
    expect(sides.cloud).toEqual([
      { source: null, label: "synced version", text: "live text", bytesLen: 9 },
    ]);
  });

  it("keeps other devices' siblings after the live file when this machine lost", () => {
    const sides = resolverSides(
      dto([
        sibling({ path: "a.conflict-11111111-1111111.md" }),
        sibling({ path: "a.conflict-aaaaaaaa-2222222.md", is_me: true, text: "my text" }),
      ]),
    );
    expect(sides.mine.text).toBe("my text");
    expect(sides.cloud.map((s) => s.source)).toEqual([null, "a.conflict-11111111-1111111.md"]);
  });

  it("labels a second lost copy of this machine apart from the first", () => {
    const sides = resolverSides(
      dto([
        sibling({ path: "a.conflict-aaaaaaaa-1111111.md", is_me: true, text: "first" }),
        sibling({ path: "a.conflict-aaaaaaaa-2222222.md", is_me: true, text: "second" }),
      ]),
    );
    expect(sides.mine.text).toBe("first");
    expect(sides.cloud.map((s) => [s.source, s.label])).toEqual([
      [null, "synced version"],
      ["a.conflict-aaaaaaaa-2222222.md", "this machine (other copy)"],
    ]);
  });
});

describe("otherTitle", () => {
  it("names the device a sibling came from", () => {
    const won = resolverSides(dto([sibling({ path: "s.md", device_name: "Thi M4 prod" })]));
    expect(otherTitle(won.cloud[0])).toBe("from Thi M4 prod");
  });

  it("calls the live file the synced version when this machine lost", () => {
    const lost = resolverSides(dto([sibling({ path: "me.md", is_me: true })]));
    expect(otherTitle(lost.cloud[0])).toBe("synced version");
  });
});

describe("pairSibling", () => {
  it("is the cloud sibling when there is one, else this machine's", () => {
    const won = resolverSides(dto([sibling({ path: "s.md" })]));
    expect(pairSibling(won.mine, won.cloud[0])).toBe("s.md");
    const lost = resolverSides(dto([sibling({ path: "me.md", is_me: true })]));
    expect(pairSibling(lost.mine, lost.cloud[0])).toBe("me.md");
  });
});

describe("keepArgs", () => {
  it("keeps this machine's sibling, not the live file, when this machine lost", () => {
    const lost = resolverSides(dto([sibling({ path: "me.md", is_me: true })]));
    expect(keepArgs(lost.mine)).toEqual({ keep: "other", sibling: "me.md" });
    expect(keepArgs(lost.cloud[0])).toEqual({ keep: "live", sibling: null });
  });

  it("keeps the live file for this machine when it won", () => {
    const won = resolverSides(dto([sibling({ path: "s.md" })]));
    expect(keepArgs(won.mine)).toEqual({ keep: "live", sibling: null });
    expect(keepArgs(won.cloud[0])).toEqual({ keep: "other", sibling: "s.md" });
  });
});
