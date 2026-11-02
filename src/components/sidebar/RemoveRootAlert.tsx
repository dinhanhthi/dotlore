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
import { BLOCKED, forgetRoot } from "@/lib/ipc";
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
      if ((await forgetRoot(slug)) === BLOCKED) return;
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
            Dotlore will stop syncing {name} and hide it on this Mac. Files stay
            on disk, and its copy in the cloud stays for your other Macs. To keep
            it on Dotlore without syncing, unlink it instead.
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
