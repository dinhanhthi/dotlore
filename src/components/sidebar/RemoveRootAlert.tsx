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
import { BLOCKED, forgetRoot, removeRoot } from "@/lib/ipc";
import { useRoots } from "@/lib/roots";

type RemoveRootAlertProps = {
  slug: string | null;
  name: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Also hide the project's cloud row on this Mac (`forgetRoot`). */
  forget?: boolean;
};

export function RemoveRootAlert({
  slug,
  name,
  open,
  onOpenChange,
  forget = false,
}: RemoveRootAlertProps) {
  const { refreshRoots, locked } = useRoots();

  /** Closes first: the remove runs as a footer task, the window stays usable. */
  async function confirm() {
    if (slug === null || locked) return;
    onOpenChange(false);
    try {
      const remove = forget ? forgetRoot : removeRoot;
      if ((await remove(slug)) === BLOCKED) return;
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
            {forget
              ? `Dotlore will stop tracking ${name} and hide it on this Mac. Its copy in the cloud stays, and other Macs keep syncing it.`
              : `Dotlore will stop tracking ${name}. Files stay on disk.`}
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
