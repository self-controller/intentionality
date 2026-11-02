import { useCallback, useEffect, useState } from "react";
import { ChevronRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import * as api from "./api";
import { hourLabel } from "./format";
import KeyboardShortcuts from "./KeyboardShortcuts";
import { ACTIONS } from "./keybinds";
import { useKeybinds } from "./useKeybinds";

/**
 * Settings, opened from the gear at the top right of the header: where a
 * day starts, the relevance cache -- the page–task verdicts Jev has given --
 * with the way to empty it, and the keyboard shortcuts, on a page of their
 * own.
 */
export default function Settings() {
  const [page, setPage] = useState<"main" | "keys">("main");
  return (
    <div className="mx-auto flex w-full max-w-[720px] flex-col gap-6 px-6 py-6">
      {page === "keys" ? (
        <KeyboardShortcuts onBack={() => setPage("main")} />
      ) : (
        <>
          <p className="eyebrow">Settings</p>
          <DayStart />
          <JevMemory />
          <Shortcuts onOpen={() => setPage("keys")} />
        </>
      )}
    </div>
  );
}

/** The way into the shortcuts page, with how many have been changed. */
function Shortcuts({ onOpen }: { onOpen: () => void }) {
  const keys = useKeybinds();
  const changed = ACTIONS.filter((a) => keys[a.id] !== a.default).length;
  return (
    <section className="surface flex flex-col px-5 py-4" aria-label="Keyboard shortcuts">
      <h3>Keyboard shortcuts</h3>
      <p className="text-xs text-muted-foreground">Every key the gate and this app answer to, and changing them.</p>
      <button
        type="button"
        onClick={onOpen}
        className="group mt-3 flex cursor-pointer items-center gap-3 border-t border-border pt-2.5 text-left text-[13px]"
      >
        <span className="min-w-0 flex-1 text-muted-foreground group-hover:text-foreground">
          {changed === 0 ? "All at their defaults" : `${changed} changed from the defaults`}
        </span>
        <span className="flex items-center gap-1 text-foreground">
          Change shortcuts
          <ChevronRight className="size-4 transition-transform group-hover:translate-x-0.5" />
        </span>
      </button>
    </section>
  );
}

/** Hours a day may start at: midnight through the morning. A day starting
 *  in the afternoon would split most days in two. */
const HOURS = Array.from({ length: 12 }, (_, h) => h);

const sentence = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** The hour a new day starts at (day.rs), 4 am unless changed here. */
function DayStart() {
  const [hour, setHour] = useState<number | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    api
      .dayStartHour()
      .then(setHour)
      .catch((e) => setProblem(String(e)));
  }, []);

  const pick = (value: string) => {
    setProblem(null);
    api
      .setDayStartHour(Number(value))
      .then(setHour)
      .catch((e) => setProblem(String(e)));
  };

  // An hour set by hand outside the menu's range is still shown.
  const hours = hour !== null && !HOURS.includes(hour) ? [...HOURS, hour] : HOURS;

  return (
    <section className="surface flex flex-col px-5 py-4" aria-label="Day">
      <h3>Day</h3>
      <p className="text-xs text-muted-foreground">
        The Dashboard's days and the clock's daily total roll over at this hour, so a late night stays on the day it
        began. Due dates stay calendar days.
      </p>
      <div className="mt-3 flex items-center gap-3 border-t border-border pt-2.5 text-[13px]">
        <span className="min-w-0 flex-1 text-muted-foreground">A new day starts at</span>
        {hour !== null && (
          <Select value={String(hour)} onValueChange={pick}>
            <SelectTrigger className="w-36" aria-label="A new day starts at">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {hours.map((h) => (
                <SelectItem key={h} value={String(h)}>
                  {sentence(hourLabel(h))}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      </div>
      {problem && <p className="pt-1.5 text-xs text-warn">{problem}</p>}
    </section>
  );
}

/** How many verdicts Jev has cached, and Forget, behind a confirm. Nothing is
 *  lost by forgetting but the requests to ask again. */
function JevMemory() {
  // From today's figures, which read the cache only and never ask Jev.
  const [judged, setJudged] = useState<number | null>(null);
  const [forgetting, setForgetting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const load = useCallback(() => {
    api
      .dayTime(null)
      .then((t) => setJudged(t.judged))
      .catch((e) => setProblem(String(e)));
  }, []);

  useEffect(load, [load]);

  const forget = () => {
    setForgetting(false);
    setBusy(true);
    setProblem(null);
    api
      .forgetRelevance()
      .then(load)
      .catch((e) => setProblem(String(e)))
      .finally(() => setBusy(false));
  };

  return (
    <section className="surface flex flex-col px-5 py-4" aria-label="Jev's memory">
      <h3>Jev's memory</h3>
      <p className="text-xs text-muted-foreground">
        Every page Jev judges against a task is remembered, so it is asked only once. Forgetting loses nothing but
        the requests to ask again.
      </p>

      <div className="mt-3 flex items-center gap-2 border-t border-border pt-2.5 text-[13px] text-muted-foreground">
        <span className="min-w-0 flex-1 truncate">
          {judged == null
            ? "…"
            : judged === 0
              ? "No pages judged by Jev yet"
              : `${judged} page–task ${judged === 1 ? "verdict" : "verdicts"} from Jev`}
        </span>
        {judged != null && judged > 0 && !forgetting && (
          <Button variant="outline" size="sm" disabled={busy} onClick={() => setForgetting(true)}>
            Forget
          </Button>
        )}
        {forgetting && (
          <>
            <span className="flex-none">Ask again on every page?</span>
            <Button variant="destructive" size="sm" disabled={busy} onClick={forget}>
              Forget all
            </Button>
            <Button variant="outline" size="sm" onClick={() => setForgetting(false)}>
              Keep
            </Button>
          </>
        )}
      </div>
      {problem && <p className="pt-1.5 text-xs text-warn">{problem}</p>}
    </section>
  );
}
