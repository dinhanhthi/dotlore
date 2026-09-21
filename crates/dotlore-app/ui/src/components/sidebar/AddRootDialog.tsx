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
  const { addProject, busy } = useRoots();
  const [slug, setSlug] = useState(defaultSlug);
  const [closing, setClosing] = useState(false);

  useEffect(() => {
    if (!open) return;
    setSlug(defaultSlug);
    setClosing(false);
  }, [open, defaultSlug]);

  function submit() {
    const trimmed = slug.trim();
    if (trimmed.length === 0 || closing) return;
    setClosing(true);
    onOpenChange(false);
    void addProject(path, trimmed);
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
              disabled={busy || closing}
            />
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={closing}
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={closing || slug.trim().length === 0}>
              Add
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
