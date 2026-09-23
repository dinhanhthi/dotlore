import { useState } from "react";
import { ChevronDown } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { errorMessage } from "@/lib/errors";
import { icloudDir, listGdriveMounts, setProvider } from "@/lib/ipc";
import { pickLocalPath } from "@/lib/pick";
import { useRoots } from "@/lib/roots";

/** What the Google Drive client calls the account's own root. */
const MY_DRIVE = "My Drive";

type Provider = "icloud" | "gdrive" | "other";

const PROVIDER_LABELS: Record<Provider, string> = {
  icloud: "iCloud Drive",
  gdrive: "Google Drive",
  other: "Other…",
};

function mountName(path: string): string {
  const name = path.split("/").filter(Boolean).pop();
  return name && name.length > 0 ? name : path;
}

type ProviderChooserProps = {
  /** Called after `set_provider` succeeds and local state is updated. */
  onApplied?: () => void;
};

export function ProviderChooser({ onApplied }: ProviderChooserProps) {
  const { applyProvider, busy, setBanner } = useRoots();
  const [provider, setProviderChoice] = useState<Provider | null>(null);
  const [mounts, setMounts] = useState<string[] | null>(null);
  const [account, setAccount] = useState<string | null>(null);
  const [localBusy, setLocalBusy] = useState(false);
  const locked = busy || localBusy;

  /**
   * Closes first, then writes: `set_provider` runs as a footer task, so the
   * window stays usable. `applyProvider` waits for it — it re-reads
   * `provider_dir`, which would still be the old folder until then.
   */
  async function apply(dir: string) {
    onApplied?.();
    if ((await setProvider(dir)) === null) return;
    applyProvider(dir);
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

  function chooseProvider(next: Provider) {
    setProviderChoice(next);
    setMounts(null);
    setAccount(null);
    if (next === "icloud") {
      void withLock(async () => {
        await apply(await icloudDir());
      });
      return;
    }
    if (next === "other") {
      void withLock(async () => {
        const dir = await pickLocalPath();
        if (dir === null) return;
        await apply(dir);
      });
      return;
    }
    void listGdriveMounts()
      .then(setMounts)
      .catch((err) => {
        setBanner(errorMessage(err, "Could not list Google Drive folders"));
      });
  }

  function chooseAccount(mount: string) {
    setAccount(mount);
    void withLock(async () => {
      await apply(`${mount}/${MY_DRIVE}`);
    });
  }

  return (
    <div className="flex flex-col gap-2">
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
          {provider === null ? "Select a provider" : PROVIDER_LABELS[provider]}
          <ChevronDown aria-hidden />
        </DropdownMenuTrigger>
        <DropdownMenuContent>
          <DropdownMenuRadioGroup
            value={provider ?? ""}
            onValueChange={(value) => chooseProvider(value as Provider)}
          >
            {(Object.keys(PROVIDER_LABELS) as Provider[]).map((id) => (
              <DropdownMenuRadioItem key={id} value={id}>
                {PROVIDER_LABELS[id]}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>

      {provider === "gdrive" && mounts !== null && (
        mounts.length === 0 ? (
          <p className="text-muted-foreground">No Google Drive folder found</p>
        ) : (
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
              {account === null ? "Select an account" : mountName(account)}
              <ChevronDown aria-hidden />
            </DropdownMenuTrigger>
            <DropdownMenuContent>
              <DropdownMenuRadioGroup
                value={account ?? ""}
                onValueChange={chooseAccount}
              >
                {mounts.map((mount) => (
                  <DropdownMenuRadioItem key={mount} value={mount}>
                    {mountName(mount)}
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
            </DropdownMenuContent>
          </DropdownMenu>
        )
      )}
    </div>
  );
}
