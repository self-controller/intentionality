import { useEffect, useState } from "react";
import {
  animate,
  domAnimation,
  LazyMotion,
  m,
  useMotionValue,
  useReducedMotion,
  useSpring,
  useTransform,
  type MotionValue,
} from "framer-motion";
// `m` under LazyMotion with domAnimation, not `motion`: everything here is a
// MotionValue bound through style, which needs none of the gesture or layout
// features, and that keeps ~50 kB out of a bundle that loads at login. `m`
// needs the LazyMotion around it -- without it the styles never update.

/** How long the cards take to fly past. Gate.tsx's INTRO_MS waits for this
 *  and then a beat on the title before it fades the intro out. */
export const FLIGHT_S = 2.0;

const CARD_COUNT = 9;
/** The share of the flight each card is on screen for; the rest staggers. */
const STAGGER_SPAN = 0.55;
const FAR_Z = -1700;
const NEAR_Z = 500;
const PAN_PX = 45;
const PARALLAX = { stiffness: 60, damping: 25, mass: 0.5 };

interface Card {
  index: number;
  cos: number;
  sin: number;
  /** Distance from the centre, as a share of the stage's width / height. */
  rx: number;
  ry: number;
  tilt: number;
}

// Spread round the title on the golden angle, so no two neighbours line up.
const CARDS: Card[] = Array.from({ length: CARD_COUNT }, (_, i) => {
  const angle = i * 137.5 * (Math.PI / 180);
  return {
    index: i,
    cos: Math.cos(angle),
    sin: Math.sin(angle),
    rx: 0.28 + (i % 3) * 0.08,
    ry: 0.26 + (i % 2) * 0.09,
    tilt: (i % 2 === 0 ? 1 : -1) * (3 + (i % 5) * 2),
  };
});

interface Stage {
  width: number;
  height: number;
}

function useStage(): Stage {
  const read = () => ({ width: window.innerWidth, height: window.innerHeight });
  const [stage, setStage] = useState(read);
  useEffect(() => {
    const onResize = () => setStage(read());
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return stage;
}

/** The pointer, as -1..1 across the window, sprung so the camera drifts. */
function useParallax() {
  const rawX = useMotionValue(0);
  const rawY = useMotionValue(0);
  const x = useSpring(rawX, PARALLAX);
  const y = useSpring(rawY, PARALLAX);
  useEffect(() => {
    const onMove = (e: PointerEvent) => {
      rawX.set((e.clientX / window.innerWidth - 0.5) * 2);
      rawY.set((e.clientY / window.innerHeight - 0.5) * 2);
    };
    window.addEventListener("pointermove", onMove, { passive: true });
    return () => window.removeEventListener("pointermove", onMove);
  }, [rawX, rawY]);
  return { x, y };
}

function TunnelCard({
  card,
  title,
  progress,
  pointer,
  stage,
}: {
  card: Card;
  /** A task from the backlog, once the welcome state has arrived. */
  title: string | undefined;
  progress: MotionValue<number>;
  pointer: { x: MotionValue<number>; y: MotionValue<number> };
  stage: Stage;
}) {
  const start = (card.index / CARD_COUNT) * (1 - STAGGER_SPAN);
  const local = useTransform(progress, [start, start + STAGGER_SPAN], [0, 1]);
  const z = useTransform(local, [0, 1], [FAR_Z, NEAR_Z]);
  const opacity = useTransform(local, [0, 0.18, 0.82, 0.98], [0, 1, 1, 0]);

  const width = Math.max(180, Math.min(270, stage.width * 0.18));
  const baseX = card.cos * stage.width * card.rx;
  const baseY = card.sin * stage.height * card.ry;
  const transform = useTransform(
    [z, pointer.x, pointer.y],
    ([cz, px, py]: number[]) =>
      `translate3d(calc(-50% + ${baseX - px * PAN_PX}px), ` +
      `calc(-50% + ${baseY - py * PAN_PX}px), ${cz}px) rotateZ(${card.tilt}deg)`,
  );

  return (
    <m.div
      style={{ width, transform, opacity }}
      className="absolute left-1/2 top-1/2 aspect-[4/5] will-change-transform"
    >
      <div className="intro-card">
        <span className="intro-card-check" />
        {title ? (
          <span className="intro-card-title">{title}</span>
        ) : (
          <span className="intro-card-lines">
            <span />
            <span />
          </span>
        )}
      </div>
    </m.div>
  );
}

function Tunnel({ titles, date }: { titles: string[]; date: string }) {
  const stage = useStage();
  const pointer = useParallax();

  // Driven by the clock, not the wheel: at the gate scrolling belongs to
  // gui.py, and there is nothing here to scroll anyway.
  const progress = useMotionValue(0);
  useEffect(() => {
    const flight = animate(progress, 1, { duration: FLIGHT_S, ease: [0.45, 0, 0.25, 1] });
    return () => flight.stop();
  }, [progress]);

  const backdrop = useTransform(progress, [0, 0.2], [0, 1]);
  const rings = useTransform(progress, [0, 1], [0.92, 1.12]);
  const titleScale = useTransform(progress, [0, 0.55, 1], [0.7, 1, 1.05]);
  const titleOpacity = useTransform(progress, [0.05, 0.35], [0, 1]);

  return (
    <div
      className="relative flex h-full w-full items-center justify-center overflow-hidden"
      style={{ perspective: "1200px", transformStyle: "preserve-3d" }}
    >
      <m.div style={{ opacity: backdrop }} className="pointer-events-none absolute inset-0">
        <div className="intro-dots absolute inset-0" />
        <div className="intro-glow absolute inset-0" />
      </m.div>
      <m.div
        style={{ opacity: backdrop, scale: rings }}
        className="pointer-events-none absolute inset-0 flex items-center justify-center"
      >
        <div className="intro-ring absolute h-[48vmin] w-[48vmin]" />
        <div className="intro-ring intro-ring-dashed absolute h-[76vmin] w-[76vmin]" />
      </m.div>

      <m.div
        style={{ scale: titleScale, opacity: titleOpacity }}
        className="pointer-events-none z-0 flex flex-col items-center px-[1.5rem] text-center"
      >
        {/* The line is reserved even before the state brings the date, so
            the title does not jump when it lands. */}
        <p className="mb-[1rem] h-[1.2em] text-[0.95rem] uppercase tracking-[0.4em] text-muted">
          {date}
        </p>
        <h1 className="intro-title text-[5.2rem] font-bold leading-[0.95] tracking-tighter">
          Let&rsquo;s get
          <br />
          to work.
        </h1>
      </m.div>

      <div className="pointer-events-none absolute inset-0" style={{ transformStyle: "preserve-3d" }}>
        {CARDS.map((card) => (
          <TunnelCard
            key={card.index}
            card={card}
            title={titles[card.index]}
            progress={progress}
            pointer={pointer}
            stage={stage}
          />
        ))}
      </div>
    </div>
  );
}

/** "Let's get to work.", arriving through a tunnel of the day's tasks. It
 *  doubles as the cover for the webview's own start-up, so it costs nothing
 *  at login: by the time it fades the welcome state has already arrived, and
 *  until then the cards fly past blank. */
export default function Intro({
  leaving,
  titles,
  date,
}: {
  leaving: boolean;
  titles: string[];
  date: string;
}) {
  const reduce = useReducedMotion();
  return (
    <div
      className={
        "fixed inset-0 z-50 bg-bg transition-opacity duration-500 ease-out " +
        (leaving ? "opacity-0 pointer-events-none" : "opacity-100")
      }
    >
      {reduce ? (
        <div className="flex h-full items-center justify-center">
          <h1 className="intro-fade intro-title text-center text-[5.2rem] font-bold leading-[0.95] tracking-tighter">
            Let&rsquo;s get
            <br />
            to work.
          </h1>
        </div>
      ) : (
        <LazyMotion features={domAnimation}>
          <Tunnel titles={titles} date={date} />
        </LazyMotion>
      )}
    </div>
  );
}
