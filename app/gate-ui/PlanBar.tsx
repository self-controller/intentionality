import { ArrowRight } from "lucide-react";
import { Button, Kbd, TextInput } from "../src/ui/primitives";
import Segmented from "./Segmented";

/** Presets write the same string Python already parses (ui.parse_minutes);
 *  the box beside them takes anything else. Blank is open-ended. */
const PRESETS = [
  { label: "30 min", value: "30" },
  { label: "1 h", value: "60" },
  { label: "90 min", value: "90" },
  { label: "2 h", value: "120" },
  { label: "Open-ended", value: "" },
];

/** The closing step, at the bottom: how long, how much, Start. The welcome
 *  screen is the window's height and only its columns scroll, so a long
 *  backlog or an open panel can never push Start off the screen. */
export default function PlanBar({
  minutes,
  setMinutes,
  count,
  doing,
  canStart,
  start,
  error,
}: {
  minutes: string;
  setMinutes: (m: string) => void;
  /** Tasks the session would start with, the add box included. */
  count: number;
  /** How many of those start in Doing. */
  doing: number;
  canStart: boolean;
  start: () => void;
  error: string;
}) {
  const trimmed = minutes.trim();
  const preset = PRESETS.some((p) => p.value === trimmed) ? trimmed : null;

  return (
    <div className="w-full flex-none border-t border-line">
      <div className="mx-auto flex w-full max-w-[104rem] flex-col gap-[0.5rem] px-[4rem] py-[1rem]">
        <div className="flex flex-wrap items-center gap-[0.9rem]">
          <label htmlFor="gate-minutes" className="text-[0.85rem] text-muted">
            How long will you be here?
          </label>
          <Segmented label="Session length" options={PRESETS} value={preset} onChange={setMinutes} />
          <TextInput
            id="gate-minutes"
            value={minutes}
            placeholder="min"
            inputMode="numeric"
            onChange={(e) => setMinutes(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && !e.ctrlKey && !e.altKey && !e.metaKey) start(); }}
            className="h-[2.2rem] w-[5.5rem] text-center font-mono text-[0.85rem]"
          />
          <span className="flex-1" />
          <span className="font-mono text-[0.74rem] text-muted">
            {count} {count === 1 ? "task" : "tasks"} today
            {doing > 0 && <span className="text-good"> · {doing} in Doing</span>}
          </span>
          <span className="flex items-center gap-[0.2rem] text-muted/60" aria-hidden="true">
            <Kbd>Ctrl</Kbd>
            <Kbd>Enter</Kbd>
          </span>
          <Button tone="primary" size="lg" onClick={start} disabled={!canStart}>
            Start session
            <ArrowRight aria-hidden="true" />
          </Button>
        </div>
        {error && <p className="text-[0.8rem] text-bad">{error}</p>}
      </div>
    </div>
  );
}
