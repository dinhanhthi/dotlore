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
import { setBanner, wipeCloudData } from "@/lib/ipc";
import { useRoots } from "@/lib/roots";

type WipeCloudDataAlertProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

export function WipeCloudDataAlert({
  open,
  onOpenChange,
}: WipeCloudDataAlertProps) {
  const { refreshRoots, busy } = useRoots();

  async function confirm() {
    if (busy) return;
    try {
      const report = await wipeCloudData();
      await refreshRoots();
      if (report.failed.length > 0) {
        const slugs = report.failed.map((f) => f.slug).join(", ");
        setBanner(
          `Could not rebuild: ${slugs}. Add them again from the sidebar.`,
        );
      }
    } catch {
      // Banner is set by `run()`.
    }
    onOpenChange(false);
  }

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Wipe all synced data?</AlertDialogTitle>
          <AlertDialogDescription>
            This deletes everything Dotlore has synced to your cloud folder,
            for every Mac, and rebuilds each project from your current
            patterns. Files in your projects are not touched. Quit Dotlore and
            reset it on your other Macs first, or they will upload the old data
            again.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={busy}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            disabled={busy}
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
