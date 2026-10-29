import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Loader, ScanEye, Timer } from "lucide-react";
import { BarMore, BarRow, Bars } from "@/components/Bars";
import ListRow from "@/components/ListRow";
import { pct, ProbabilityBar } from "@/components/ProbabilityBar";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import * as api from "./api";
import { activities, ALIGN_TEXT, band, dayKey, dayLabel, fmt, hhmm } from "./format";
import type { Analysis, AnalysisDetail, Judged, JudgedPage, PageRef } from "./types";

// The cut of the deleted src-tauri/src/claude.rs (TOP_APPS and
// TOP_TITLES_PER_APP; see git history). Only rows Claude wrote use it: their
// pane is headed "What the model saw", so the cut has to be the prompt's cut,
// not a taste judgement — anything past it is shown separately and labelled
// as unsent.
const TOP_APPS = 8;
const MODEL_TITLES = 5;
const CUT = { apps: TOP_APPS, titles: MODEL_TITLES };

// Pages listed before "+N more": the busiest carry nearly all of a stretch.
const PAGES_SHOWN = 10;

const TRIGGER: Record<Judged["trigger"], string | null> = {
  scheduled: null,
  manual: "run by hand",
  drift: "early — unexcused at-risk time",
  checkpoint: null,
};

const pageKey = (p: { app: string; title: string }) => p.app + "\n" + p.title;

export default function Analyses({
  hasSession,
  sessionId,
  appealOpen,
  onAppeal,
  onSeen,
}: {
  hasSession: boolean;
  // The open session, if any: only its pages can be appealed, because an
  // appeal is judged against the tasks open now.
  sessionId: number | null;
  appealOpen: boolean;
  onAppeal: (page: PageRef) => void;
  onSeen: () => void;
}) {
  const [items, setItems] = useState<Analysis[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [detail, setDetail] = useState<AnalysisDetail | null>(null);
  const [running, setRunning] = useState<"check" | "checkpoint" | null>(null);
  const [runNote, setRunNote] = useState<string | null>(null);
  const [showUnsent, setShowUnsent] = useState(false);
  const [showAllPages, setShowAllPages] = useState(false);
  // Pages won on appeal from here. The row's snapshot stays as it was; the
  // win counts from the next check, and the row says so.
  const [won, setWon] = useState<Set<string>>(new Set());
  const asked = useRef<JudgedPage | null>(null);

  const selectedRef = useRef<number | null>(null);
  useEffect(() => {
    selectedRef.current = selected;
  }, [selected]);
  // Marking is idempotent server-side, but onSeen is not: this keeps the badge
  // honest under StrictMode's double mount and under a double click.
  const marked = useRef(new Set<number>());

  const select = useCallback(
    (a: Analysis) => {
      setSelected(a.id);
      setDetail(null);
      setShowUnsent(false);
      setShowAllPages(false);
      api.getAnalysisDetail(a.id).then(setDetail).catch(() => {});
      if (a.seen_at || marked.current.has(a.id)) return;
      marked.current.add(a.id);
      api
        .markAnalysisSeen(a.id)
        .then(() => {
          // Any non-null stamp does: the UI only reads seen_at as a boolean.
          const at = new Date().toISOString();
          setItems((prev) => prev.map((x) => (x.id === a.id ? { ...x, seen_at: at } : x)));
          onSeen();
        })
        .catch(() => marked.current.delete(a.id));
    },
    [onSeen],
  );

  const reload = useCallback(() => api.listRecentAnalyses(api.RECENT_ANALYSES), []);

  useEffect(() => {
    reload()
      .then((list) => {
        setItems(list);
        if (list.length > 0 && selectedRef.current == null) select(list[0]);
      })
      .catch(() => {});
    // A check landing while the tab is open must not yank the pane out from
    // under a read, so the selection is left alone once there is one.
    const unlisten = listen("analysis:new", () => {
      reload()
        .then((list) => {
          setItems(list);
          if (list.length > 0 && selectedRef.current == null) select(list[0]);
        })
        .catch(() => {});
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [reload, select]);

  // The appeal closed: ask the cache (no request) whether it was won.
  useEffect(() => {
    const page = asked.current;
    if (appealOpen || !page) return;
    asked.current = null;
    api
      .pageJudgement(page.app, page.title)
      .then((j) => {
        if (j.relevant != null) setWon((prev) => new Set(prev).add(pageKey(page)));
      })
      .catch(() => {});
  }, [appealOpen]);

  const appeal = (page: JudgedPage) => {
    asked.current = page;
    onAppeal({ label: page.name || page.app, app: page.app, title: page.title, from: "check" });
  };

  const runNow = () => {
    setRunning("check");
    setRunNote(null);
    api
      .runAnalysisNow()
      .then(async (id) => {
        if (id == null) {
          setRunNote("Skipped — under 5 minutes of tracked activity since the last check.");
          return;
        }
        const list = await reload();
        setItems(list);
        const fresh = list.find((a) => a.id === id);
        if (fresh) select(fresh);
      })
      .catch((e) => setRunNote(String(e)))
      .finally(() => setRunning(null));
  };

  // The checkpoint without waiting out the clock. It puts its own screen up
  // (App listens for checkpoint:new), so there is nothing to select here.
  const runCheckpoint = () => {
    setRunning("checkpoint");
    setRunNote(null);
    api
      .runCheckpointNow()
      .then(() => reload().then(setItems))
      .catch((e) => setRunNote(String(e)))
      .finally(() => setRunning(null));
  };

  const current = items.find((a) => a.id === selected) ?? null;
  const observed = detail?.observed ?? null;
  const judged = detail?.judged ?? null;
  const { ranked, other } = observed ? activities(observed, CUT) : { ranked: [], other: null };
  const unsent = ranked.filter((a) => !a.sent);
  const rows = [...ranked.filter((a) => a.sent), ...(showUnsent ? unsent : [])];
  // Other counts too: it can outweigh any one row, and a bar past 100% would lie.
  const max = Math.max(1, ranked[0]?.secs ?? 0, other?.secs ?? 0);

  const windowSeconds = current
    ? (new Date(current.window_end).getTime() - new Date(current.window_start).getTime()) / 1000
    : 0;

  // Newest first, so same-day rows are already adjacent.
  const groups: { key: string; label: string; items: Analysis[] }[] = [];
  for (const a of items) {
    const key = dayKey(a.created_at);
    const last = groups[groups.length - 1];
    if (last && last.key === key) last.items.push(a);
    else groups.push({ key, label: dayLabel(a.created_at), items: [a] });
  }

  return (
    <div className="flex items-start gap-8 px-6 py-6">
      <aside className="max-h-[calc(100vh-104px)] w-[280px] flex-none overflow-y-auto">
        {groups.map((g) => (
          <div key={g.key} className="mb-4 flex flex-col gap-0.5">
            <h3 className="mb-1.5 px-3">{g.label}</h3>
            {g.items.map((a) => {
              const b = band(a.alignment);
              return (
                <ListRow
                  key={a.id}
                  selected={a.id === selected}
                  className={cn(
                    "flex items-center gap-2",
                    // Unread checks are the ones worth returning to.
                    !a.seen_at && "font-semibold text-foreground",
                  )}
                  onClick={() => select(a)}
                >
                  <span className="flex-none font-mono text-[11px] font-normal text-muted-foreground">
                    {hhmm(a.created_at)}
                  </span>
                  <span className="min-w-0 flex-1 truncate">
                    {a.kind === "checkpoint" && (
                      <span className="me-1.5 rounded border border-border bg-secondary px-1 font-mono text-[10px] font-normal text-muted-foreground">
                        time&rsquo;s up
                      </span>
                    )}
                    {a.headline}
                  </span>
                  <span
                    className={"flex-none font-mono text-xs font-normal tabular-nums " + ALIGN_TEXT[b.cls]}
                    title={b.label}
                  >
                    {b.glyph} {a.alignment ?? "—"}
                  </span>
                </ListRow>
              );
            })}
          </div>
        ))}
        {items.length === 0 && <p className="px-3 text-muted-foreground">No checks yet.</p>}
      </aside>

      <section className="min-w-0 max-w-[760px] flex-1">
        {current ? (
          <>
            <div className="flex items-start justify-between gap-6">
              <div className="min-w-0">
                <p className="eyebrow">
                  {current.kind === "checkpoint" ? "Checkpoint" : "Check"} · session #{current.session_id}
                  {current.session_statement ? ` · ${current.session_statement}` : ""}
                  {judged && TRIGGER[judged.trigger] ? ` · ${TRIGGER[judged.trigger]}` : ""}
                </p>
                <h2 className="mt-2">{current.headline}</h2>
                <p className="mt-1.5 font-mono text-xs text-muted-foreground">
                  {dayLabel(current.created_at)} {hhmm(current.created_at)} · covers{" "}
                  {hhmm(current.window_start)}–{hhmm(current.window_end)} ({fmt(windowSeconds)})
                </p>
              </div>
              <div className={"flex flex-none flex-col items-end " + ALIGN_TEXT[band(current.alignment).cls]}>
                <span className="font-mono text-4xl font-medium leading-none tracking-[-0.04em] tabular-nums">
                  {band(current.alignment).glyph} {current.alignment ?? "—"}
                </span>
                <span className="eyebrow mt-2 text-current">{band(current.alignment).label}</span>
              </div>
            </div>
            <p className="mt-5 max-w-[60ch] leading-relaxed">{current.body}</p>
            {current.recommendation && (
              <div className="surface mt-6 max-w-[60ch] px-5 py-4">
                <h3>Recommended</h3>
                <p className="mb-1.5 mt-2 text-[15px] font-medium tracking-[-0.01em]">
                  {current.recommendation.advice}
                  {current.recommendation.minutes > 0 && (
                    <span className="font-normal text-muted-foreground"> · about {current.recommendation.minutes} min</span>
                  )}
                </p>
                {current.recommendation_note && (
                  <p className="mb-2 text-[13px] text-muted-foreground">{current.recommendation_note}</p>
                )}
                <p className="mb-2 text-foreground/90">{current.recommendation.why}</p>
                <p className="font-mono text-[11px] text-muted-foreground">
                  {current.recommendation.source ?? "Not yet backed by a cited study."}
                </p>
                {judged?.pick && (
                  <p className="mt-1 font-mono text-[11px] text-muted-foreground">
                    Jev&rsquo;s pick · {pct(judged.pick.probabilities[judged.pick.id] ?? null)}
                  </p>
                )}
              </div>
            )}

            {detail && judged ? (
              <JudgedView
                judged={judged}
                active={detail.observed.active_seconds}
                away={detail.observed.afk_seconds}
                appealable={current.session_id === sessionId}
                won={won}
                showAll={showAllPages}
                onShowAll={() => setShowAllPages((v) => !v)}
                onAppeal={appeal}
              />
            ) : detail ? (
              <div className="surface mt-6 px-5 py-4">
                <div className="mb-4 flex items-baseline justify-between gap-4">
                  <h3>What the model saw</h3>
                  {observed && (
                    <p className="font-mono text-xs text-muted-foreground">
                      {fmt(observed.active_seconds)} active · {fmt(observed.afk_seconds)} away
                    </p>
                  )}
                </div>
                <Bars>
                  {rows.map((a) => (
                    <BarRow
                      key={a.label}
                      label={a.label}
                      hint={a.hint}
                      value={fmt(a.secs)}
                      pct={(a.secs / max) * 100}
                      unsent={!a.sent}
                    />
                  ))}
                  {/* The prompt's remainder lines reach the model, so Other counts
                      as seen however many titles it stands for, and stays last.
                      Only real titles past the cut are ones the model never got. */}
                  {other && (
                    <BarRow
                      label={other.label}
                      hint={other.hint}
                      value={fmt(other.secs)}
                      pct={(other.secs / max) * 100}
                      unsent={!other.sent}
                    />
                  )}
                  {unsent.length > 0 && (
                    <BarMore onClick={() => setShowUnsent((v) => !v)}>
                      {showUnsent ? "▾ hide the rest" : `▸ +${unsent.length} more — recorded, not sent to the model`}
                    </BarMore>
                  )}
                  {observed && ranked.length === 0 && !other && (
                    <p className="text-muted-foreground">nothing recorded</p>
                  )}
                </Bars>
              </div>
            ) : null}
          </>
        ) : (
          <p className="text-muted-foreground">
            No checks yet — they land at random intervals, and quiet windows are skipped.
          </p>
        )}

        <div className="mt-6 flex flex-wrap items-center gap-2.5">
          <Button variant="outline" onClick={runNow} disabled={running != null || !hasSession}>
            {running === "check" ? <Loader className="animate-spin" /> : <ScanEye />}
            {running === "check" ? "Checking…" : "Run a check now"}
          </Button>
          <Button variant="outline" onClick={runCheckpoint} disabled={running != null || !hasSession}>
            {running === "checkpoint" ? <Loader className="animate-spin" /> : <Timer />}
            {running === "checkpoint" ? "Checking…" : "Run a checkpoint now"}
          </Button>
          {!hasSession && <span className="text-muted-foreground">no open session</span>}
          {runNote && <span className="text-muted-foreground">{runNote}</span>}
        </div>
      </section>
    </div>
  );
}

/** A row Jev judged: where the stretch's time went by task, the at-risk line,
 *  and every page with its verdict. The snapshot is the row's own, taken when
 *  it was written, so it always matches the number at the top. */
function JudgedView({
  judged,
  active,
  away,
  appealable,
  won,
  showAll,
  onShowAll,
  onAppeal,
}: {
  judged: Judged;
  active: number;
  away: number;
  appealable: boolean;
  won: Set<string>;
  showAll: boolean;
  onShowAll: () => void;
  onAppeal: (page: JudgedPage) => void;
}) {
  const tasks = judged.tasks.filter((t) => t.seconds > 0).sort((a, b) => b.seconds - a.seconds);
  const max = Math.max(1, ...tasks.map((t) => t.seconds), judged.off_seconds, judged.unjudged_seconds);
  const pages = showAll ? judged.pages : judged.pages.slice(0, PAGES_SHOWN);
  const hidden = judged.pages.length - PAGES_SHOWN;
  const bar = Math.round(judged.threshold * 100);

  return (
    <>
      <div className="surface mt-6 px-5 py-4">
        <div className="mb-4 flex items-baseline justify-between gap-4">
          <h3>Where the time went</h3>
          <p className="font-mono text-xs text-muted-foreground">
            {fmt(active)} active · {fmt(away)} away
          </p>
        </div>
        <Bars>
          {tasks.map((t) => (
            <BarRow key={t.task_id} label={t.title} value={fmt(t.seconds)} pct={(t.seconds / max) * 100} />
          ))}
          {judged.off_seconds > 0 && (
            <BarRow
              label="No task needed it"
              value={fmt(judged.off_seconds)}
              pct={(judged.off_seconds / max) * 100}
              tone="warn"
            />
          )}
          {judged.unjudged_seconds > 0 && (
            <BarRow
              label="Not judged"
              hint="Pages past the first thirty, an app's long tail, this app's own window, or pages Jev could not be asked about"
              value={fmt(judged.unjudged_seconds)}
              pct={(judged.unjudged_seconds / max) * 100}
              tone="muted"
            />
          )}
          {tasks.length === 0 && judged.off_seconds === 0 && judged.unjudged_seconds === 0 && (
            <p className="text-muted-foreground">Nothing was judged for this stretch.</p>
          )}
        </Bars>
        {judged.risk.length > 0 && (
          <p className="mt-4 text-[13px] text-muted-foreground">
            <span className="text-foreground">At-risk apps</span>
            {judged.risk.map((r) => (
              <span key={r.name}>
                {" · "}
                {r.name} {fmt(r.seconds)}
                {r.no_task_seconds > 0 && ` (${fmt(r.no_task_seconds)} no task)`}
              </span>
            ))}
          </p>
        )}
      </div>

      {judged.pages.length > 0 && (
        <div className="surface mt-6 px-5 py-4">
          <div className="mb-3 flex items-baseline justify-between gap-4">
            <h3>Pages Jev judged</h3>
            <p className="font-mono text-xs text-muted-foreground">
              relevant at {bar}%{judged.models.length > 0 ? ` · ${judged.models.join(", ")}` : ""}
            </p>
          </div>
          <ul className="divide-y divide-border/60">
            {pages.map((p) => (
              <PageRow
                key={pageKey(p)}
                page={p}
                threshold={judged.threshold}
                appealable={appealable && p.verdict === "no_task"}
                won={won.has(pageKey(p))}
                onAppeal={() => onAppeal(p)}
              />
            ))}
          </ul>
          {hidden > 0 && (
            <div className="mt-2">
              <BarMore onClick={onShowAll}>{showAll ? "▾ fewer" : `▸ +${hidden} more`}</BarMore>
            </div>
          )}
        </div>
      )}
    </>
  );
}

function PageRow({
  page,
  threshold,
  appealable,
  won,
  onAppeal,
}: {
  page: JudgedPage;
  threshold: number;
  appealable: boolean;
  won: boolean;
  onAppeal: () => void;
}) {
  const excused = page.verdict === "excused";
  const verdict =
    page.verdict === "excused"
      ? `${pct(page.probability)} “${page.task_title}”`
      : page.verdict === "no_task"
        ? page.task_title
          ? `no task · best ${pct(page.probability)} “${page.task_title}”`
          : "no task"
        : "not judged";
  return (
    <li className="py-2">
      <div className="flex items-center gap-3 text-[13px]">
        <span className="min-w-0 flex-1 truncate" title={page.title}>
          <span className={excused ? "text-foreground" : "text-foreground/80"}>{page.name || page.title}</span>
          <span className="ms-2 font-mono text-[11px] text-muted-foreground">{page.app}</span>
        </span>
        <span className="w-12 flex-none text-right font-mono text-xs tabular-nums text-muted-foreground">
          {fmt(page.seconds)}
        </span>
      </div>
      <div className="mt-1 flex items-center gap-3">
        <span
          className={cn(
            "min-w-0 flex-1 truncate text-[12px]",
            excused ? "text-muted-foreground" : page.verdict === "no_task" ? "text-warn" : "text-muted-foreground",
          )}
        >
          {verdict}
          {page.source === "appeal" && " · after appeal"}
        </span>
        {won ? (
          <span className="flex-none text-[12px] text-muted-foreground">won on appeal — counts from the next check</span>
        ) : (
          appealable && (
            <Button
              variant="outline"
              size="sm"
              className="h-7"
              onClick={onAppeal}
              title="Argue that this page served a task. A win keeps the timer off it too."
            >
              Appeal
            </Button>
          )
        )}
      </div>
      {page.verdict !== "unjudged" && (
        <ProbabilityBar className="mt-1.5" probability={page.probability} threshold={threshold} excused={excused} />
      )}
    </li>
  );
}
