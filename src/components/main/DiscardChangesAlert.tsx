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

type DiscardChangesAlertProps = {
  open: boolean;
  fileName: string;
  onCancel: () => void;
  onDiscard: () => void;
};

/** Asks before leaving the resolver with unsaved Result changes. */
export function DiscardChangesAlert({
  open,
  fileName,
  onCancel,
  onDiscard,
}: DiscardChangesAlertProps) {
  return (
    <AlertDialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onCancel();
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Discard changes to {fileName}?</AlertDialogTitle>
          <AlertDialogDescription>
            Your picks and edits in Result will be lost.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction variant="destructive" onClick={onDiscard}>
            Discard
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
