import React, { useEffect, useRef, useState } from "react";
import { Button, TextInput } from "../src/ui/primitives";
import { post } from "./bridge";
import type { Ask, Choice } from "./types";

/** The log pane, shared by both question screens.
 *
 *  This is where everything the gate print()s lands -- the recovery sweep and
 *  the debrief. Python tees it to the real stdout first, so what shows here is
 *  a copy, never the only record. */
function Log({ text }: { text: string }) {
  const box = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [text]);
  return (
    <pre
      ref={box}
      className="max-h-[55vh] flex-1 overflow-y-auto whitespace-pre-wrap break-words
                 rounded-[0.5rem] border border-line bg-surface p-[1rem]
                 font-mono text-[0.8rem] leading-relaxed text-muted"
    >
      {text}
    </pre>
  );
}

function Frame({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex h-full items-center justify-center">
      <div className="gate-rise flex w-[40rem] max-w-[calc(100%-4rem)] flex-col gap-[1.2rem]">
        {children}
      </div>
    </div>
  );
}

export function AskScreen({ state }: { state: Ask }) {
  const [text, setText] = useState("");
  return (
    <Frame>
      {state.log && <Log text={state.log} />}
      {state.question && (
        <p className="text-[1.15rem] font-medium text-text">{state.question}</p>
      )}
      <div className="flex gap-[0.5rem]">
        <TextInput
          value={text}
          autoFocus
          placeholder={state.placeholder}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") post("answer", { text }); }}
          className="flex-1"
        />
        <Button tone="primary" onClick={() => post("answer", { text })}>Enter</Button>
      </div>
    </Frame>
  );
}

export function ChoiceScreen({ state }: { state: Choice }) {
  // One keystroke answers, the way it does on the terminal. Python's own
  // Gtk.EventControllerKey is a backstop on the bubble phase; whichever fires
  // first wins, and the second is discarded because _wait takes answer[0].
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.altKey || e.metaKey) return;
      const hit = state.choices.find((c) => c.key === e.key.toLowerCase());
      if (hit) { e.preventDefault(); post("choose", { key: hit.key }); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [state.choices]);

  return (
    <Frame>
      {state.log && <Log text={state.log} />}
      {state.question && (
        <p className="text-[1.15rem] font-medium text-text">{state.question}</p>
      )}
      <div className="flex flex-wrap gap-[0.5rem]">
        {state.choices.map((c) => (
          <Button key={c.key} onClick={() => post("choose", { key: c.key })}>
            {c.label}
            <span className="ml-[0.5em] text-muted">[{c.key}]</span>
          </Button>
        ))}
      </div>
    </Frame>
  );
}
