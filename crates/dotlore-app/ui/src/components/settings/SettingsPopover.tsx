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
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { loginItemEnabled, setLoginItem } from "@/lib/ipc";
import { useRoots } from "@/lib/roots";
import { readTheme, writeTheme, type Theme } from "@/lib/theme";

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
  const [theme, setTheme] = useState<Theme>(() => readTheme());

  useEffect(() => {
    void loginItemEnabled()
      .then(setLoginOn)
      .catch(() => {
        setLoginOn(false);
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
            className="text-muted-foreground"
            aria-label="Settings"
          />
        }
      >
        <Settings aria-hidden />
      </PopoverTrigger>
      <PopoverContent side="top" align="end" className="w-80">
        <PopoverHeader>
          <PopoverTitle>Settings</PopoverTitle>
        </PopoverHeader>
        <div className="flex flex-col gap-2">
          <span className="text-sm">Cloud folder</span>
          <div className="flex items-center gap-2">
            <Tooltip>
              <TooltipTrigger
                render={
                  <button
                    type="button"
                    className="min-w-0 flex-1 truncate text-left font-mono text-xs text-muted-foreground"
                  />
                }
              >
                {folder}
              </TooltipTrigger>
              <TooltipContent>
                {providerDir ?? "No cloud folder set"}
              </TooltipContent>
            </Tooltip>
            <Button
              variant="secondary"
              size="xs"
              onClick={() => setChanging((open) => !open)}
            >
              Change…
            </Button>
          </div>
        </div>
        {changing ? (
          <ProviderChooser compact onApplied={() => setChanging(false)} />
        ) : null}
        <div className="flex items-center justify-between gap-3">
          <span className="text-sm">Appearance</span>
          <div className="flex rounded-4xl border border-border p-0.5">
            <Button
              type="button"
              variant={theme === "light" ? "secondary" : "ghost"}
              size="xs"
              aria-pressed={theme === "light"}
              onClick={() => {
                setTheme("light");
                writeTheme("light");
              }}
            >
              Light
            </Button>
            <Button
              type="button"
              variant={theme === "dark" ? "secondary" : "ghost"}
              size="xs"
              aria-pressed={theme === "dark"}
              onClick={() => {
                setTheme("dark");
                writeTheme("dark");
              }}
            >
              Dark
            </Button>
          </div>
        </div>
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
      </PopoverContent>
    </Popover>
  );
}
