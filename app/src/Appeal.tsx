import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import * as api from "./api";
import { pct, ProbabilityBar } from "@/components/ProbabilityBar";
import type { Judgement, PageRef, Verdict } from "./types";

/** A turn in the argument: what the user said, or how Jev's verdict read. */
type Turn = { who: "you" | "jev"; text: string };

/** Jev gives probabilities, not prose; this is the sentence they become. */
function reply(j: Judgement): string {
  if (j.no_tasks) return "No open tasks on the board, so nothing this page could be relevant to.";
  const i = j.relevant;
  if (i != null) {
    const v = j.verdicts[i];
    return `Jev agrees: ${pct(v.probability)} relevant to “${v.task_title}”. The timer stays off this page.`;
  }
  const judged = j.verdicts.filter((v) => v.probability != null);
  if (judged.length === 0) {
    return j.error ? `Could not ask Jev: ${j.error}` : "Not judged yet.";
  }
  const best = judged.reduce((a, b) => ((b.probability ?? 0) > (a.probability ?? 0) ? b : a));
  const bar = Math.round(j.threshold * 100);
  return `Not convinced — best ${pct(best.probability)} for “${best.task_title}”, and it takes ${bar}%. Add to your case.`;
}

function VerdictRow({ v, excused, threshold }: { v: Verdict; excused: boolean; threshold: number }) {
  return (
    <li className="py-1.5">
      <div className="flex items-center gap-3 text-[13px]">
        <span className={cn("min-w-0 flex-1 truncate", excused ? "text-foreground" : "text-muted-foreground")}>
          {v.task_title}
        </span>
        {v.source === "appeal" && <span className="text-[11px] text-muted-foreground">after appeal</span>}
        <span className={cn("w-10 text-right font-mono text-xs tabular-nums", excused ? "text-foreground" : "text-muted-foreground")}>
          {pct(v.probability)}
        </span>
      </div>
      <ProbabilityBar className="mt-1" probability={v.probability} threshold={threshold} excused={excused} />
    </li>
  );
}

/**
 * The appeal: the clock's arrow was clicked over a page Jev found irrelevant
 * to every task, and here the user argues otherwise. Each send puts the
 * whole case so far to Jev again, with every task; a verdict over the bar
 * stops the timer for this page and is remembered.
 *
 * An overlay like Checkpoint: a moment, not a place.
 */
export default function Appeal({ page, onClose }: { page: PageRef; onClose: () => void }) {
  const [judgement, setJudgement] = useState<Judgement | null>(null);
  const [turns, setTurns] = useState<Turn[]>([]);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const argument = useRef<string[]>([]);
  const box = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    api
      .pageJudgement(page.app, page.title)
      .then((j) => {
        setJudgement(j);
        setTurns([{ who: "jev", text: reply(j) }]);
      })
      .catch((e) => setError(String(e)));
    box.current?.focus();
  }, [page.app, page.title]);

  const send = () => {
    const text = draft.trim();
    if (!text || busy) return;
    argument.current = [...argument.current, text];
    setTurns((t) => [...t, { who: "you", text }]);
    setDraft("");
    setBusy(true);
    setError(null);
    api
      .appeal(page.app, page.title, argument.current)
      .then((j) => {
        setJudgement(j);
        setTurns((t) => [...t, { who: "jev", text: reply(j) }]);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  const won = judgement?.relevant != null;
  const canArgue = judgement != null && !judgement.no_tasks && !won;

  return (
    <div className="fixed inset-0 z-30 flex items-center justify-center overflow-y-auto bg-black/70 p-6 backdrop-blur-sm">
      <section
        className="surface flex max-h-full w-full max-w-[640px] flex-col overflow-hidden rounded-2xl shadow-2xl shadow-black/60"
        role="dialog"
        aria-modal="true"
        aria-label="Appeal"
      >
        <div className="bg-grid border-b border-border px-7 pb-5 pt-6">
          <p className="eyebrow">{page.from === "check" ? `${page.label} · from a check` : `${page.label} · at risk`}</p>
          <h1 className="mt-2 truncate text-2xl" title={page.title}>
            {page.title || page.app}
          </h1>
          <p className="mt-1 text-[13px] text-muted-foreground">
            Is this page relevant to a task? Make the case; Jev re-judges it against every open task.
          </p>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto px-7 py-5">
          {judgement && judgement.verdicts.length > 0 && (
            <ul className="mb-5 rounded-lg border border-border bg-background/50 px-4 py-2">
              {judgement.verdicts.map((v, i) => (
                <VerdictRow key={v.task_id} v={v} excused={judgement.relevant === i} threshold={judgement.threshold} />
              ))}
            </ul>
          )}
          <ol className="space-y-3">
            {turns.map((t, i) => (
              <li key={i} className={cn("flex", t.who === "you" ? "justify-end" : "justify-start")}>
                <p
                  className={cn(
                    "max-w-[85%] whitespace-pre-wrap rounded-xl px-3.5 py-2 text-[14px] leading-relaxed",
                    t.who === "you" ? "bg-primary text-primary-foreground" : "bg-secondary text-foreground",
                  )}
                >
                  {t.text}
                </p>
              </li>
            ))}
            {busy && (
              <li className="flex justify-start">
                <p className="rounded-xl bg-secondary px-3.5 py-2 text-[14px] text-muted-foreground">Asking Jev…</p>
              </li>
            )}
          </ol>
          {error && <p className="mt-3 text-[13px] text-destructive">{error}</p>}
        </div>

        <div className="border-t border-border px-7 py-4">
          {canArgue && (
            <form
              className="flex items-end gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                send();
              }}
            >
              <Textarea
                ref={box}
                rows={2}
                className="min-h-[2.5rem] flex-1 resize-none text-[14px]"
                placeholder="Why this page serves one of the tasks — Enter sends, Shift+Enter for a new line"
                value={draft}
                disabled={busy}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey) {
                    e.preventDefault();
                    send();
                  }
                }}
              />
              <Button type="submit" size="lg" disabled={busy || !draft.trim()}>
                Send
              </Button>
              <Button type="button" variant="outline" size="lg" onClick={onClose} disabled={busy}>
                Close
              </Button>
            </form>
          )}
          {!canArgue && (
            <div className="flex items-center gap-2.5">
              <Button size="lg" onClick={onClose}>
                {won ? "Done" : "Close"}
              </Button>
              {won && <span className="text-[13px] text-muted-foreground">Remembered for this page.</span>}
            </div>
          )}
        </div>
      </section>
    </div>
  );
}
