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
import { removeRoot } from "@/lib/ipc";
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
  const { refreshRoots, busy } = useRoots();

  async function confirm() {
    if (slug === null || busy) return;
    try {
      await removeRoot(slug);
      await refreshRoots();
      onOpenChange(false);
    } catch {
      // Banner is set by `run()`.
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
          <AlertDialogCancel disabled={busy}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            disabled={busy || slug === null}
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
