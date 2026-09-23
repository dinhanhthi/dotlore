import { ChevronDown, Loader2, Pencil, Settings, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { RevealInFinderButton } from "@/components/layout/RevealInFinderButton";
import { ChangeCloudFolderDialog } from "@/components/settings/ChangeCloudFolderDialog";
import { SettingsSeedList } from "@/components/settings/SettingsSeedList";
import { WipeCloudDataAlert } from "@/components/settings/WipeCloudDataAlert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  defaultIgnore,
  loginItemEnabled,
  maxFileMb,
  maxSeedFolderMb,
  patternCatalogs,
  setDefaultIgnore,
  setLoginItem,
  setMaxFileMb,
  setMaxSeedFolderMb,
  setPatternCatalog,
  WIPE_LABEL,
  type PatternCatalog,
} from "@/lib/ipc";
import { useRoots, useTaskLabel } from "@/lib/roots";
import { useTheme } from "@/lib/theme";

const SETTINGS_TABS = [
  { id: "general", label: "General" },
  { id: "patterns", label: "Patterns" },
  { id: "never", label: "Never-list" },
] as const;

type SettingsTab = (typeof SETTINGS_TABS)[number]["id"];

function ignoreToLines(text: string): string[] {
  return text.split("\n").filter((line) => line.length > 0);
}

async function commitMb(
  raw: number,
  write: (mb: number) => Promise<void>,
  read: () => Promise<number>,
  setLocal: (mb: number) => void,
  locked: boolean,
) {
  if (locked) return;
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
  const { locked } = useRoots();
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
                disabled={locked}
                onChange={(event) => setFileMb(Number(event.target.value))}
                onBlur={() => {
                  void commitMb(fileMb, setMaxFileMb, maxFileMb, setFileMb, locked);
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
                disabled={locked}
                onChange={(event) => setFolderMb(Number(event.target.value))}
                onBlur={() => {
                  void commitMb(
                    folderMb,
                    setMaxSeedFolderMb,
                    maxSeedFolderMb,
                    setFolderMb,
                    locked,
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

/** Global seed include-list for new projects and agent folders. */
export function SettingsPatterns() {
  const { locked } = useRoots();
  const [catalogs, setCatalogs] = useState<PatternCatalog[]>([]);
  const [selectedId, setSelectedId] = useState("projects");
  const [lines, setLines] = useState<string[]>([]);
  const selectedIdRef = useRef(selectedId);
  selectedIdRef.current = selectedId;

  useEffect(() => {
    void patternCatalogs()
      .then((next) => {
        const chosen =
          next.find((catalog) => catalog.id === selectedIdRef.current) ??
          next[0];
        setCatalogs(next);
        if (!chosen) return;
        setSelectedId(chosen.id);
        setLines(chosen.lines);
      })
      .catch(() => {
        // getter failed; leave the catalog list empty
      });
  }, []);

  function selectCatalog(id: string) {
    const catalog = catalogs.find((item) => item.id === id);
    if (!catalog) return;
    setSelectedId(catalog.id);
    setLines(catalog.lines);
  }

  async function commit(next: string[]) {
    if (locked) return;
    const catalogId = selectedId;
    try {
      await setPatternCatalog(catalogId, next);
      setCatalogs((current) =>
        current.map((catalog) =>
          catalog.id === catalogId ? { ...catalog, lines: next } : catalog,
        ),
      );
      if (selectedIdRef.current === catalogId) setLines(next);
    } catch {
      void patternCatalogs()
        .then((nextCatalogs) => {
          const chosen =
            nextCatalogs.find(
              (catalog) => catalog.id === selectedIdRef.current,
            ) ?? nextCatalogs[0];
          setCatalogs(nextCatalogs);
          if (!chosen) return;
          setSelectedId(chosen.id);
          setLines(chosen.lines);
        })
        .catch(() => {});
    }
  }

  const selectedLabel =
    catalogs.find((catalog) => catalog.id === selectedId)?.label ?? "Projects";

  return (
    <SettingsSeedList
      id="default-patterns"
      hint="Applies to folders added from now on. Folders already added keep their list."
      lines={lines}
      disabled={locked}
      addPlaceholder="Add a pattern"
      onCommit={(next) => {
        void commit(next);
      }}
      catalog={
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={locked}
                className="shrink-0 text-xs"
              />
            }
          >
            {selectedLabel}
            <ChevronDown aria-hidden />
          </DropdownMenuTrigger>
          <DropdownMenuContent>
            <DropdownMenuRadioGroup
              value={selectedId}
              onValueChange={selectCatalog}
            >
              {catalogs.map((catalog) => (
                <DropdownMenuRadioItem
                  key={catalog.id}
                  value={catalog.id}
                  closeOnClick
                >
                  {catalog.label}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      }
    />
  );
}

/** Global seed never-list for new projects and agent folders. */
export function SettingsNeverList() {
  const { locked } = useRoots();
  const [lines, setLines] = useState<string[]>([]);

  useEffect(() => {
    void defaultIgnore()
      .then((text) => setLines(ignoreToLines(text)))
      .catch(() => {
        // getter failed; leave the list empty
      });
  }, []);

  async function commit(next: string[]) {
    if (locked) return;
    try {
      await setDefaultIgnore(`${next.join("\n")}\n`);
      setLines(next);
    } catch {
      void defaultIgnore()
        .then((text) => setLines(ignoreToLines(text)))
        .catch(() => {});
    }
  }

  return (
    <SettingsSeedList
      id="default-ignore"
      hint="Applies to projects and agent folders added from now on."
      lines={lines}
      disabled={locked}
      addPlaceholder="Add an entry"
      onCommit={(next) => {
        void commit(next);
      }}
    />
  );
}

function SettingsGeneral({
  onChangeFolder,
}: {
  onChangeFolder: () => void;
}) {
  const { providerDir, locked } = useRoots();
  const taskLabel = useTaskLabel();
  const wiping = taskLabel === WIPE_LABEL;
  const [loginOn, setLoginOn] = useState(false);
  const [wipeOpen, setWipeOpen] = useState(false);
  const [theme, setTheme] = useTheme();

  useEffect(() => {
    void loginItemEnabled()
      .then(setLoginOn)
      .catch(() => {
        setLoginOn(false);
      });
  }, []);

  async function toggleLogin(on: boolean) {
    if (locked) return;
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
              {providerDir !== null && (
                <Tooltip>
                  <TooltipTrigger
                    render={
                      <Button
                        variant="ghost"
                        size="icon-xs"
                        className="text-destructive hover:text-destructive"
                        disabled={locked}
                        aria-label="Wipe cloud data"
                        onClick={() => setWipeOpen(true)}
                      />
                    }
                  >
                    {wiping ? (
                      <Loader2 className="size-3.5 animate-spin" aria-hidden />
                    ) : (
                      <Trash2 className="size-3.5" aria-hidden />
                    )}
                  </TooltipTrigger>
                  <TooltipContent>Wipe cloud data</TooltipContent>
                </Tooltip>
              )}
              <RevealInFinderButton path={providerDir} />
              <Tooltip>
                <TooltipTrigger
                  render={
                    <Button
                      variant="ghost"
                      size="icon-xs"
                      className="text-muted-foreground"
                      disabled={locked}
                      aria-label="Change cloud folder"
                      onClick={onChangeFolder}
                    />
                  }
                >
                  <Pencil className="size-3.5" aria-hidden />
                </TooltipTrigger>
                <TooltipContent>Change cloud folder</TooltipContent>
              </Tooltip>
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
        <WipeCloudDataAlert open={wipeOpen} onOpenChange={setWipeOpen} />
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
              onClick={() => setTheme("light")}
            >
              Light
            </Button>
            <Button
              type="button"
              variant={theme === "dark" ? "secondary" : "ghost"}
              size="xs"
              aria-pressed={theme === "dark"}
              onClick={() => setTheme("dark")}
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
            disabled={locked}
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
            Cloud folder, appearance, and what new folders track.
          </DialogDescription>
        </div>
        <div
          role="tablist"
          aria-label="Settings sections"
          className="grid grid-cols-3 rounded-4xl border border-border p-0.5"
        >
          {SETTINGS_TABS.map(({ id, label }) => (
            <Button
              key={id}
              type="button"
              role="tab"
              id={`settings-tab-${id}`}
              aria-controls={`settings-panel-${id}`}
              aria-selected={tab === id}
              variant={tab === id ? "default" : "ghost"}
              size="xs"
              className="w-full"
              onClick={() => setTab(id)}
            >
              {label}
            </Button>
          ))}
        </div>
      </DialogHeader>
      <div
        className={
          tab === "general"
            ? "h-[28rem] overflow-y-auto px-6 pt-5 pb-7"
            : "flex h-[28rem] min-h-0 flex-col overflow-hidden px-6 pt-5 pb-7"
        }
      >
        {tab === "general" ? (
          <div
            role="tabpanel"
            id="settings-panel-general"
            aria-labelledby="settings-tab-general"
          >
            <SettingsGeneral onChangeFolder={onChangeFolder ?? (() => {})} />
          </div>
        ) : tab === "patterns" ? (
          <div
            role="tabpanel"
            id="settings-panel-patterns"
            aria-labelledby="settings-tab-patterns"
            className="flex min-h-0 flex-1 flex-col"
          >
            <SettingsPatterns />
          </div>
        ) : (
          <div
            role="tabpanel"
            id="settings-panel-never"
            aria-labelledby="settings-tab-never"
            className="flex min-h-0 flex-1 flex-col"
          >
            <SettingsNeverList />
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
        size="icon-sm"
        className="text-muted-foreground"
        aria-label="Settings"
        onClick={() => setOpen(true)}
      >
        <Settings aria-hidden />
      </Button>
      <SettingsDialog open={open} onOpenChange={setOpen} />
    </>
  );
}
