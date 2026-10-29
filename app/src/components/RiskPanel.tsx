import { useCallback, useEffect, useRef, useState } from "react";
import { Hourglass, Plus, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import * as api from "../api";
import { fmt } from "../format";
import type { RiskToday } from "../types";

/** A full day of ActivityWatch events per look, so not every second. */
const POLL_MS = 30_000;

/**
 * Today's time on the at-risk apps, on the Dashboard: one row per flagged
 * app or website, where flags are added and removed, and the relevance
 * cache's size with the way to empty it. `onForget` runs once the cache is
 * gone, since every verdict the Dashboard's pie read went with it.
 */
export default function RiskPanel({ onForget }: { onForget?: () => void }) {
  const [today, setToday] = useState<RiskToday | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  // A refused add or remove, worded by Rust.
  const [problem, setProblem] = useState<string | null>(null);
  // "Forget" asks once before it drops every verdict.
  const [forgetting, setForgetting] = useState(false);
  // Answers can land out of order (a slow poll after a quick refresh); only
  // the newest request's answer is shown.
  const latest = useRef(0);

  const load = useCallback(() => {
    const ask = ++latest.current;
    api
      .riskToday()
      .then((t) => ask === latest.current && setToday(t))
      .catch((e) => ask === latest.current && setProblem(String(e)));
  }, []);

  useEffect(() => {
    load();
    const id = window.setInterval(load, POLL_MS);
    return () => window.clearInterval(id);
  }, [load]);

  const change = (write: Promise<unknown>, after?: () => void) => {
    setBusy(true);
    setProblem(null);
    write
      .then(() => {
        after?.();
        load();
      })
      .catch((e) => setProblem(String(e)))
      .finally(() => setBusy(false));
  };
  const add = () => {
    if (draft.trim()) change(api.addRiskApp(draft), () => setDraft(""));
  };

  const apps = [...(today?.apps ?? [])].sort((a, b) => b.seconds - a.seconds || a.id - b.id);
  const most = Math.max(1, ...apps.map((a) => a.seconds));
  const counted = today != null && apps.length > 0 && !today.error;

  return (
    <section className="surface flex flex-col px-5 py-4" aria-label="At-risk apps">
      <div className="mb-3 flex items-baseline justify-between gap-4">
        <h3 className="flex items-center gap-1.5">
          <Hourglass className="size-3.5 flex-none" />
          At-risk apps · today
        </h3>
        {counted && (
          <span className="font-mono text-xs tabular-nums text-foreground" title="A moment counts once, however many flags it matched">
            {fmt(today.total)}
          </span>
        )}
      </div>
      {today?.error && <p className="pb-1.5 text-xs text-warn">ActivityWatch unreachable — no times right now</p>}
      {today != null && apps.length === 0 ? (
        <p className="pb-2 text-[13px] leading-snug text-muted-foreground">
          Nothing flagged. Add an app or a website, and a timer of today's total sits at the top right of the
          screen while it is the window in front.
        </p>
      ) : (
        <ul className="-mx-2 max-h-72 list-none overflow-y-auto p-0">
          {apps.map((a) => (
            <li key={a.id} className="group rounded-md px-2 py-1.5 hover:bg-secondary">
              <div className="flex items-center gap-2 text-[13px]">
                <span className="min-w-0 flex-1 truncate text-foreground">{a.name}</span>
                <span className="font-mono text-xs tabular-nums text-muted-foreground">
                  {today?.error ? "—" : fmt(a.seconds)}
                </span>
                <button
                  type="button"
                  aria-label={`Stop flagging ${a.name}`}
                  title="Stop flagging"
                  disabled={busy}
                  onClick={() => change(api.removeRiskApp(a.id))}
                  className="-me-1 flex size-5 flex-none items-center justify-center rounded text-muted-foreground opacity-0 transition-opacity hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100"
                >
                  <X className="size-3.5" />
                </button>
              </div>
              <div className="mt-1 h-1 overflow-hidden rounded-full bg-secondary group-hover:bg-background/60">
                <div
                  className="h-full rounded-full bg-warn/80"
                  style={{ width: `${today?.error ? 0 : (a.seconds / most) * 100}%` }}
                />
              </div>
            </li>
          ))}
        </ul>
      )}
      <form
        className="mt-2 flex items-center gap-1.5"
        onSubmit={(e) => {
          e.preventDefault();
          add();
        }}
      >
        <Input
          className="h-8 min-w-0 flex-1 text-[13px]"
          value={draft}
          disabled={busy}
          placeholder="Add an app or site — e.g. linkedin.com"
          aria-label="App or website to flag"
          onChange={(e) => setDraft(e.target.value)}
        />
        <Button type="submit" mode="icon" size="sm" aria-label="Flag it" disabled={busy || !draft.trim()}>
          <Plus />
        </Button>
      </form>
      {problem && <p className="pt-1.5 text-xs text-warn">{problem}</p>}
      {/* The relevance cache: pages Jev has judged against tasks. Nothing is
          lost by forgetting but the requests to ask again. */}
      <div className="mt-3 flex items-center gap-2 border-t border-border pt-2.5 text-[11px] text-muted-foreground">
        <span className="min-w-0 flex-1 truncate">
          {today == null
            ? ""
            : today.judged === 0
              ? "No pages judged by Jev yet"
              : `${today.judged} page–task ${today.judged === 1 ? "verdict" : "verdicts"} from Jev`}
        </span>
        {today != null && today.judged > 0 && !forgetting && (
          <Button variant="outline" size="sm" className="h-7" disabled={busy} onClick={() => setForgetting(true)}>
            Forget
          </Button>
        )}
        {forgetting && (
          <>
            <span className="flex-none">Ask again on every page?</span>
            <Button
              variant="destructive"
              size="sm"
              className="h-7"
              disabled={busy}
              onClick={() => {
                setForgetting(false);
                change(api.forgetRelevance(), onForget);
              }}
            >
              Forget all
            </Button>
            <Button variant="outline" size="sm" className="h-7" onClick={() => setForgetting(false)}>
              Keep
            </Button>
          </>
        )}
      </div>
    </section>
  );
}
