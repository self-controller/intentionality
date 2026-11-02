import { useEffect, useMemo, useRef, useState } from "react";
import { AppWindow, ChevronDown, EyeOff, Globe, Plus, X } from "lucide-react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import * as api from "../api";
import { fmt } from "../format";
import type { IgnoreChoice, IgnoreRule, InstalledApp } from "../types";

/** Window classes of apps whose windows are many things: ignoring one
 *  ignores all of them, which the row says. Words of the class, as
 *  screen_time.rs matches browsers. */
const BROWSERS = ["zen", "firefox", "librewolf", "floorp", "waterfox", "chromium", "chrome", "brave", "vivaldi", "epiphany", "opera", "msedge"];
const TERMINALS = ["ptyxis", "terminal", "kgx", "console", "kitty", "alacritty", "foot", "wezterm", "konsole", "ghostty"];

function covers(classes: string[]): string | undefined {
  const words = classes.flatMap((c) => c.split(/[^a-z0-9]+/));
  if (words.some((w) => BROWSERS.includes(w))) return "every website in it";
  if (words.some((w) => TERMINALS.includes(w))) return "every terminal";
  return undefined;
}

/** The site a typed text names, as ignore.rs keeps it: "https://www.
 *  linkedin.com/feed" -> "linkedin.com". Rust checks it again. */
function typedHost(text: string): string | null {
  let rest = text.trim().toLowerCase().replace(/^https?:\/\//, "");
  rest = rest.split(/[/?#]/)[0].replace(/^www\./, "");
  return /^[a-z0-9-]+(\.[a-z0-9-]+)*\.[a-z]{2,}$/.test(rest) ? rest : null;
}

/** One thing the menu can ignore. */
interface Pick {
  key: string;
  rule: IgnoreRule;
  label: string;
  /** Right-hand note: the day's time. */
  note?: string;
  /** Under the name: what ignoring an app covers ("every website in it"). */
  covers?: string;
  section: "typed" | "seen" | "installed";
}

/**
 * The culprits box's ignore list, as one small line: "Ignored · 2 ▾". It
 * opens on what is ignored (each removable) and a search over what can be:
 * apps and sites seen on the day shown (`day`, a past day's name; none is
 * today), busiest first, every installed app, or a site typed or pasted. An
 * ignored app or site is never judged: Jev is not asked, the timer never
 * shows on it, and its time is "Not judged". `onChange` runs after every
 * change, so the Dashboard reads the day again.
 */
export default function IgnoreMenu({
  seen,
  day,
  onChange,
}: {
  seen: IgnoreChoice[];
  day?: string;
  onChange: () => void;
}) {
  const when = day ? `on ${day}` : "today";
  const [rules, setRules] = useState<IgnoreRule[]>([]);
  const [installed, setInstalled] = useState<InstalledApp[]>([]);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const list = useRef<HTMLUListElement>(null);

  useEffect(() => {
    api.ignoreList().then(setRules).catch((e) => setProblem(String(e)));
  }, []);

  // Installed apps are read when the menu opens: one may have been
  // installed since.
  useEffect(() => {
    if (!open) return;
    setQuery("");
    setActive(0);
    setProblem(null);
    api.installedApps().then(setInstalled).catch(() => setInstalled([]));
  }, [open]);

  const picks = useMemo(() => {
    const q = query.trim().toLowerCase();
    const has = (r: { kind: string; value: string }) => rules.some((x) => x.kind === r.kind && x.value === r.value);
    const hit = (...texts: string[]) => !q || texts.some((t) => t.toLowerCase().includes(q));
    const out: Pick[] = [];
    const host = typedHost(query);
    if (host && !has({ kind: "site", value: host })) {
      out.push({
        key: `typed:${host}`,
        rule: { kind: "site", value: host, name: host, classes: [] },
        label: `Ignore site ${host}`,
        section: "typed",
      });
    }
    for (const c of seen) {
      if (c.ignored || has(c) || !hit(c.name, c.value) || (c.kind === "site" && c.value === host)) continue;
      const rule = { kind: c.kind, value: c.value, name: c.name, classes: c.classes };
      out.push({
        key: `${c.kind}:${c.value}`,
        rule,
        label: c.name,
        note: fmt(c.seconds),
        covers: c.kind === "app" ? covers(c.classes) : undefined,
        section: "seen",
      });
    }
    const listed = new Set(out.map((p) => p.key));
    for (const a of installed) {
      const value = a.id.toLowerCase();
      const rule: IgnoreRule = { kind: "app", value, name: a.name, classes: a.classes };
      if (listed.has(`app:${value}`) || has(rule) || !hit(a.name, a.id)) continue;
      out.push({ key: `app:${value}`, rule, label: a.name, covers: covers(a.classes), section: "installed" });
    }
    return out;
  }, [query, rules, seen, installed]);

  // The highlighted row stays in view as the arrows move it.
  useEffect(() => {
    list.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const change = (write: Promise<IgnoreRule[]>) => {
    setBusy(true);
    setProblem(null);
    write
      .then((next) => {
        setRules(next);
        setQuery("");
        setActive(0);
        onChange();
      })
      .catch((e) => setProblem(String(e)))
      .finally(() => setBusy(false));
  };

  const onKey = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      setActive((i) => (picks.length === 0 ? 0 : (i + step + picks.length) % picks.length));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const pick = picks[Math.min(active, picks.length - 1)];
      if (pick && !busy) change(api.addIgnore(pick.rule));
    }
  };

  const heading = (section: Pick["section"]) =>
    section === "seen" ? `Seen ${when}` : section === "installed" ? "Installed apps" : null;

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="-me-1.5 inline-flex h-6 flex-none items-center gap-1 rounded-md px-1.5 text-xs text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground data-[state=open]:bg-secondary data-[state=open]:text-foreground"
          title="Apps and sites that are never judged"
        >
          <EyeOff className="size-3.5" />
          {rules.length > 0 ? `Ignored · ${rules.length}` : "Ignore"}
          <ChevronDown className="size-3 opacity-70" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        aria-label="Ignored apps and sites"
        onOpenAutoFocus={(e) => {
          // Into the search box, not the first button.
          e.preventDefault();
          (e.currentTarget as HTMLElement).querySelector("input")?.focus();
        }}
      >
        <div className="border-b border-border p-2">
          <input
            value={query}
            disabled={busy}
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0);
            }}
            onKeyDown={onKey}
            placeholder="Type a site or app…"
            aria-label="Site or app to ignore"
            className="h-8 w-full rounded-md border border-input bg-input/30 px-2.5 text-[13px] text-foreground outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50"
          />
        </div>

        {rules.length > 0 && !query.trim() && (
          <div className="border-b border-border px-1 py-1.5">
            <p className="px-2 pb-1 text-[11px] font-medium text-muted-foreground">Ignored</p>
            <ul className="list-none p-0">
              {rules.map((r) => (
                <li key={`${r.kind}:${r.value}`} className="group flex items-center gap-2 rounded px-2 py-1 text-[13px] hover:bg-secondary">
                  {r.kind === "site" ? <Globe className="size-3.5 flex-none text-muted-foreground" /> : <AppWindow className="size-3.5 flex-none text-muted-foreground" />}
                  <span className="min-w-0 flex-1 truncate text-foreground" title={r.kind === "app" ? r.classes.join(", ") : r.value}>
                    {r.name}
                  </span>
                  <button
                    type="button"
                    aria-label={`Judge ${r.name} again`}
                    title="Judge it again"
                    disabled={busy}
                    onClick={() => change(api.removeIgnore(r.kind, r.value))}
                    className="flex size-5 flex-none items-center justify-center rounded text-muted-foreground hover:text-foreground"
                  >
                    <X className="size-3.5" />
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}

        <ul ref={list} className="max-h-64 list-none overflow-y-auto p-1" role="listbox" aria-label="Apps and sites to ignore">
          {picks.length === 0 && (
            <li className="px-2 py-2 text-[13px] text-muted-foreground">
              {query.trim() ? "No app or site matches — type a site like linkedin.com" : `Nothing else seen ${when}`}
            </li>
          )}
          {picks.map((p, i) => {
            const title = i === 0 || picks[i - 1].section !== p.section ? heading(p.section) : null;
            return (
              <li key={p.key} role="presentation">
                {title && <p className="px-2 pb-1 pt-2 text-[11px] font-medium text-muted-foreground">{title}</p>}
                <button
                  type="button"
                  role="option"
                  aria-selected={i === active}
                  data-index={i}
                  disabled={busy}
                  onMouseEnter={() => setActive(i)}
                  onClick={() => change(api.addIgnore(p.rule))}
                  className={cn(
                    "flex w-full items-center gap-2 rounded px-2 py-1 text-left text-[13px]",
                    i === active && "bg-secondary",
                  )}
                >
                  {p.section === "typed" ? (
                    <Plus className="size-3.5 flex-none text-muted-foreground" />
                  ) : p.rule.kind === "site" ? (
                    <Globe className="size-3.5 flex-none text-muted-foreground" />
                  ) : (
                    <AppWindow className="size-3.5 flex-none text-muted-foreground" />
                  )}
                  <span className="flex min-w-0 flex-1 flex-col">
                    <span className="truncate text-foreground">{p.label}</span>
                    {p.covers && <span className="truncate text-[11px] text-muted-foreground">{p.covers}</span>}
                  </span>
                  {p.note && <span className="flex-none font-mono text-[11px] tabular-nums text-muted-foreground">{p.note}</span>}
                </button>
              </li>
            );
          })}
        </ul>

        <p className="border-t border-border px-3 py-2 text-[11px] leading-snug text-muted-foreground">
          {problem ? (
            <span className="text-warn">{problem}</span>
          ) : (
            "Never judged: no timer on it, and its time counts as Not judged. A site covers its subdomains."
          )}
        </p>
      </PopoverContent>
    </Popover>
  );
}
