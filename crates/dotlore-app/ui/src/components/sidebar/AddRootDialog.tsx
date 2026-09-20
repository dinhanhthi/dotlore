import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { addRoot } from "@/lib/ipc";
import { useRoots } from "@/lib/roots";

type AddRootDialogProps = {
  path: string;
  defaultSlug: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

export function AddRootDialog({
  path,
  defaultSlug,
  open,
  onOpenChange,
}: AddRootDialogProps) {
  const { refreshRoots, selectRoot, busy } = useRoots();
  const [slug, setSlug] = useState(defaultSlug);

  useEffect(() => {
    if (!open) return;
    setSlug(defaultSlug);
  }, [open, defaultSlug]);

  async function submit() {
    const trimmed = slug.trim();
    if (trimmed.length === 0 || busy) return;
    try {
      const created = await addRoot(path, trimmed);
      await refreshRoots();
      selectRoot(created);
      onOpenChange(false);
    } catch {
      // Banner is set by `run()`.
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <DialogHeader>
            <DialogTitle>Add to Dotlore</DialogTitle>
            <DialogDescription>
              Slug for{" "}
              <span className="font-mono break-all text-foreground">{path}</span>
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-1.5 py-2">
            <label htmlFor="add-root-slug" className="text-label text-muted-foreground">
              Slug
            </label>
            <Input
              id="add-root-slug"
              value={slug}
              onChange={(event) => setSlug(event.target.value)}
              autoComplete="off"
              spellCheck={false}
              disabled={busy}
            />
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={busy}
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={busy || slug.trim().length === 0}>
              Add
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
