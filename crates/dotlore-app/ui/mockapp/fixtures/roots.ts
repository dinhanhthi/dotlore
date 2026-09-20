import type { RootRow } from "@/lib/types";

export const ICLOUD_PROVIDER =
  "/Users/demo/Library/Mobile Documents/com~apple~CloudDocs/dotlore";

export const GDRIVE_MOUNTS = [
  "/Users/demo/Library/CloudStorage/GoogleDrive-demo",
];

export const LINKABLE_SLUGS = ["old-mac-notes"];

export function demoRoots(): RootRow[] {
  return [
    {
      slug: "dotlore",
      path: "/Users/demo/git/dotlore",
      name: "dotlore",
      is_agent: true,
      status: { kind: "Synced" },
    },
    {
      slug: "memlore",
      path: "/Users/demo/git/memlore",
      name: "memlore",
      is_agent: false,
      status: { kind: "Synced" },
    },
    {
      slug: "notes",
      path: "/Users/demo/Notes",
      name: "Notes",
      is_agent: false,
      status: { kind: "Conflicts", detail: 2 },
    },
    {
      slug: "missing-proj",
      path: "/Users/demo/gone/project",
      name: "missing-proj",
      is_agent: false,
      status: { kind: "RootMissing" },
    },
  ];
}
