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
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";

const MAX_LISTED = 10;

type TrashRestoreDialogProps = {
  open: boolean;
  /** Files whose live copy exists and differs from the trash copy. */
  rels: string[];
  /** Defaults to the trash wording, which lists several files. */
  title?: string;
  onCancel: () => void;
  onReplace: () => void;
  onKeepBoth: () => void;
};

export function TrashRestoreDialog({
  open,
  rels,
  title = "Some files already exist in the project",
  onCancel,
  onReplace,
  onKeepBoth,
}: TrashRestoreDialogProps) {
  const listed = rels.slice(0, MAX_LISTED);
  const more = rels.length - listed.length;

  return (
    <AlertDialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onCancel();
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>
            Replace moves the current file to the trash first. Keep both writes
            a copy named {"<name>.restored.<ext>"} and leaves the current file
            alone. Restored files sync to your other Macs.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <div className="flex flex-col gap-1">
          <ul className="flex flex-col gap-0.5 rounded-lg bg-muted px-3 py-2 font-path text-xs text-foreground">
            {listed.map((rel) => (
              <li key={rel} className="truncate" title={rel}>
                {rel}
              </li>
            ))}
          </ul>
          {more > 0 ? (
            <p className="text-sm text-muted-foreground">and {more} more</p>
          ) : null}
        </div>
        <AlertDialogFooter>
          <Tooltip>
            <TooltipTrigger render={<AlertDialogCancel aria-label="Cancel" />}>
              Cancel
            </TooltipTrigger>
            <TooltipContent>Cancel</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger
              render={
                <AlertDialogAction
                  variant="outline"
                  aria-label="Keep both"
                  onClick={(event) => {
                    event.preventDefault();
                    onKeepBoth();
                  }}
                />
              }
            >
              Keep both
            </TooltipTrigger>
            <TooltipContent>Keep both</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger
              render={
                <AlertDialogAction
                  aria-label="Replace"
                  onClick={(event) => {
                    event.preventDefault();
                    onReplace();
                  }}
                />
              }
            >
              Replace
            </TooltipTrigger>
            <TooltipContent>Replace</TooltipContent>
          </Tooltip>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
