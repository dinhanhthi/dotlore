import { Settings } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { RevealInFinderButton } from "@/components/layout/RevealInFinderButton";
import { ChangeCloudFolderDialog } from "@/components/settings/ChangeCloudFolderDialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
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

type SettingsTab = "general" | "patterns";

function linesToPatterns(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

async function commitMb(
  raw: number,
  write: (mb: number) => Promise<void>,
  read: () => Promise<number>,
  setLocal: (mb: number) => void,
  busy: boolean,
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

function SettingsLimits() {
  const { busy } = useRoots();
  const [fileMb, setFileMb] = useState(50);
  const [folderMb, setFolderMb] = useState(200);

  useEffect(() => {
    void Promise.all([maxFileMb(), maxSeedFolderMb()])
      .then(([file, folder]) => {
        setFileMb(file);
        setFolderMb(folder);
      })
      .catch(() => {
        // getters failed; leave the 50 / 200 placeholders
      });
  }, []);

  return (
    <section className="flex flex-col gap-3">
      <h3 className="text-label text-muted-foreground">Size limits</h3>
      <div className="flex flex-col gap-4">
        <div className="flex flex-col gap-1.5">
          <div className="flex items-center justify-between gap-3">
            <label htmlFor="max-file-mb" className="text-sm">
              Max file size
            </label>
            <div className="flex items-center gap-1.5">
              <Input
                id="max-file-mb"
                type="number"
                min={1}
                step={1}
                className="w-24"
                value={fileMb}
                disabled={busy}
                onChange={(event) => setFileMb(Number(event.target.value))}
                onBlur={() => {
                  void commitMb(fileMb, setMaxFileMb, maxFileMb, setFileMb, busy);
                }}
              />
              <span className="text-xs text-muted-foreground">MB</span>
            </div>
          </div>
          <p className="text-xs text-muted-foreground">
            Hard limit checked on every sync — a file above it is never tracked.
          </p>
        </div>
        <div className="flex flex-col gap-1.5">
          <div className="flex items-center justify-between gap-3">
            <label htmlFor="max-seed-folder-mb" className="text-sm">
              Max folder size when adding
            </label>
            <div className="flex items-center gap-1.5">
              <Input
                id="max-seed-folder-mb"
                type="number"
                min={1}
                step={1}
                className="w-24"
                value={folderMb}
                disabled={busy}
                onChange={(event) => setFolderMb(Number(event.target.value))}
                onBlur={() => {
                  void commitMb(
                    folderMb,
                    setMaxSeedFolderMb,
                    maxSeedFolderMb,
                    setFolderMb,
                    busy,
                  );
                }}
              />
              <span className="text-xs text-muted-foreground">MB</span>
            </div>
          </div>
          <p className="text-xs text-muted-foreground">
            Only applies when a project or an entry is first added, to stop a
            directory of many small files (like ~/.cursor/chats/, 4.1 GB) being
            pulled in wholesale.
          </p>
        </div>
      </div>
    </section>
  );
}

/** Global seed patterns and never-list. */
export function SettingsDefaults() {
  const { busy } = useRoots();
  const [patternsText, setPatternsText] = useState("");
  const [ignoreText, setIgnoreText] = useState("");

  useEffect(() => {
    void Promise.all([defaultPatterns(), defaultIgnore()])
      .then(([patterns, ignore]) => {
        setPatternsText(patterns.join("\n"));
        setIgnoreText(ignore);
      })
      .catch(() => {
        // getters failed; leave the fields empty
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

  return (
    <div className="flex flex-col gap-5">
      <div className="flex flex-col gap-1.5">
        <label htmlFor="default-patterns" className="text-sm">
          Default patterns
        </label>
        <textarea
          id="default-patterns"
          rows={6}
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
          rows={6}
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
    </div>
  );
}

function SettingsGeneral({
  onChangeFolder,
}: {
  onChangeFolder: () => void;
}) {
  const { providerDir, busy } = useRoots();
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
    <div className="flex flex-col gap-6">
      <section className="flex flex-col gap-2">
        <h3 className="text-label text-muted-foreground">Cloud</h3>
        <div className="rounded-2xl bg-muted/40 p-3 ring-1 ring-foreground/5">
          <div className="flex items-center justify-between gap-3">
            <div className="flex min-w-0 items-center gap-2">
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
            <div className="flex shrink-0 items-center gap-1.5">
              <RevealInFinderButton path={providerDir} />
              <Button variant="secondary" size="xs" onClick={onChangeFolder}>
                Change…
              </Button>
            </div>
          </div>
          <Tooltip>
            <TooltipTrigger
              render={
                <button
                  type="button"
                  className="mt-1.5 block w-full min-w-0 truncate text-left font-mono text-xs text-muted-foreground"
                />
              }
            >
              {folder}
            </TooltipTrigger>
            <TooltipContent>
              {providerDir ?? "No cloud folder set"}
            </TooltipContent>
          </Tooltip>
        </div>
      </section>
      <section className="flex flex-col gap-3">
        <h3 className="text-label text-muted-foreground">Preferences</h3>
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
      </section>
      <SettingsLimits />
    </div>
  );
}

export function SettingsPanel({
  onChangeFolder,
}: {
  onChangeFolder?: () => void;
}) {
  const [tab, setTab] = useState<SettingsTab>("general");

  return (
    <>
      <DialogHeader className="gap-4 border-b border-border px-6 pt-6 pr-14 pb-4">
        <div className="flex flex-col gap-1">
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription>
            Cloud folder, appearance, and what new projects track.
          </DialogDescription>
        </div>
        <div
          role="tablist"
          aria-label="Settings sections"
          className="grid grid-cols-2 rounded-4xl border border-border p-0.5"
        >
          <Button
            type="button"
            role="tab"
            id="settings-tab-general"
            aria-controls="settings-panel-general"
            aria-selected={tab === "general"}
            variant={tab === "general" ? "default" : "ghost"}
            size="xs"
            className="w-full"
            onClick={() => setTab("general")}
          >
            General
          </Button>
          <Button
            type="button"
            role="tab"
            id="settings-tab-patterns"
            aria-controls="settings-panel-patterns"
            aria-selected={tab === "patterns"}
            variant={tab === "patterns" ? "default" : "ghost"}
            size="xs"
            className="w-full"
            onClick={() => setTab("patterns")}
          >
            Patterns
          </Button>
        </div>
      </DialogHeader>
      <div className="h-[28rem] overflow-y-auto px-6 pt-5 pb-7">
        {tab === "general" ? (
          <div
            role="tabpanel"
            id="settings-panel-general"
            aria-labelledby="settings-tab-general"
          >
            <SettingsGeneral onChangeFolder={onChangeFolder ?? (() => {})} />
          </div>
        ) : (
          <div
            role="tabpanel"
            id="settings-panel-patterns"
            aria-labelledby="settings-tab-patterns"
          >
            <SettingsDefaults />
          </div>
        )}
      </div>
    </>
  );
}

export function SettingsDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [changeOpen, setChangeOpen] = useState(false);
  const pendingChange = useRef(false);

  useEffect(() => {
    if (open || !pendingChange.current) return;
    pendingChange.current = false;
    setChangeOpen(true);
  }, [open]);

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="gap-0 overflow-hidden p-0 sm:max-w-xl">
          <SettingsPanel
            key={open ? "open" : "closed"}
            onChangeFolder={() => {
              pendingChange.current = true;
              onOpenChange(false);
            }}
          />
        </DialogContent>
      </Dialog>
      <ChangeCloudFolderDialog open={changeOpen} onOpenChange={setChangeOpen} />
    </>
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
  const [open, setOpen] = useState(false);

  return (
    <>
      <Button
        variant="ghost"
        size="icon-xs"
        className="text-muted-foreground"
        aria-label="Settings"
        onClick={() => setOpen(true)}
      >
        <Settings className="size-3.5" aria-hidden />
      </Button>
      <SettingsDialog open={open} onOpenChange={setOpen} />
    </>
  );
}
