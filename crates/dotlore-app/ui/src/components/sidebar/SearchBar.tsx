import type { KeyboardEvent } from "react";
import { Search } from "lucide-react";

import { Input } from "@/components/ui/input";

type SearchBarProps = {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  label?: string;
  onKeyDown?: (event: KeyboardEvent<HTMLInputElement>) => void;
};

export function SearchBar({
  value,
  onChange,
  placeholder = "Search",
  label = "Search agents or projects",
  onKeyDown,
}: SearchBarProps) {
  return (
    <div className="min-w-0 flex-1">
      <div className="relative">
        <Search
          aria-hidden
          className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          type="search"
          value={value}
          onChange={(event) => onChange(event.target.value)}
          onKeyDown={onKeyDown}
          placeholder={placeholder}
          autoComplete="off"
          spellCheck={false}
          aria-label={label}
          className="h-8 pl-8"
        />
      </div>
    </div>
  );
}
