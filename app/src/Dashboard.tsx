import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { CalendarDays, ChevronLeft, ChevronRight, Loader, Sparkles } from "lucide-react";
import { BarRow, Bars } from "@/components/Bars";
import { Pie, type PieSlice, type PieTone } from "@/components/Pie";
import Culprits from "@/components/Culprits";
import MonthCalendar from "@/components/MonthCalendar";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import * as api from "./api";
import { fmt, hourLabel, parseDay, shiftDay } from "./format";
import type { DayTime, TaskTime } from "./types";
import { matches } from "./keybinds";
import { keymap } from "./useKeybinds";

/** A full day of ActivityWatch events per look, read against the cache.
 *  Today only: a past day does not change on its own. */
const POLL_MS = 60_000;

/** One colour per task, handed out in board order. Past this many tasks, the
 *  busiest keep one each but the last, and the rest share "Other tasks". */
const TASK_TONES: PieTone[] = ["series1", "series2", "series3", "series4", "series5"];

/**
 * The day's tasks as pie slices, in the order they come (board order: Doing,
 * then finished that day), so a task keeps its colour as the totals change, and
 * the colour of each task's bar.
 */
function byTask(tasks: TaskTime[]): { slices: PieSlice[]; tone: Map<string, PieTone> } {
  const tone = new Map<string, PieTone>();
  const slice = (t: TaskTime, i: number): PieSlice => {
    tone.set(t.title, TASK_TONES[i]);
    return { key: `task:${t.title}`, label: t.title, seconds: t.seconds, tone: TASK_TONES[i], hint: t.title };
  };
  if (tasks.length <= TASK_TONES.length) return { slices: tasks.map(slice), tone };
  const busiest = new Set([...tasks].sort((a, b) => b.seconds - a.seconds).slice(0, TASK_TONES.length - 1));
  const rest = tasks.filter((t) => !busiest.has(t));
  rest.forEach((t) => tone.set(t.title, "primary"));
  const other: PieSlice = {
    key: "task:other",
    label: `Other tasks (${rest.length})`,
    seconds: rest.reduce((sum, t) => sum + t.seconds, 0),
    tone: "primary",
    hint: rest.map((t) => t.title).join("\n"),
  };
  return { slices: [...tasks.filter((t) => busiest.has(t)).map(slice), other], tone };
}

// Radix remounts a tab each time it is chosen; the day shown should stay as
// it was left (the pattern Board uses for its label filter). null = today.
let shownDay: string | null = null;

/** "Saturday, October 31", with the year only when it is not this one. */
function longDate(day: string): string {
  const d = parseDay(day);
  return d.toLocaleDateString([], {
    weekday: "long",
    month: "long",
    day: "numeric",
    year: d.getFullYear() !== new Date().getFullYear() ? "numeric" : undefined,
  });
}

/** "Sat, Oct 31": a past day named inside a sentence. */
function shortDate(day: string): string {
  const d = parseDay(day);
  return d.toLocaleDateString([], {
    weekday: "short",
    month: "short",
    day: "numeric",
    year: d.getFullYear() !== new Date().getFullYear() ? "numeric" : undefined,
  });
}

/** One headline number. Proportional figures: it stands alone, not in a column. */
function Kpi({ label, value, note }: { label: string; value: string; note: string }) {
  return (
    <div className="surface px-5 py-4">
      <p className="text-[13px] text-muted-foreground">{label}</p>
      <p className="mt-1 text-[40px] font-semibold leading-none tracking-[-0.03em] text-foreground">{value}</p>
      <p className="mt-2 text-xs text-muted-foreground">{note}</p>
    </div>
  );
}

/**
 * One day -- today so far, or a past day stepped back to -- from the hour days
 * start at (Settings, 4 am by default) to the next: time active and away, how
 * much of the active time went to pages some task needed (the at-risk timer's
 * own rule, from Jev's cached verdicts), and where the rest went (the
 * culprits, with the ignore list).
 */
export default function Dashboard({ build }: { build: React.ReactNode }) {
  const [day, setDay] = useState<string | null>(shownDay);
  const [data, setData] = useState<DayTime | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [judging, setJudging] = useState(false);
  // What the last Judge press could not do, kept past the next poll (whose
  // answer, from the cache alone, carries no error).
  const [judgeNote, setJudgeNote] = useState<string | null>(null);
  const [picking, setPicking] = useState(false);
  // Only the newest request's answer is shown: stepping through days quickly
  // leaves earlier answers in flight. A Judge answer outranks any poll that
  // was in flight while it ran, but lands only if its day is still shown.
  const latest = useRef(0);

  const load = useCallback((which: string | null) => {
    const ask = ++latest.current;
    api
      .dayTime(which)
      .then((t) => {
        if (ask !== latest.current) return;
        setData(t);
        setError(null);
      })
      .catch((e) => ask === latest.current && setError(String(e)));
  }, []);

  useEffect(() => {
    load(day);
    // A past day is history: only Judge and the ignore list change it.
    if (day !== null) return;
    const id = window.setInterval(() => load(null), POLL_MS);
    // A session opening or closing changes which tasks a page answers to.
    const unlistenOpened = listen("session:opened", () => load(null));
    const unlistenClosed = listen("session:closed", () => load(null));
    return () => {
      window.clearInterval(id);
      unlistenOpened.then((f) => f());
      unlistenClosed.then((f) => f());
    };
  }, [day, load]);

  const today = data?.today ?? null;
  const isToday = day === null || day === today;
  const shown = day ?? today;
  // The figures on screen belong to another day until the new answer lands;
  // dimmed meanwhile, and gone if that answer is an error.
  const stale = data !== null && shown !== null && data.day !== shown;
  const view = stale && error ? null : data;

  const go = useCallback(
    (to: string | null) => {
      const next = to === null || to === today ? null : to;
      shownDay = next;
      setDay(next);
      setJudgeNote(null);
    },
    [today],
  );
  const step = useCallback(
    (by: number) => {
      if (shown === null || today === null) return;
      const to = shiftDay(shown, by);
      if (to <= today) go(to);
    },
    [shown, today, go],
  );

  // Previous / next day (left and right unless Settings says otherwise) step
  // through days, unless something else has the keys: a text field, or a
  // dialog or popover (the calendar, the ignore menu, the checkpoint).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const keys = keymap();
      const dir = matches(e, keys["app.prevDay"]) ? -1 : matches(e, keys["app.nextDay"]) ? 1 : 0;
      if (!dir || e.defaultPrevented || e.repeat) return;
      const target = e.target as HTMLElement | null;
      if (target?.closest('input, textarea, select, [contenteditable=true], [role="tablist"]')) return;
      if (document.querySelector('[role="dialog"]')) return;
      e.preventDefault();
      step(dir);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [step]);

  const judge = () => {
    const which = day;
    setJudging(true);
    setJudgeNote(null);
    api
      .judgeDay(which)
      .then((t) => {
        // shownDay moves with every step, even across a tab remount.
        if (shownDay !== which) return;
        latest.current++;
        setData(t);
        setError(null);
        if (t.error) setJudgeNote(`Some pages could not be judged — ${t.error}`);
      })
      .catch((e) => shownDay === which && setJudgeNote(String(e)))
      .finally(() => setJudging(false));
  };

  const hour = data ? hourLabel(data.start_hour) : null;
  const pastName = !isToday && shown ? shortDate(shown) : undefined;
  const title = isToday ? `Today${today ? ` · ${longDate(today)}` : ""}` : longDate(shown!);
  const since = (what: string) => (isToday ? `Since ${hour ?? "the day began"}, ${what}` : `That day, ${what}`);

  return (
    <div className="mx-auto flex w-full max-w-[1080px] flex-col gap-6 px-6 py-6">
      <div className="-my-1 flex items-center gap-1">
        <Button
          variant="ghost"
          mode="icon"
          size="sm"
          aria-label="Previous day"
          title="Previous day (←)"
          disabled={shown === null}
          onClick={() => step(-1)}
        >
          <ChevronLeft />
        </Button>
        <Popover open={picking} onOpenChange={setPicking}>
          <PopoverTrigger asChild>
            <button
              type="button"
              aria-label={`${title} — pick a day`}
              className="eyebrow inline-flex h-8 items-center gap-1.5 rounded-md px-2 transition-colors hover:bg-secondary hover:text-foreground data-[state=open]:bg-secondary data-[state=open]:text-foreground"
            >
              <CalendarDays className="size-3.5" />
              {title}
            </button>
          </PopoverTrigger>
          <PopoverContent align="start" className="w-auto p-1" aria-label="Pick a day">
            {shown && today && (
              <MonthCalendar
                value={shown}
                today={today}
                max={today}
                className="mt-0 border-0 bg-transparent"
                onPick={(d) => {
                  setPicking(false);
                  go(d);
                }}
              />
            )}
          </PopoverContent>
        </Popover>
        <Button
          variant="ghost"
          mode="icon"
          size="sm"
          aria-label="Next day"
          title="Next day (→)"
          disabled={isToday}
          onClick={() => step(1)}
        >
          <ChevronRight />
        </Button>
        {!isToday && (
          <Button variant="outline" size="sm" className="ms-2" onClick={() => go(null)}>
            Today
          </Button>
        )}
        {hour && (
          <p className="ms-auto text-xs text-muted-foreground" title="Change it in Settings">
            Days start at {hour}
          </p>
        )}
      </div>

      <div className={cn("flex flex-col gap-6 transition-opacity", stale && "opacity-50")} aria-busy={stale}>
        <div className="grid grid-cols-2 gap-4">
          <Kpi
            label="Time active"
            value={view ? fmt(view.active_seconds) : "—"}
            note={since("at the keyboard")}
          />
          <Kpi
            label="Time away"
            value={view ? fmt(view.afk_seconds) : "—"}
            note={since("idle or locked")}
          />
        </div>

        <div className="grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_340px]">
          <section className="surface px-5 py-4" aria-label="Relevant to your tasks">
            <div className="mb-4 flex items-baseline justify-between gap-4">
              <h3>Relevant to your tasks</h3>
              {view && (
                <p className="font-mono text-xs text-muted-foreground">
                  relevant at {Math.round(view.threshold * 100)}%
                </p>
              )}
            </div>
            {error ? (
              <p className="text-muted-foreground">unavailable — {error}</p>
            ) : !view ? (
              <p className="text-muted-foreground">…</p>
            ) : (
              <Relevance data={view} past={!isToday} judging={judging} judgeNote={judgeNote} onJudge={judge} />
            )}
          </section>
          <Culprits data={view} day={pastName} onChange={() => load(day)} />
        </div>
      </div>

      <p className="font-mono text-[11px] text-muted-foreground">{build}</p>
    </div>
  );
}

function Relevance({
  data: today,
  past,
  judging,
  judgeNote,
  onJudge,
}: {
  data: DayTime;
  past: boolean;
  judging: boolean;
  judgeNote: string | null;
  onJudge: () => void;
}) {
  const outside = today.outside_seconds >= 60 ? ` Includes ${fmt(today.outside_seconds)} outside any session.` : "";
  const doing = past ? "that was in Doing in its session" : "in Doing";
  const unjudgedHint = `Pages Jev has no verdict on for every task ${doing}, this app's own window, and windows with no title.${outside}`;
  // Relevant time is the pie's task slices: each page goes to the likeliest
  // task that clears the bar.
  const tasks = today.tasks.filter((t) => t.seconds > 0);
  const { slices: taskSlices, tone } = byTask(tasks);
  const slices: PieSlice[] = [
    ...taskSlices,
    { key: "off", label: "Not relevant", seconds: today.off_seconds, tone: "warn" },
    { key: "unjudged", label: "Not judged", seconds: today.unjudged_seconds, tone: "muted", hint: unjudgedHint },
  ];
  const active = today.active_seconds;
  const relevant = active > 0 ? Math.round((today.aligned_seconds / active) * 100) : 0;
  const busiest = [...tasks].sort((a, b) => b.seconds - a.seconds);
  const max = Math.max(1, ...tasks.map((t) => t.seconds), today.off_seconds, today.unjudged_seconds);
  const pages = today.askable === 1 ? "page" : "pages";

  return (
    <>
      <Pie
        slices={slices}
        headline={`${relevant}%`}
        caption="relevant"
        empty={past ? "Nothing tracked that day" : "Nothing tracked yet today"}
      />

      {(today.askable > 0 || judging || judgeNote) && (
        <div className="mt-5 flex flex-wrap items-center gap-x-3 gap-y-1.5">
          {(today.askable > 0 || judging) && (
            <Button variant="outline" size="sm" onClick={onJudge} disabled={judging}>
              {judging ? <Loader className="animate-spin" /> : <Sparkles />}
              {judging ? "Judging…" : `Judge ${today.askable} ${pages} with Jev`}
            </Button>
          )}
          {!judging && today.askable > 0 && (
            <span className="text-xs text-muted-foreground">
              The busiest unjudged pages · sends their titles to TypeSafe · billed
            </span>
          )}
          {judgeNote && <span className="w-full text-xs text-warn">{judgeNote}</span>}
        </div>
      )}

      {tasks.length > 0 && (
        <div className="mt-6 border-t border-border pt-4">
          <h3 className="mb-3">By task</h3>
          <Bars>
            {busiest.map((t) => (
              <BarRow
                key={t.title}
                label={t.title}
                value={fmt(t.seconds)}
                pct={(t.seconds / max) * 100}
                tone={tone.get(t.title)}
              />
            ))}
            {today.off_seconds > 0 && (
              <BarRow
                label="Not relevant"
                value={fmt(today.off_seconds)}
                pct={(today.off_seconds / max) * 100}
                tone="warn"
              />
            )}
            {today.unjudged_seconds > 0 && (
              <BarRow
                label="Not judged"
                hint={unjudgedHint}
                value={fmt(today.unjudged_seconds)}
                pct={(today.unjudged_seconds / max) * 100}
                tone="muted"
              />
            )}
          </Bars>
        </div>
      )}
    </>
  );
}
