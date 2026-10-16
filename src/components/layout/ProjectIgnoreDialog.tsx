import { useEffect, useState } from "react";

import { ignoreToLines } from "@/components/settings/SettingsPopover";
import { SettingsSeedList } from "@/components/settings/SettingsSeedList";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { BLOCKED, projectIgnore, setProjectIgnore } from "@/lib/ipc";
import { useRoots, useTaskLabel } from "@/lib/roots";

type ProjectIgnoreDialogProps = {
  slug: string | null;
  name: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

/** One project's own ignore list, on top of the global never-list. */
export function ProjectIgnoreDialog({
  slug,
  name,
  open,
  onOpenChange,
}: ProjectIgnoreDialogProps) {
  const { locked } = useRoots();
  const taskLabel = useTaskLabel();
  const [lines, setLines] = useState<string[]>([]);

  useEffect(() => {
    if (!open || slug === null) return;
    setLines([]);
    void projectIgnore(slug)
      .then((text) => setLines(ignoreToLines(text)))
      .catch(() => {
        // getter failed; leave the list empty
      });
  }, [open, slug]);

  async function commit(next: string[]) {
    if (locked || slug === null) return;
    try {
      if ((await setProjectIgnore(slug, `${next.join("\n")}\n`)) === BLOCKED) {
        return;
      }
      setLines(next);
    } catch {
      void projectIgnore(slug)
        .then((text) => setLines(ignoreToLines(text)))
        .catch(() => {});
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex min-h-0 flex-col sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Ignore list — {name}</DialogTitle>
        </DialogHeader>
        <div className="flex h-[24rem] min-h-0 flex-col">
          <SettingsSeedList
            id={`project-ignore-${slug}`}
            hint="Only for this project, on top of the global never-list. Syncs to your other Macs. Matching files are untracked, not deleted. Start a line with ! to sync a path the never-list ignores (name the ignored entry itself, e.g. !/plans/); sensitive files always stay out."
            lines={lines}
            disabled={locked || taskLabel !== null}
            addPlaceholder="Add an entry"
            onCommit={(next) => {
              void commit(next);
            }}
          />
        </div>
      </DialogContent>
    </Dialog>
  );
}
