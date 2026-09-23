import { useState } from "react";
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
import { errorMessage } from "@/lib/errors";
import { BLOCKED, icloudDir, listGdriveMounts, setProvider } from "@/lib/ipc";
import { middleEllipsis } from "@/lib/path";
import { pickLocalPath } from "@/lib/pick";
import { useRoots } from "@/lib/roots";

/** What the Google Drive client calls the account's own root. */
const MY_DRIVE = "My Drive";

export const APPLY_LABEL = "Use this folder";
export const CANCEL_LABEL = "Cancel";

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

/** The folder a Google Drive account mount syncs through. */
export function accountDir(mount: string): string {
  return `${mount}/${MY_DRIVE}`;
}

type ProviderChooserProps = {
  /** Called as soon as the folder is confirmed, before `set_provider` runs. */
  onApplied?: () => void;
  /** Dismiss without writing. Omitted during onboarding, which has no way out. */
  onCancel?: () => void;
};

export function ProviderChooser({ onApplied, onCancel }: ProviderChooserProps) {
  const { applyProvider, locked: appLocked, setBanner } = useRoots();
  const [provider, setProviderChoice] = useState<Provider | null>(null);
  const [mounts, setMounts] = useState<string[] | null>(null);
  const [account, setAccount] = useState<string | null>(null);
  // The folder the next apply commits to. Resolved while choosing, so the
  // button can say what it will do and stay disabled until it can do it.
  const [dir, setDir] = useState<string | null>(null);
  const [localBusy, setLocalBusy] = useState(false);
  const locked = appLocked || localBusy;

  /**
   * Closes first, then writes: `set_provider` runs as a footer task, so the
   * window stays usable. `applyProvider` waits for it — it re-reads
   * `provider_dir`, which would still be the old folder until then.
   */
  async function apply(target: string) {
    onApplied?.();
    if ((await setProvider(target)) === BLOCKED) return;
    applyProvider(target);
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

  /** Choosing only resolves a folder; nothing is written until apply. */
  function chooseProvider(next: Provider) {
    setProviderChoice(next);
    setMounts(null);
    setAccount(null);
    setDir(null);
    if (next === "icloud") {
      void withLock(async () => {
        setDir(await icloudDir());
      });
      return;
    }
    if (next === "other") {
      void withLock(async () => {
        setDir(await pickLocalPath());
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
    setDir(accountDir(mount));
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
