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

  const addRow = (
    <div className="flex items-center gap-2">
      <Input
        id={id}
        placeholder={addPlaceholder}
        value={draft}
        disabled={disabled}
        spellCheck={false}
        className="font-mono text-xs"
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
        disabled={disabled}
        onClick={commitAdd}
      >
        Add
      </Button>
    </div>
  );

  const search = (
    <Input
      placeholder="Search"
      value={query}
      disabled={disabled}
      className={catalog ? "min-w-0 flex-1" : undefined}
      onChange={(event) => setQuery(event.target.value)}
    />
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-1.5">
      {label ? (
        <label htmlFor={id} className="text-sm">
          {label}
        </label>
      ) : null}
      <p className="text-xs text-muted-foreground">{hint}</p>
      <div className="mt-4 flex min-h-0 flex-1 flex-col gap-3">
        {catalog ? (
          <div className="flex items-center gap-2">
            {catalog}
            {search}
          </div>
        ) : (
          search
        )}
        {addRow}
        <div className="min-h-0 flex-1 overflow-y-auto">{list}</div>
      </div>
    </div>
  );
}
