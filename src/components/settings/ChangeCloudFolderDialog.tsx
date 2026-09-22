import { ProviderChooser } from "@/components/setup/ProviderChooser";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

type ChangeCloudFolderDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

export function ChangeCloudFolderDialog({
  open,
  onOpenChange,
}: ChangeCloudFolderDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Choose a cloud folder</DialogTitle>
          <DialogDescription>
            Dotlore syncs through a folder you already sync — iCloud Drive,
            Google Drive, or any other.
          </DialogDescription>
        </DialogHeader>
        <ProviderChooser onApplied={() => onOpenChange(false)} />
      </DialogContent>
    </Dialog>
  );
}
