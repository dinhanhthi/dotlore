import { useState } from "react";

import { Button } from "@/components/ui/button";
import { errorMessage } from "@/lib/errors";
import { icloudDir, listGdriveMounts, setProvider } from "@/lib/ipc";
import { pickLocalPath } from "@/lib/pick";
import { useRoots } from "@/lib/roots";

/** What the Google Drive client calls the account's own root. */
const MY_DRIVE = "My Drive";

function mountName(path: string): string {
  const name = path.split("/").filter(Boolean).pop();
  return name && name.length > 0 ? name : path;
}

type ProviderChooserProps = {
  /** Called after `set_provider` succeeds and local state is updated. */
  onApplied?: () => void;
  /** Smaller outline buttons for the settings popover. */
  compact?: boolean;
};

export function ProviderChooser({
  onApplied,
  compact = false,
}: ProviderChooserProps) {
  const { applyProvider, busy, setBanner } = useRoots();
  const [mounts, setMounts] = useState<string[] | null>(null);
  const [localBusy, setLocalBusy] = useState(false);
  const locked = busy || localBusy;

  async function apply(dir: string) {
    await setProvider(dir);
    applyProvider(dir);
    onApplied?.();
  }

  async function withLock(action: () => Promise<void>) {
    if (locked) return;
    setLocalBusy(true);
    try {
      await action();
    } catch (err) {
      setBanner(errorMessage(err, "Could not set the cloud folder"));
    } finally {
      setLocalBusy(false);
    }
  }

  const buttonSize = compact ? "sm" : "default";

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-col gap-2">
        <Button
          variant="outline"
          size={buttonSize}
          className="w-full"
          disabled={locked}
          onClick={() => {
            void withLock(async () => {
              await apply(await icloudDir());
            });
          }}
        >
          iCloud Drive
        </Button>
        <Button
          variant="outline"
          size={buttonSize}
          className="w-full"
          disabled={locked}
          onClick={() => {
            void listGdriveMounts()
              .then(setMounts)
              .catch((err) => {
                setBanner(errorMessage(err, "Could not list Google Drive folders"));
              });
          }}
        >
          Google Drive…
        </Button>
        <Button
          variant="outline"
          size={buttonSize}
          className="w-full"
          disabled={locked}
          onClick={() => {
            void withLock(async () => {
              const dir = await pickLocalPath(true);
              if (dir === null) return;
              await apply(dir);
            });
          }}
        >
          Other…
        </Button>
      </div>
      {mounts !== null &&
        (mounts.length === 0 ? (
          <p className="text-muted-foreground">No Google Drive folder found</p>
        ) : (
          <div className="flex flex-col items-start gap-1">
            {mounts.map((mount) => (
              <Button
                key={mount}
                variant="ghost"
                size="sm"
                disabled={locked}
                onClick={() => {
                  void withLock(async () => {
                    await apply(`${mount}/${MY_DRIVE}`);
                  });
                }}
              >
                {mountName(mount)}
              </Button>
            ))}
          </div>
        ))}
    </div>
  );
}
