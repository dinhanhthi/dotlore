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
import { BLOCKED, reportError, wipeCloudData } from "@/lib/ipc";
import { useRoots, useTaskLabel } from "@/lib/roots";

type WipeCloudDataAlertProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

export function WipeCloudDataAlert({
  open,
  onOpenChange,
}: WipeCloudDataAlertProps) {
  const { refreshRoots } = useRoots();
  const running = useTaskLabel() !== null;

  /** Closes first: the wipe runs as a footer task, the window stays usable. */
  async function confirm() {
    if (running) return;
    onOpenChange(false);
    try {
      const report = await wipeCloudData();
      if (report === BLOCKED) return;
      await refreshRoots();
      if (report.failed.length > 0) {
        const slugs = report.failed.map((f) => f.slug).join(", ");
        reportError(
          `Could not rebuild: ${slugs}. Add them again from the sidebar.`,
        );
      }
    } catch {
      // Banner is set by `runTask()`.
    }
  }

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Wipe all synced data?</AlertDialogTitle>
          <AlertDialogDescription>
            This deletes everything Dotlore has synced to your cloud folder,
            for every machine, and rebuilds each project from your current
            patterns. Files in your projects are not touched. Quit Dotlore and
            reset it on your other machines first, or they will upload the old data
            again.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={running}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            disabled={running}
            onClick={(event) => {
              event.preventDefault();
              void confirm();
            }}
          >
            Wipe
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
