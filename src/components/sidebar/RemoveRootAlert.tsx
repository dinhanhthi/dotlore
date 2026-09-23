import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { BLOCKED, removeRoot } from "@/lib/ipc";
import { useRoots } from "@/lib/roots";

type RemoveRootAlertProps = {
  slug: string | null;
  name: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

export function RemoveRootAlert({
  slug,
  name,
  open,
  onOpenChange,
}: RemoveRootAlertProps) {
  const { refreshRoots, locked } = useRoots();

  /** Closes first: the remove runs as a footer task, the window stays usable. */
  async function confirm() {
    if (slug === null || locked) return;
    onOpenChange(false);
    try {
      if ((await removeRoot(slug)) === BLOCKED) return;
      await refreshRoots();
    } catch {
      // Banner is set by `runTask()`.
    }
  }

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Remove from Dotlore?</AlertDialogTitle>
          <AlertDialogDescription>
            Dotlore will stop tracking {name}. Files stay on disk.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={locked}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            disabled={locked || slug === null}
            onClick={(event) => {
              event.preventDefault();
              void confirm();
            }}
          >
            Remove from Dotlore
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
