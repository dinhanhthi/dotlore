import { useEffect, useState } from "react";
import { ChevronDown } from "lucide-react";

import { RevealInFinderButton } from "@/components/layout/RevealInFinderButton";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { mountDir, mountLabel } from "@/lib/cloud-mounts";
import { errorMessage } from "@/lib/errors";
import {
  BLOCKED,
  icloudDir,
  listCloudMounts,
  reportError,
  setProvider,
} from "@/lib/ipc";
import { middleEllipsis } from "@/lib/path";
import { pickLocalPath } from "@/lib/pick";
import { useRoots } from "@/lib/roots";

export const APPLY_LABEL = "Use this folder";
export const CANCEL_LABEL = "Cancel";

const ICLOUD = "icloud";
const OTHER = "other";

function mountName(path: string): string {
  const name = path.split("/").filter(Boolean).pop();
  return name && name.length > 0 ? name : path;
}

/** `ICLOUD`, `OTHER`, or a mount path from `listCloudMounts()`. */
function choiceLabel(choice: string): string {
  if (choice === ICLOUD) return "iCloud Drive";
  if (choice === OTHER) return "Other…";
  return mountLabel(mountName(choice));
}

type ProviderChooserProps = {
  /** Called as soon as the folder is confirmed, before `set_provider` runs. */
  onApplied?: () => void;
  /** Dismiss without writing. Omitted during onboarding, which has no way out. */
  onCancel?: () => void;
};

export function ProviderChooser({ onApplied, onCancel }: ProviderChooserProps) {
  const { applyProvider, locked: appLocked } = useRoots();
  const [choice, setChoice] = useState<string | null>(null);
  const [mounts, setMounts] = useState<string[]>([]);
  // The folder the next apply commits to. Resolved while choosing, so the
  // button can say what it will do and stay disabled until it can do it.
  const [dir, setDir] = useState<string | null>(null);
  const [localBusy, setLocalBusy] = useState(false);
  const locked = appLocked || localBusy;

  useEffect(() => {
    void listCloudMounts()
      .then(setMounts)
      .catch((err) => {
        reportError(errorMessage(err, "Could not list cloud folders"));
      });
  }, []);

  /**
   * Closes first, then writes: `set_provider` runs as a footer task, so the
   * window stays usable. `applyProvider` waits for it — it re-reads
   * `provider_dir`, which would still be the old folder until then.
   */
  async function apply(target: string) {
    onApplied?.();
    try {
      if ((await setProvider(target)) === BLOCKED) return;
    } catch {
      // `runTask()` already logged the failure.
      return;
    }
    applyProvider(target);
  }

  async function withLock(action: () => Promise<void>) {
    if (locked) return;
    setLocalBusy(true);
    try {
      await action();
    } catch (err) {
      reportError(errorMessage(err, "Could not set the cloud folder"));
    } finally {
      setLocalBusy(false);
    }
  }

  /** Choosing only resolves a folder; nothing is written until apply. */
  function chooseProvider(next: string) {
    setChoice(next);
    setDir(null);
    if (next === ICLOUD) {
      void withLock(async () => {
        setDir(await icloudDir());
      });
      return;
    }
    if (next === OTHER) {
      void withLock(async () => {
        setDir(await pickLocalPath());
      });
      return;
    }
    setDir(mountDir(next));
  }

  function confirm() {
    if (dir === null) return;
    void withLock(() => apply(dir));
  }

  return (
    <div className="flex min-w-0 flex-col gap-2">
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <Button
              type="button"
              variant="outline"
              disabled={locked}
              className="w-full justify-between"
            />
          }
        >
          {choice === null ? "Select a provider" : choiceLabel(choice)}
          <ChevronDown aria-hidden />
        </DropdownMenuTrigger>
        <DropdownMenuContent>
          <DropdownMenuRadioGroup
            value={choice ?? ""}
            onValueChange={(value) => chooseProvider(value)}
          >
            {[ICLOUD, ...mounts, OTHER].map((id) => (
              <DropdownMenuRadioItem key={id} value={id}>
                {choiceLabel(id)}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>

      {dir !== null && (
        <div className="flex min-w-0 items-center gap-1">
          <span
            className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground"
            title={dir}
          >
            {middleEllipsis(dir)}
          </span>
          <RevealInFinderButton path={dir} />
        </div>
      )}

      <div className="mt-2 flex items-center gap-2">
        {onCancel && (
          <Button
            type="button"
            variant="outline"
            className="flex-1"
            onClick={onCancel}
          >
            {CANCEL_LABEL}
          </Button>
        )}
        <Button
          type="button"
          className="flex-1"
          disabled={locked || dir === null}
          onClick={confirm}
        >
          {APPLY_LABEL}
        </Button>
      </div>
    </div>
  );
}
