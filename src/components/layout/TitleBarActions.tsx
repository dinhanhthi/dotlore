import { Moon, Sun, TriangleAlert } from "lucide-react";

import { SettingsPopover } from "@/components/settings/SettingsPopover";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { conflictTotal, useRoots } from "@/lib/roots";
import { useTheme } from "@/lib/theme";

function ThemeToggle() {
  const [theme, setTheme] = useTheme();
  const next = theme === "dark" ? "light" : "dark";
  const label =
    next === "light" ? "Switch to light theme" : "Switch to dark theme";

  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className="text-muted-foreground"
            aria-label={label}
            onClick={() => setTheme(next)}
          />
        }
      >
        {theme === "dark" ? <Moon aria-hidden /> : <Sun aria-hidden />}
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

function ConflictAlert() {
  const { roots, showConflicts } = useRoots();
  const conflicts = conflictTotal(roots);
  if (conflicts === 0) return null;
  const label = conflicts === 1 ? "1 conflict" : `${conflicts} conflicts`;

  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className="text-status-conflict"
            aria-label={`${label} — show them`}
            onClick={showConflicts}
          />
        }
      >
        <TriangleAlert aria-hidden />
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

export function TitleBarActions() {
  return (
    <div className="flex shrink-0 items-center gap-0.5 pr-2">
      <ConflictAlert />
      <ThemeToggle />
      <SettingsPopover />
    </div>
  );
}
