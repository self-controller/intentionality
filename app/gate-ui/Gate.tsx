import { useEffect, useState } from "react";
import { onPush, post } from "./bridge";
import type { State } from "./types";
import Intro from "./Intro";
import Welcome from "./Welcome";
import { AskScreen, ChoiceScreen } from "./Prompt";

const INTRO_MS = 1100;
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
    const t = window.setTimeout(skip, INTRO_MS);
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

  return (
    <>
      {state?.screen === "welcome" && <Welcome state={state} />}
      {state?.screen === "ask" && <AskScreen state={state} />}
      {state?.screen === "choice" && <ChoiceScreen state={state} />}
      {!gone && <Intro leaving={done || !wanted} />}
    </>
  );
}
