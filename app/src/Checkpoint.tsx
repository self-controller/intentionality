import { useState } from "react";
import * as api from "./api";
import { band, hhmm } from "./format";
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
    <div className="checkpoint-scrim">
      <section className="checkpoint" role="dialog" aria-modal="true" aria-label="Time is up">
        <h1>
          Time's up
          <span className="muted">
            {intended != null && elapsed != null
              ? ` — ${intended} min intended, ${elapsed} elapsed`
              : elapsed != null
                ? ` — ${elapsed} min elapsed`
                : ""}
          </span>
        </h1>

        <div className="checkpoint-read">
          <span className={`align ${b.cls}`}>
            {b.glyph} {analysis.alignment ?? "—"}
          </span>
          <span className={`align ${b.cls} label`}>{b.label}</span>
          <span className="muted">
            covers {hhmm(analysis.window_start)}–{hhmm(analysis.window_end)}
          </span>
        </div>

        <h2>{analysis.headline}</h2>
        <p className="detail-body">{analysis.body}</p>

        {rec && (
          <div className="recommend">
            <h3>Recommended</h3>
            <p className="advice">
              {rec.advice}
              {rec.minutes > 0 && <span className="muted"> · about {rec.minutes} min</span>}
            </p>
            {analysis.recommendation_note && (
              <p className="muted note">{analysis.recommendation_note}</p>
            )}
            <p className="why">{rec.why}</p>
            {/* Said plainly rather than left to be assumed: these are
                defaults, and the day one of them earns a citation it says so. */}
            <p className="muted source">
              {rec.source ? rec.source : "Not yet backed by a cited study."}
            </p>
          </div>
        )}

        <div className="checkpoint-actions">
          <button onClick={() => finish()} disabled={busy}>
            Got it
          </button>
          {EXTENSIONS.map((m) => (
            <button key={m} onClick={() => finish(m)} disabled={busy}>
              +{m} min
            </button>
          ))}
          {error && <span className="muted">{error}</span>}
        </div>
      </section>
    </div>
  );
}
