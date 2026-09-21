import { Settings } from "lucide-react";
import { useEffect, useState } from "react";

import { RevealInFinderButton } from "@/components/layout/RevealInFinderButton";
import { ChangeCloudFolderDialog } from "@/components/settings/ChangeCloudFolderDialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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
import {
  defaultIgnore,
  defaultPatterns,
  loginItemEnabled,
  maxFileMb,
  maxSeedFolderMb,
  setDefaultIgnore,
  setDefaultPatterns,
  setLoginItem,
  setMaxFileMb,
  setMaxSeedFolderMb,
} from "@/lib/ipc";
import { useRoots } from "@/lib/roots";
import { readTheme, writeTheme, type Theme } from "@/lib/theme";

const fieldClass =
  "w-full min-w-0 resize-y rounded-2xl border border-input bg-input/30 px-3 py-2 font-mono text-xs outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50";

function linesToPatterns(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

/** Global seed patterns, never-list, and size ceilings. */
export function SettingsDefaults() {
  const { busy } = useRoots();
  const [patternsText, setPatternsText] = useState("");
  const [ignoreText, setIgnoreText] = useState("");
  const [fileMb, setFileMb] = useState(50);
  const [folderMb, setFolderMb] = useState(200);

  useEffect(() => {
    void Promise.all([
      defaultPatterns(),
      defaultIgnore(),
      maxFileMb(),
      maxSeedFolderMb(),
    ])
      .then(([patterns, ignore, file, folder]) => {
        setPatternsText(patterns.join("\n"));
        setIgnoreText(ignore);
        setFileMb(file);
        setFolderMb(folder);
      })
      .catch(() => {
        // getters failed; leave the 50 / 200 placeholders
      });
  }, []);

  async function commitPatterns() {
    if (busy) return;
    try {
      await setDefaultPatterns(linesToPatterns(patternsText));
    } catch {
      void defaultPatterns()
        .then((patterns) => setPatternsText(patterns.join("\n")))
        .catch(() => {});
    }
  }

  async function commitIgnore() {
    if (busy) return;
    try {
      await setDefaultIgnore(ignoreText);
    } catch {
      void defaultIgnore()
        .then(setIgnoreText)
        .catch(() => {});
    }
  }

  async function commitMb(
    raw: number,
    write: (mb: number) => Promise<void>,
    read: () => Promise<number>,
    setLocal: (mb: number) => void,
  ) {
    if (busy) return;
    const mb = Math.round(raw);
    if (!Number.isFinite(mb) || mb <= 0) {
      void read()
        .then(setLocal)
        .catch(() => {});
      return;
    }
    setLocal(mb);
    try {
      await write(mb);
    } catch {
      void read()
        .then(setLocal)
        .catch(() => {});
    }
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-1.5">
        <label htmlFor="default-patterns" className="text-sm">
          Default patterns
        </label>
        <textarea
          id="default-patterns"
          rows={5}
          spellCheck={false}
          className={fieldClass}
          value={patternsText}
          disabled={busy}
          onChange={(event) => setPatternsText(event.target.value)}
          onBlur={() => {
            void commitPatterns();
          }}
        />
        <p className="text-xs text-muted-foreground">
          Applies to projects added from now on, not existing ones. Does not
          affect agent folders (~/.claude, ~/.cursor, …), which always seed
          from the built-in per-agent lists.
        </p>
      </div>
      <div className="flex flex-col gap-1.5">
        <label htmlFor="default-ignore" className="text-sm">
          Default never-list
        </label>
        <textarea
          id="default-ignore"
          rows={5}
          spellCheck={false}
          className={fieldClass}
          value={ignoreText}
          disabled={busy}
          onChange={(event) => setIgnoreText(event.target.value)}
          onBlur={() => {
            void commitIgnore();
          }}
        />
        <p className="text-xs text-muted-foreground">
          The never-list also applies to projects added from now on, not
          existing ones, and to both projects and agent folders.
        </p>
      </div>
      <div className="flex flex-col gap-1.5">
        <div className="flex items-center justify-between gap-3">
          <label htmlFor="max-file-mb" className="text-sm">
            Max file size (MB)
          </label>
          <Input
            id="max-file-mb"
            type="number"
            min={1}
            step={1}
            className="w-20"
            value={fileMb}
            disabled={busy}
            onChange={(event) => setFileMb(Number(event.target.value))}
            onBlur={() => {
              void commitMb(fileMb, setMaxFileMb, maxFileMb, setFileMb);
            }}
          />
        </div>
        <p className="text-xs text-muted-foreground">
          Hard limit checked on every sync — a file above it is never tracked.
        </p>
      </div>
      <div className="flex flex-col gap-1.5">
        <div className="flex items-center justify-between gap-3">
          <label htmlFor="max-seed-folder-mb" className="text-sm">
            Max folder size when adding (MB)
          </label>
          <Input
            id="max-seed-folder-mb"
            type="number"
            min={1}
            step={1}
            className="w-20"
            value={folderMb}
            disabled={busy}
            onChange={(event) => setFolderMb(Number(event.target.value))}
            onBlur={() => {
              void commitMb(
                folderMb,
                setMaxSeedFolderMb,
                maxSeedFolderMb,
                setFolderMb,
              );
            }}
          />
        </div>
        <p className="text-xs text-muted-foreground">
          Only applies when a project or an entry is first added, to stop a
          directory of many small files (like ~/.cursor/chats/, 4.1 GB) being
          pulled in wholesale.
        </p>
      </div>
    </div>
  );
}

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

/** Short service label from the connected folder path. */
function providerServiceLabel(path: string): string {
  const gdrive = /\/CloudStorage\/GoogleDrive-([^/]+)/.exec(path);
  if (gdrive?.[1]) {
    const account = gdrive[1].includes("@")
      ? gdrive[1].slice(0, gdrive[1].indexOf("@"))
      : gdrive[1];
    return `GDrive ${account}`;
  }
  if (path.includes("/Mobile Documents/com~apple~CloudDocs")) return "iCloud";
  return "Other";
}

export function SettingsPopover() {
  const { providerDir, busy } = useRoots();
  const [open, setOpen] = useState(false);
  const [changeOpen, setChangeOpen] = useState(false);
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
    <>
    <Popover open={open} onOpenChange={setOpen}>
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
        <Settings className="size-3.5" aria-hidden />
      </PopoverTrigger>
      <PopoverContent
        side="top"
        align="end"
        className="max-h-[min(40rem,calc(100vh-1.5rem))] w-80 overflow-y-auto"
      >
        <PopoverHeader>
          <PopoverTitle>Settings</PopoverTitle>
        </PopoverHeader>
        <div className="flex flex-col gap-2">
          <div className="flex items-center gap-2">
            <span className="text-sm">Cloud folder</span>
            {providerDir !== null && (
              <Badge
                variant="secondary"
                className="max-w-32 truncate font-normal"
              >
                {providerServiceLabel(providerDir)}
              </Badge>
            )}
          </div>
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
            <RevealInFinderButton path={providerDir} />
            <Button
              variant="secondary"
              size="xs"
              onClick={() => {
                setOpen(false);
                setChangeOpen(true);
              }}
            >
              Change…
            </Button>
          </div>
        </div>
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
        <SettingsDefaults />
      </PopoverContent>
    </Popover>
    <ChangeCloudFolderDialog open={changeOpen} onOpenChange={setChangeOpen} />
    </>
  );
}
