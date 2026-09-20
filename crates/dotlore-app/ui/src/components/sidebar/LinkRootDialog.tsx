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
import { errorMessage } from "@/lib/errors";
import { linkRoot, listLinkable } from "@/lib/ipc";
import { pickLocalPath } from "@/lib/pick";
import { useRoots } from "@/lib/roots";

type LinkRootDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

export function LinkRootDialog({ open, onOpenChange }: LinkRootDialogProps) {
  const { refreshRoots, selectRoot, busy, setBanner } = useRoots();
  const [slugs, setSlugs] = useState<string[]>([]);
  const [picked, setPicked] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setPicked(null);
    setLoading(true);
    void listLinkable()
      .then((next) => {
        if (!cancelled) setSlugs(next);
      })
      .catch((err) => {
        if (!cancelled) {
          setBanner(errorMessage(err, "Could not list linkable slugs"));
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, setBanner]);

  async function linkTo(slug: string, directory: boolean) {
    if (busy) return;
    const path = await pickLocalPath(directory);
    if (path === null) return;
    try {
      await linkRoot(slug, path);
      await refreshRoots();
      selectRoot(slug);
      onOpenChange(false);
    } catch {
      // Banner is set by `run()`.
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Link a cloud slug</DialogTitle>
          <DialogDescription>
            {picked === null
              ? "Slugs already in the cloud folder that this Mac is not tracking."
              : `Choose a local folder or file for ${picked}.`}
          </DialogDescription>
        </DialogHeader>
        {loading ? (
          <p className="text-sm text-muted-foreground">Loading…</p>
        ) : picked === null ? (
          slugs.length === 0 ? (
            <p className="text-sm text-muted-foreground">Nothing to link.</p>
          ) : (
            <div className="flex max-h-56 flex-col gap-1 overflow-y-auto">
              {slugs.map((slug) => (
                <Button
                  key={slug}
                  type="button"
                  variant="ghost"
                  className="w-full justify-start font-mono"
                  disabled={busy}
                  onClick={() => setPicked(slug)}
                >
                  {slug}
                </Button>
              ))}
            </div>
          )
        ) : (
          <div className="flex flex-col gap-2">
            <Button
              type="button"
              variant="outline"
              disabled={busy}
              onClick={() => {
                void linkTo(picked, true);
              }}
            >
              Folder…
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={busy}
              onClick={() => {
                void linkTo(picked, false);
              }}
            >
              File…
            </Button>
          </div>
        )}
        <DialogFooter>
          {picked !== null && (
            <Button
              type="button"
              variant="ghost"
              disabled={busy}
              onClick={() => setPicked(null)}
            >
              Back
            </Button>
          )}
          <Button
            type="button"
            variant="outline"
            disabled={busy}
            onClick={() => onOpenChange(false)}
          >
            Cancel
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
