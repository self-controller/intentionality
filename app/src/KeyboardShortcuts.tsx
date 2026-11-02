import { useEffect, useRef, useState } from "react";
import { ArrowLeft } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import {
  ACTIONS,
  FIXED,
  clashes,
  comboOf,
  keycaps,
  modifierOf,
  usesSuper,
  type Action,
  type Section,
} from "./keybinds";
import { saveKeybinds, useKeybinds } from "./useKeybinds";

const SECTIONS: { id: Section; title: string; note: string }[] = [
  {
    id: "gate",
    title: "Gate",
    note: "The screen you plan the session on. Changes here take effect the next time the gate opens.",
  },
  {
    id: "app",
    title: "Intentionality app",
    note: "This window. Changes take effect at once.",
  },
];

/**
 * Settings' keyboard shortcuts: every key the gate and the app answer to, in
 * two sections. A shortcut is changed by clicking its keys and pressing the
 * new ones; the plain keys (arrows, Enter, Esc) are listed but fixed.
 */
export default function KeyboardShortcuts({ onBack }: { onBack: () => void }) {
  const keys = useKeybinds();
  // The row waiting for keys, if any.
  const [recording, setRecording] = useState<string | null>(null);
  // A row's last refusal or failure, shown under it.
  const [problem, setProblem] = useState<{ id: string; text: string } | null>(null);

  const save = (id: string, combo: string) => {
    const next = { ...keys, [id]: combo };
    const clash = clashes(next)[id];
    if (clash) {
      setProblem({ id, text: `${keycaps(combo).join(" + ")} is already ${clash}.` });
      return;
    }
    setProblem(null);
    saveKeybinds(next).catch((e) => setProblem({ id, text: String(e) }));
  };

  const resetAll = () => {
    setProblem(null);
    saveKeybinds(Object.fromEntries(ACTIONS.map((a) => [a.id, a.default]))).catch((e) =>
      setProblem({ id: "", text: String(e) }),
    );
  };

  const changed = ACTIONS.some((a) => keys[a.id] !== a.default);

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center gap-3">
        <Button variant="ghost" mode="icon" size="sm" aria-label="Back to Settings" title="Back" onClick={onBack}>
          <ArrowLeft />
        </Button>
        <p className="eyebrow">Settings · Keyboard shortcuts</p>
        {changed && (
          <Button variant="outline" size="sm" className="ms-auto" onClick={resetAll}>
            Reset all
          </Button>
        )}
      </div>
      {problem?.id === "" && <p className="text-xs text-warn">{problem.text}</p>}

      {SECTIONS.map((section) => (
        <section key={section.id} className="surface flex flex-col px-5 py-4" aria-label={section.title}>
          <h3>{section.title}</h3>
          <p className="text-xs text-muted-foreground">{section.note}</p>
          <ul className="mt-3 flex flex-col">
            {ACTIONS.filter((a) => a.section === section.id).map((a) => (
              <ShortcutRow
                key={a.id}
                action={a}
                combo={keys[a.id]}
                recording={recording === a.id}
                problem={problem?.id === a.id ? problem.text : null}
                onRecord={() => {
                  setProblem(null);
                  setRecording(a.id);
                }}
                onDone={(combo) => {
                  setRecording(null);
                  if (combo !== null && combo !== keys[a.id]) save(a.id, combo);
                }}
                onReset={() => save(a.id, a.default)}
              />
            ))}
            {FIXED.filter((f) => f.section === section.id).map((f) => (
              <li
                key={f.label}
                className="flex items-center gap-3 border-t border-border py-2.5 text-[13px] text-muted-foreground"
              >
                <span className="min-w-0 flex-1">{f.label}</span>
                <span className="flex flex-none items-center gap-1.5 opacity-70">
                  {f.keys.map((k, i) => (
                    <Caps key={i} caps={keycaps(k.join("+"))} />
                  ))}
                </span>
                <span className="w-12 flex-none text-right text-[11px]">fixed</span>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

function ShortcutRow({
  action,
  combo,
  recording,
  problem,
  onRecord,
  onDone,
  onReset,
}: {
  action: Action;
  combo: string;
  recording: boolean;
  problem: string | null;
  onRecord: () => void;
  /** The keys pressed, "" to unbind, null for cancelled. */
  onDone: (combo: string | null) => void;
  onReset: () => void;
}) {
  useRecorder(recording, onDone);
  const superInApp = action.section === "app" && usesSuper(combo);

  return (
    <li className="flex flex-col border-t border-border py-2.5 text-[13px]">
      <div className="flex items-center gap-3">
        <span className="min-w-0 flex-1">
          {action.label}
          {action.where && <span className="ms-2 text-xs text-muted-foreground">{action.where}</span>}
        </span>
        <button
          type="button"
          onClick={recording ? () => onDone(null) : onRecord}
          aria-label={`Change the shortcut for ${action.label}`}
          className={cn(
            "flex h-8 min-w-28 flex-none cursor-pointer items-center justify-center gap-1.5 rounded-md border px-2",
            "outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring",
            recording
              ? "border-primary/60 bg-primary/10 text-foreground"
              : "border-transparent hover:border-border hover:bg-secondary",
          )}
        >
          {recording ? (
            <span className="text-xs text-muted-foreground">Press keys… Esc cancels</span>
          ) : combo ? (
            <Caps caps={keycaps(combo)} />
          ) : (
            <span className="text-xs text-muted-foreground">Not set</span>
          )}
        </button>
        <span className="w-12 flex-none text-right">
          {combo !== action.default && !recording && (
            <button
              type="button"
              onClick={onReset}
              className="cursor-pointer text-[11px] text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
            >
              Reset
            </button>
          )}
        </span>
      </div>
      {recording && (
        <p className="pt-1.5 text-xs text-muted-foreground">
          Backspace leaves it unset. A modifier on its own, like Super, counts when you let go of it.
        </p>
      )}
      {problem && <p className="pt-1.5 text-xs text-warn">{problem}</p>}
      {superInApp && !problem && (
        <p className="pt-1.5 text-xs text-warn">GNOME keeps Super for itself, so this app may never see it.</p>
      )}
    </li>
  );
}

function Caps({ caps }: { caps: string[] }) {
  return (
    <span className="flex items-center gap-1">
      {caps.map((c, i) => (
        <kbd
          key={i}
          className="rounded border border-border bg-secondary px-1.5 py-0.5 font-mono text-[11px] leading-4 text-foreground"
        >
          {c}
        </kbd>
      ))}
    </span>
  );
}

/**
 * While `on`, the next keys pressed are the shortcut: a chord on its key
 * down, a modifier alone when it is let go. Taken in the capture phase and
 * stopped there, so Ctrl+Tab or Ctrl+N being recorded does not also switch
 * tabs or make a task.
 */
function useRecorder(on: boolean, onDone: (combo: string | null) => void) {
  const doneRef = useRef(onDone);
  doneRef.current = onDone;
  useEffect(() => {
    const done = (combo: string | null) => doneRef.current(combo);
    if (!on) return;
    let lone: string | null = null;
    const down = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.repeat) return;
      const mod = modifierOf(e);
      if (mod) {
        const others = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Super"].filter(
          (m) => m && m !== mod,
        );
        lone = others.length ? null : mod;
        return;
      }
      lone = null;
      const bare = !e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey;
      if (bare && e.code === "Escape") done(null);
      else if (bare && (e.code === "Backspace" || e.code === "Delete")) done("");
      else done(comboOf(e));
    };
    const up = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (lone && modifierOf(e) === lone) done(lone);
    };
    // A click anywhere else gives up, as Esc does.
    const away = (e: PointerEvent) => {
      if (!(e.target as HTMLElement | null)?.closest("[aria-label^='Change the shortcut']")) done(null);
    };
    window.addEventListener("keydown", down, true);
    window.addEventListener("keyup", up, true);
    window.addEventListener("pointerdown", away, true);
    return () => {
      window.removeEventListener("keydown", down, true);
      window.removeEventListener("keyup", up, true);
      window.removeEventListener("pointerdown", away, true);
    };
  }, [on]);
}
