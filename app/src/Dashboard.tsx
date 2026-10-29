import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ChevronRight, Loader, Sparkles } from "lucide-react";
import { BarRow, Bars } from "@/components/Bars";
import { Pie, type PieSlice } from "@/components/Pie";
import RiskPanel from "@/components/RiskPanel";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import * as api from "./api";
import { fmt } from "./format";
import Sessions from "./Sessions";
import type { TodayTime } from "./types";

/** A full day of ActivityWatch events per look, read against the cache. */
const POLL_MS = 60_000;

// Radix remounts a tab each time it is chosen; the fold should stay as it
// was left (the pattern Board uses for its label filter).
let sessionsOpen = false;

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
 * Today, since local midnight: time active and away, how much of the active
 * time went to pages some task needed (the at-risk timer's own rule, from
 * Jev's cached verdicts), the at-risk apps, and -- folded away -- the
 * session history this tab used to open on.
 */
export default function Dashboard({ build }: { build: React.ReactNode }) {
  const [today, setToday] = useState<TodayTime | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [judging, setJudging] = useState(false);
  // What the last Judge press could not do, kept past the next poll (whose
  // answer, from the cache alone, carries no error).
  const [judgeNote, setJudgeNote] = useState<string | null>(null);
  const [open, setOpen] = useState(sessionsOpen);
  // Only the newest request's answer is shown; a Judge answer outranks any
  // poll that was in flight while it ran.
  const latest = useRef(0);

  const load = useCallback(() => {
    const ask = ++latest.current;
    api
      .todayTime()
      .then((t) => {
        if (ask !== latest.current) return;
        setToday(t);
        setError(null);
      })
      .catch((e) => ask === latest.current && setError(String(e)));
  }, []);

  useEffect(() => {
    load();
    const id = window.setInterval(load, POLL_MS);
    // A session opening or closing changes which tasks a page answers to.
    const unlistenOpened = listen("session:opened", load);
    const unlistenClosed = listen("session:closed", load);
    return () => {
      window.clearInterval(id);
      unlistenOpened.then((f) => f());
      unlistenClosed.then((f) => f());
    };
  }, [load]);

  const judge = () => {
    setJudging(true);
    setJudgeNote(null);
    api
      .judgeToday()
      .then((t) => {
        latest.current++;
        setToday(t);
        setError(null);
        if (t.error) setJudgeNote(`Some pages could not be judged — ${t.error}`);
      })
      .catch((e) => setJudgeNote(String(e)))
      .finally(() => setJudging(false));
  };

  const toggle = () => {
    sessionsOpen = !open;
    setOpen(sessionsOpen);
  };

  const date = new Date().toLocaleDateString([], { weekday: "long", month: "long", day: "numeric" });

  return (
    <div className="mx-auto flex w-full max-w-[1080px] flex-col gap-6 px-6 py-6">
      <p className="eyebrow">Today · {date}</p>

      <div className="grid grid-cols-2 gap-4">
        <Kpi
          label="Time active"
          value={today ? fmt(today.active_seconds) : "—"}
          note="Since midnight, at the keyboard"
        />
        <Kpi
          label="Time away"
          value={today ? fmt(today.afk_seconds) : "—"}
          note="Since midnight, idle or locked"
        />
      </div>

      <div className="grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_340px]">
        <section className="surface px-5 py-4" aria-label="Relevant to your tasks">
          <div className="mb-4 flex items-baseline justify-between gap-4">
            <h3>Relevant to your tasks</h3>
            {today && (
              <p className="font-mono text-xs text-muted-foreground">
                relevant at {Math.round(today.threshold * 100)}%
              </p>
            )}
          </div>
          {error ? (
            <p className="text-muted-foreground">unavailable — {error}</p>
          ) : !today ? (
            <p className="text-muted-foreground">…</p>
          ) : (
            <Relevance today={today} judging={judging} judgeNote={judgeNote} onJudge={judge} />
          )}
        </section>
        <RiskPanel onForget={load} />
      </div>

      <section>
        <button
          type="button"
          aria-expanded={open}
          onClick={toggle}
          className="flex items-center gap-1.5 rounded-md py-1 pe-2 text-muted-foreground transition-colors hover:text-foreground"
        >
          <ChevronRight className={cn("size-4 flex-none transition-transform", open && "rotate-90")} />
          <h3 className="text-inherit">Recent sessions</h3>
        </button>
        {open && (
          <div className="mt-4">
            <Sessions />
          </div>
        )}
      </section>

      <p className="font-mono text-[11px] text-muted-foreground">{build}</p>
    </div>
  );
}

function Relevance({
  today,
  judging,
  judgeNote,
  onJudge,
}: {
  today: TodayTime;
  judging: boolean;
  judgeNote: string | null;
  onJudge: () => void;
}) {
  const outside = today.outside_seconds >= 60 ? ` Includes ${fmt(today.outside_seconds)} outside any session.` : "";
  const slices: PieSlice[] = [
    { key: "aligned", label: "Relevant to a task", seconds: today.aligned_seconds, tone: "primary" },
    { key: "off", label: "Not relevant", seconds: today.off_seconds, tone: "warn" },
    {
      key: "unjudged",
      label: "Not judged",
      seconds: today.unjudged_seconds,
      tone: "muted",
      hint: `Pages Jev has no verdict on for every task, this app's own window, and windows with no title.${outside}`,
    },
  ];
  const active = today.active_seconds;
  const relevant = active > 0 ? Math.round((today.aligned_seconds / active) * 100) : 0;
  const tasks = today.tasks.filter((t) => t.seconds > 0);
  const max = Math.max(1, ...tasks.map((t) => t.seconds), today.off_seconds, today.unjudged_seconds);
  const pages = today.askable === 1 ? "page" : "pages";

  return (
    <>
      <Pie slices={slices} headline={`${relevant}%`} caption="relevant" empty="Nothing tracked yet today" />

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
            {tasks.map((t) => (
              <BarRow key={t.title} label={t.title} value={fmt(t.seconds)} pct={(t.seconds / max) * 100} />
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
                hint={slices[2].hint}
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
