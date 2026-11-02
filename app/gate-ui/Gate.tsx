import { useEffect, useState } from "react";
import { domAnimation, LazyMotion } from "framer-motion";
import { onPush, post } from "./bridge";
import type { State } from "./types";
import Intro, { FLIGHT_S } from "./Intro";
import Welcome from "./Welcome";
import { BlankScreen, ChoiceScreen } from "./Prompt";

// The tunnel's flight plus a beat on the title; with reduced motion there is
// no flight, only the title fading in.
const INTRO_MS = FLIGHT_S * 1000 + 300;
const STILL_INTRO_MS = 1100;
const FADE_MS = 500;

export default function Gate() {
  const [state, setState] = useState<State | null>(null);
  const [done, setDone] = useState(false);
  const [gone, setGone] = useState(false);

  useEffect(() => {
    onPush(setState);
    post("ready");
  }, []);

  useEffect(() => {
    const skip = () => setDone(true);
    const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const t = window.setTimeout(skip, still ? STILL_INTRO_MS : INTRO_MS);
    window.addEventListener("keydown", skip);
    window.addEventListener("pointerdown", skip);
    return () => {
      window.clearTimeout(t);
      window.removeEventListener("keydown", skip);
      window.removeEventListener("pointerdown", skip);
    };
  }, []);

  // The intro covers the webview's own start-up and nothing else. It never
  // stands in front of a question: `gate close` opens on a debrief, and
  // "Let's get to work." would be the wrong thing to say to someone finishing.
  const wanted = state === null || state.screen === "welcome";
  useEffect(() => {
    if (done || !wanted) {
      const t = window.setTimeout(() => setGone(true), FADE_MS);
      return () => window.clearTimeout(t);
    }
  }, [done, wanted]);

  // One LazyMotion for every screen: the intro's `m` components draw their
  // features from it. domAnimation only -- the welcome screen animates with
  // CSS, because layout animations on the rows made a tick feel slow -- and
  // strict, so a stray `motion` import (the whole library) fails loudly.
  return (
    <LazyMotion features={domAnimation} strict>
      <div className="flex flex-1 flex-col">
        {state?.screen === "welcome" && <Welcome state={state} covered={!done} />}
        {state?.screen === "blank" && <BlankScreen state={state} />}
        {state?.screen === "choice" && <ChoiceScreen state={state} />}
      </div>
      {!gone && (
        <Intro
          leaving={done || !wanted}
          titles={state?.screen === "welcome" ? state.rows.map((r) => r.title) : []}
          date={state?.screen === "welcome" ? state.date : ""}
        />
      )}
    </LazyMotion>
  );
}
