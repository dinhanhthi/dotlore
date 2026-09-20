import { getVersion } from "@tauri-apps/api/app";
import { Settings } from "lucide-react";
import { useEffect, useState } from "react";

import { ProviderChooser } from "@/components/setup/ProviderChooser";
import { Button } from "@/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverHeader,
  PopoverTitle,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";
import { loginItemEnabled, setLoginItem } from "@/lib/ipc";
import { useRoots } from "@/lib/roots";

const APP_VERSION = "0.1.0";
export const GIT_INSTALL_CMD = "xcode-select --install";
export const GIT_MISSING_BANNER = "git not found — run: xcode-select --install";

export function GitMissingBanner() {
  const [copied, setCopied] = useState(false);

  function copy() {
    void navigator.clipboard.writeText(GIT_INSTALL_CMD).then(
      () => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      },
      () => {
        // WebView clipboard can be denied; the command stays on screen.
      },
    );
  }

  return (
    <div className="flex shrink-0 items-center gap-2 border-b border-destructive/40 bg-destructive/10 px-3 py-1.5 text-destructive">
      <p className="min-w-0 flex-1">{GIT_MISSING_BANNER}</p>
      <Button
        variant="ghost"
        size="xs"
        className="text-destructive hover:text-destructive"
        onClick={copy}
      >
        {copied ? "Copied" : "Copy"}
      </Button>
    </div>
  );
}

function tildePath(path: string): string {
  return path.replace(/^\/Users\/[^/]+/, "~");
}

export function SettingsPopover() {
  const { providerDir, busy } = useRoots();
  const [changing, setChanging] = useState(false);
  const [loginOn, setLoginOn] = useState(false);
  const [version, setVersion] = useState(APP_VERSION);

  useEffect(() => {
    void loginItemEnabled()
      .then(setLoginOn)
      .catch(() => {
        setLoginOn(false);
      });
    void getVersion()
      .then(setVersion)
      .catch(() => {
        setVersion(APP_VERSION);
      });
  }, []);

  async function toggleLogin(on: boolean) {
    if (busy) return;
    const previous = loginOn;
    setLoginOn(on);
    try {
      await setLoginItem(on);
    } catch {
      setLoginOn(previous);
    }
  }

  const folder = providerDir === null ? "No folder" : tildePath(providerDir);

  return (
    <Popover
      onOpenChange={(open) => {
        if (!open) setChanging(false);
      }}
    >
      <PopoverTrigger
        render={
          <Button
            variant="ghost"
            size="icon-xs"
            className="size-5 text-muted-foreground"
            aria-label="Settings"
          />
        }
      >
        <Settings aria-hidden />
      </PopoverTrigger>
      <PopoverContent side="top" align="end" className="w-80">
        <PopoverHeader>
          <PopoverTitle>Cloud folder</PopoverTitle>
        </PopoverHeader>
        <div className="flex items-center gap-2">
          <span
            className="min-w-0 flex-1 truncate font-path text-muted-foreground"
            title={providerDir ?? "No cloud folder set"}
          >
            {folder}
          </span>
          <Button
            variant="ghost"
            size="xs"
            onClick={() => setChanging((open) => !open)}
          >
            Change…
          </Button>
        </div>
        {changing ? (
          <ProviderChooser compact onApplied={() => setChanging(false)} />
        ) : null}
        <div className="flex items-center justify-between gap-3">
          <label htmlFor="start-at-login" className="text-sm">
            Start at login
          </label>
          <Switch
            id="start-at-login"
            checked={loginOn}
            disabled={busy}
            onCheckedChange={(on) => {
              void toggleLogin(on);
            }}
          />
        </div>
        <p className="text-xs text-muted-foreground">Dotlore {version}</p>
      </PopoverContent>
    </Popover>
  );
}
