import { X } from "lucide-react";
import { useState, type ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { compareAlpha } from "@/lib/order";

export function SettingsSeedList({
  id,
  label,
  hint,
  lines,
  disabled,
  catalog,
  onCommit,
  addPlaceholder,
}: {
  id: string;
  label?: string;
  hint: string;
  lines: string[];
  disabled: boolean;
  catalog?: ReactNode;
  onCommit: (lines: string[]) => void;
  addPlaceholder: string;
}) {
  const [query, setQuery] = useState("");
  const [draft, setDraft] = useState("");
  const visible = lines
    .filter((line) => line.toLowerCase().includes(query.toLowerCase()))
    .sort(compareAlpha);

  function removeLine(line: string) {
    if (disabled) return;
    const index = lines.indexOf(line);
    if (index < 0) return;
    onCommit([...lines.slice(0, index), ...lines.slice(index + 1)]);
  }

  function commitAdd() {
    if (disabled) return;
    const trimmed = draft.trim();
    if (trimmed.length === 0) return;
    if (lines.includes(trimmed)) return;
    onCommit([...lines, trimmed]);
    setDraft("");
  }

  const list =
    lines.length === 0 ? (
      <p className="text-xs text-muted-foreground">No entries</p>
    ) : visible.length === 0 ? (
      <p className="text-xs text-muted-foreground">No matches</p>
    ) : (
      <div className="flex flex-col gap-1">
        {visible.map((line) => (
          <div
            key={line}
            className="flex items-center gap-2 rounded-2xl bg-muted/40 px-3 py-1.5 font-mono text-xs"
          >
            <span className="min-w-0 flex-1 truncate">{line}</span>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              className="text-muted-foreground"
              aria-label={`Remove ${line}`}
              disabled={disabled}
              onClick={() => removeLine(line)}
            >
              <X aria-hidden />
            </Button>
          </div>
        ))}
      </div>
    );

  const toolbar = (
    <div
      data-slot="seed-list-toolbar"
      className="flex items-center gap-1.5"
    >
      {catalog}
      <Input
        placeholder="Search"
        value={query}
        disabled={disabled}
        className="h-8 min-w-0 flex-1 px-2.5 text-xs md:text-xs"
        onChange={(event) => setQuery(event.target.value)}
      />
      <Input
        id={id}
        placeholder={addPlaceholder}
        value={draft}
        disabled={disabled}
        spellCheck={false}
        className="h-8 min-w-0 flex-1 px-2.5 font-mono text-xs md:text-xs"
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={(event) => {
          if (event.key !== "Enter") return;
          event.preventDefault();
          commitAdd();
        }}
      />
      <Button
        type="button"
        variant="secondary"
        size="sm"
        className="shrink-0 text-xs"
        disabled={disabled}
        onClick={commitAdd}
      >
        Add
      </Button>
    </div>
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-1.5">
      {label ? (
        <label htmlFor={id} className="text-sm">
          {label}
        </label>
      ) : null}
      <div className="flex min-h-0 flex-1 flex-col gap-3">
        <div className="flex flex-col gap-1.5">
          {toolbar}
          <p className="text-xs text-muted-foreground">{hint}</p>
        </div>
        <div
          data-slot="seed-list-items"
          className="min-h-0 flex-1 overflow-y-auto"
        >
          {list}
        </div>
      </div>
    </div>
  );
}
