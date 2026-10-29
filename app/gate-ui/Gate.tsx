import { useEffect, useState } from "react";
import { onPush, post } from "./bridge";
import type { State } from "./types";
import Intro, { FLIGHT_S } from "./Intro";
import Stars from "./Stars";
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

  // Scrolling arrives from Python, not from wheel events. gate/gui.py takes
  // every scroll before WebKit sees it (WebKit's kinetic coast cannot be
  // stopped from the page, and it ran the wrong way at the console), turns it
  // the way the desktop scrolls, and calls by() per step, fling() when a
  // touchpad flick ends and stop() when fingers land again. Opened in a plain
  // browser nothing calls these, and the browser scrolls as usual.
  useEffect(() => {
    // Where the pointer is: the scroller under it takes the step.
    let px = window.innerWidth / 2;
    let py = window.innerHeight / 2;
    const track = (e: PointerEvent) => { px = e.clientX; py = e.clientY; };

    const canScroll = (el: Element, dy: number) =>
      dy < 0 ? el.scrollTop > 0 : el.scrollTop + el.clientHeight < el.scrollHeight - 1;
    // The nearest scroller under the pointer that can still move, else the
    // page, as native scroll-chaining would choose.
    const scrollerFor = (dy: number): Element | null => {
      let el = document.elementFromPoint(px, py);
      while (el && el !== document.documentElement && el !== document.body) {
        const { overflowY } = getComputedStyle(el);
        if ((overflowY === "auto" || overflowY === "scroll") && canScroll(el, dy)) return el;
        el = el.parentElement;
      }
      return document.scrollingElement;
    };
    const move = (dx: number, dy: number): boolean => {
      const el = scrollerFor(dy);
      if (!el) return false;
      const before = el.scrollTop;
      el.scrollBy({ left: dx, top: dy, behavior: "instant" });
      return el.scrollTop !== before;
    };

    // The coast: velocity decays exponentially, at WebKit's and GTK's own
    // kinetic friction, and stops at an edge or when it has slowed to nothing.
    const FRICTION = 4; // per second
    let raf = 0;
    const stop = () => {
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
    };
    const fling = (vx: number, vy: number) => {
      stop();
      let last = performance.now();
      const step = (now: number) => {
        const dt = (now - last) / 1000;
        last = now;
        const decay = Math.exp(-FRICTION * dt);
        // Distance covered over dt at a velocity decaying from v.
        const ddy = (vy * (1 - decay)) / FRICTION;
        const ddx = (vx * (1 - decay)) / FRICTION;
        vx *= decay;
        vy *= decay;
        if (Math.abs(vy) < 20 || !move(ddx, ddy)) {
          raf = 0;
          return;
        }
        raf = requestAnimationFrame(step);
      };
      raf = requestAnimationFrame(step);
    };

    window.__gateScroll = {
      by: (dx, dy) => { stop(); move(dx, dy); },
      fling,
      stop,
    };
    window.addEventListener("pointermove", track);
    window.addEventListener("pointerdown", stop);
    window.addEventListener("keydown", stop);
    return () => {
      stop();
      delete window.__gateScroll;
      window.removeEventListener("pointermove", track);
      window.removeEventListener("pointerdown", stop);
      window.removeEventListener("keydown", stop);
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
      <Stars />
      <div className="relative z-10 flex flex-1 flex-col">
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
    </>
  );
}
