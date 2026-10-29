import { type ReactNode, type RefObject, useLayoutEffect, useRef } from "react";
// Types only, like Board's: three.js loads with the board, not with this.
import type { KanbanBoard3DHandle } from "threejs-elements/react";

// The panel's width when the box allows it: wide, so the card lies landscape.
const WIDTH = 760;
// Room kept between the panel and the box's edges, in CSS px.
const MARGIN = 16;
// The 3D card under the panel is asked for this much less on every side, so
// its square corners stay inside the panel's rounded ones (a 14px radius
// leaves about 4px of a square corner showing).
const CORNER_INSET = 6;
// Reading → editing (and back) grows or shrinks the panel over this long.
// Other size changes -- the calendar opening, a window resize -- jump.
const RESIZE_MS = 180;
// A size change this soon after the mode changed still counts as the mode's:
// the form's labels arrive a moment after the form does.
const MODE_GRACE_MS = 120;

type Size = { w: number; h: number };

const easeOut = (t: number) => 1 - (1 - t) ** 3;
const same = (a: Size | null, b: Size) => a != null && Math.round(a.w) === Math.round(b.w) && Math.round(a.h) === Math.round(b.h);

/**
 * The open card: `children` (the view or the form) on a page-coloured panel
 * that covers the 3D card hanging in front of the camera, so the card looks
 * like the page and its padding is the gap on every side. The board does the
 * dimming and the flight back down; this does what is on the card.
 *
 * The 3D card follows the panel's size, so when it is put back it leaves
 * from exactly where the panel was.
 */
export default function CardEditor3D({
  board,
  shown,
  mode,
  onDismiss,
  children,
}: {
  board: RefObject<KanbanBoard3DHandle | null>;
  shown: boolean;
  // What the card is showing. A change is grown into rather than jumped to.
  mode: string;
  // A click on the dimmed board around the card, which cancels as the old
  // modal's scrim did.
  onDismiss: () => void;
  children: ReactNode;
}) {
  const layer = useRef<HTMLDivElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  // The panel's size now, and where it is headed.
  const size = useRef<Size | null>(null);
  const target = useRef<Size | null>(null);
  const tween = useRef<{ from: Size; start: number } | null>(null);
  const animateUntil = useRef(0);

  // Set on the elements directly, every frame of a tween: React has no part.
  const apply = (s: Size) => {
    size.current = s;
    const p = panel.current;
    if (p) {
      p.style.width = `${s.w}px`;
      p.style.height = `${s.h}px`;
    }
    board.current?.setFocusSize(Math.max(1, s.w - 2 * CORNER_INSET), Math.max(1, s.h - 2 * CORNER_INSET));
  };

  // The panel is as tall as what is on it, within the box. The scroller takes
  // the target size at once, so nothing reflows while the panel catches up.
  const report = () => {
    const l = layer.current;
    const sc = scroller.current;
    const c = content.current;
    if (!l || !sc || !c) return;
    const w = Math.max(0, Math.min(WIDTH, l.clientWidth - 2 * MARGIN));
    sc.style.width = `${w}px`;
    const next = { w, h: Math.max(0, Math.min(c.offsetHeight, l.clientHeight - 2 * MARGIN)) };
    sc.style.height = `${next.h}px`;
    if (same(target.current, next)) return;
    target.current = next;
    const now = performance.now();
    if (size.current && (tween.current || now < animateUntil.current)) {
      tween.current = { from: size.current, start: now };
    } else {
      tween.current = null;
      apply(next);
    }
  };
  const reportRef = useRef(report);
  reportRef.current = report;

  // Measured before the first paint, so the card is the right size on the
  // frame it appears.
  useLayoutEffect(() => {
    const measure = () => reportRef.current();
    const ro = new ResizeObserver(measure);
    ro.observe(layer.current!);
    ro.observe(content.current!);
    measure();
    return () => ro.disconnect();
  }, []);

  // The face swapped (Edit, or its Cancel): grow into the new size.
  const firstMode = useRef(true);
  useLayoutEffect(() => {
    if (firstMode.current) {
      firstMode.current = false;
      return;
    }
    animateUntil.current = performance.now() + MODE_GRACE_MS;
    reportRef.current();
  }, [mode]);

  // Centred on the card every frame, so it stays put through a resize of the
  // box; a tween advances here too.
  useLayoutEffect(() => {
    let raf = 0;
    const follow = () => {
      const t = tween.current;
      const goal = target.current;
      if (t && goal) {
        const k = Math.min(1, (performance.now() - t.start) / RESIZE_MS);
        const e = easeOut(k);
        apply({ w: t.from.w + (goal.w - t.from.w) * e, h: t.from.h + (goal.h - t.from.h) * e });
        if (k >= 1) tween.current = null;
      }
      const rect = board.current?.getFocusRect();
      const p = panel.current;
      const s = size.current;
      if (rect && p && s) {
        p.style.left = `${rect.left + rect.width / 2 - s.w / 2}px`;
        p.style.top = `${rect.top + rect.height / 2 - s.h / 2}px`;
      }
      raf = requestAnimationFrame(follow);
    };
    follow();
    return () => cancelAnimationFrame(raf);
  }, [board]);

  return (
    <div
      ref={layer}
      className={"absolute inset-0 z-10 " + (shown ? "" : "pointer-events-none")}
      onMouseDown={onDismiss}
    >
      <div
        ref={panel}
        className={
          "absolute overflow-hidden rounded-xl bg-background " +
          "after:pointer-events-none after:absolute after:inset-0 after:rounded-xl after:border after:border-border " +
          // Appears at once; fades only as the card leaves.
          (shown ? "opacity-100" : "opacity-0 transition-opacity duration-150")
        }
        // Clicks on the card are the form's, not the scrim's.
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div ref={scroller} className="absolute left-1/2 top-0 -translate-x-1/2 overflow-y-auto">
          <div ref={content} className="p-6">
            {children}
          </div>
        </div>
      </div>
    </div>
  );
}
