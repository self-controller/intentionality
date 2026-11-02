import { useState } from "react";
import { Search, X } from "lucide-react";
import { IconButton, Kbd } from "../src/ui/primitives";

/** The backlog's search box, in the add box's pill shape. Ctrl+F reaches it
 *  from anywhere (Welcome.tsx); the screen owns the query and the keys. */
export default function SearchBox({
  value,
  onChange,
  onKeyDown,
}: {
  value: string;
  onChange: (next: string) => void;
  onKeyDown: (e: React.KeyboardEvent<HTMLInputElement>) => void;
}) {
  const [focused, setFocused] = useState(false);
  return (
    <div
      className="flex w-[15rem] max-w-full items-center gap-[0.45rem] rounded-full border border-line bg-surface/70
                 py-[0.15rem] pl-[0.75rem] pr-[0.25rem] transition-colors duration-150 focus-within:border-white/25
                 focus-within:bg-surface"
    >
      <Search className="size-[0.9rem] flex-none text-muted" aria-hidden="true" />
      <input
        id="gate-search"
        type="text"
        value={value}
        placeholder="Search tasks"
        aria-label="Search tasks"
        autoComplete="off"
        spellCheck={false}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        className="h-[1.75rem] min-w-0 flex-1 bg-transparent text-[0.84rem] text-text outline-none placeholder:text-muted/60"
      />
      {value ? (
        <IconButton
          size="sm"
          label="Clear the search"
          className="size-[1.5rem] [&_svg]:size-[0.85rem]"
          onClick={() => {
            onChange("");
            document.getElementById("gate-search")?.focus();
          }}
        >
          <X />
        </IconButton>
      ) : (
        !focused && (
          <span className="flex flex-none items-center gap-[0.2rem] pr-[0.35rem] text-muted/60" aria-hidden="true">
            <Kbd>Ctrl</Kbd>
            <Kbd>F</Kbd>
          </span>
        )
      )}
    </div>
  );
}
