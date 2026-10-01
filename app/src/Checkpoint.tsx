import { useState } from "react";
import { Button } from "@/components/ui/button";
import * as api from "./api";
import { ALIGN_TEXT, band, hhmm } from "./format";
import type { Analysis, Session } from "./types";

// What "+N min" offers. Deliberately short: this is a nudge past the line you
// drew, not a way to quietly redraw the line.
const EXTENSIONS = [15, 30];

function elapsedMinutes(session: Session | null, at: string): number | null {
  if (!session) return null;
  const ms = new Date(at).getTime() - new Date(session.started_at).getTime();
  return Math.round(ms / 60000);
}

/**
 * The time-up screen. An overlay rather than a nav tab: it is a moment, not a
 * place, and once acknowledged there is nothing to come back to — the row
 * stays in the Analyses history like any other.
 */
export default function Checkpoint({
  analysis,
  session,
  onDismiss,
}: {
  analysis: Analysis;
  session: Session | null;
  onDismiss: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const b = band(analysis.alignment);
  const elapsed = elapsedMinutes(session, analysis.created_at);
  const intended = session?.intended_minutes ?? null;
  const rec = analysis.recommendation;

  // Dismissing marks the row seen, which is also what keeps pending_checkpoint
  // from showing it again on the next start.
  const finish = (extend?: number) => {
    setBusy(true);
    setError(null);
    const work = extend == null ? Promise.resolve() : api.extendCheckpoint(extend);
    work
      .then(() => api.markAnalysisSeen(analysis.id))
      .then(onDismiss)
      .catch((e) => {
        setError(String(e));
        setBusy(false);
      });
  };

  return (
    <div className="fixed inset-0 z-30 flex items-center justify-center overflow-y-auto bg-black/70 p-6 backdrop-blur-sm">
      <section
        className="surface max-h-full w-full max-w-[580px] overflow-y-auto rounded-2xl shadow-2xl shadow-black/60"
        role="dialog"
        aria-modal="true"
        aria-label="Time is up"
      >
        <div className="bg-grid border-b border-border px-7 pb-6 pt-7">
          <p className="eyebrow">
            {intended != null && elapsed != null
              ? `${intended} min intended · ${elapsed} elapsed`
              : elapsed != null
                ? `${elapsed} min elapsed`
                : "Checkpoint"}
          </p>
          <h1 className="mt-2 text-4xl">Time&rsquo;s up</h1>
          <div className="mt-4 flex items-baseline gap-3">
            <span className={"font-mono text-2xl font-medium tracking-[-0.03em] tabular-nums " + ALIGN_TEXT[b.cls]}>
              {b.glyph} {analysis.alignment ?? "—"}
            </span>
            <span className={"eyebrow " + ALIGN_TEXT[b.cls]}>{b.label}</span>
            <span className="font-mono text-xs text-muted-foreground">
              covers {hhmm(analysis.window_start)}–{hhmm(analysis.window_end)}
            </span>
          </div>
        </div>

        <div className="px-7 pb-7 pt-6">
          <h2>{analysis.headline}</h2>
          <p className="mt-2 max-w-[60ch] leading-relaxed">{analysis.body}</p>

          {rec && (
            <div className="mt-5 rounded-lg border border-border bg-background/50 px-4 py-3.5">
              <h3>Recommended</h3>
              <p className="mb-1.5 mt-2 text-[15px] font-medium tracking-[-0.01em]">
                {rec.advice}
                {rec.minutes > 0 && <span className="font-normal text-muted-foreground"> · about {rec.minutes} min</span>}
              </p>
              {analysis.recommendation_note && (
                <p className="mb-2 text-[13px] text-muted-foreground">{analysis.recommendation_note}</p>
              )}
              <p className="mb-2 text-foreground/90">{rec.why}</p>
              {/* Said plainly rather than left to be assumed: these are
                  defaults, and the day one of them earns a citation it says so. */}
              <p className="font-mono text-[11px] text-muted-foreground">
                {rec.source ? rec.source : "Not yet backed by a cited study."}
              </p>
            </div>
          )}

          <div className="mt-6 flex items-center gap-2.5">
            {/* Got it is the one that closes the moment; the extensions are
                the alternatives to it. */}
            <Button size="lg" onClick={() => finish()} disabled={busy}>
              Got it
            </Button>
            {EXTENSIONS.map((m) => (
              <Button key={m} variant="outline" size="lg" onClick={() => finish(m)} disabled={busy}>
                +{m} min
              </Button>
            ))}
            {error && <span className="text-destructive">{error}</span>}
          </div>
        </div>
      </section>
    </div>
  );
}
