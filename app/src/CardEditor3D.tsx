import { type ReactNode, type RefObject, useEffect, useRef } from "react";
// Types only, like Board's: three.js loads with the board, not with this.
import type { KanbanBoard3DHandle } from "threejs-elements/react";

// The form's width when the box allows it: wide, so the card lies landscape.
const WIDTH = 760;
// Card showing around the form, and room kept between the card and the box's
// edges, in CSS px.
const RIM = 14;
const MARGIN = 16;

/**
 * The card's face while it hangs in front of the camera: `children` (the
 * form) laid over the open card, following it on screen. The card is sized to
 * the form, so a form that grows (the calendar opening) grows the card.
 *
 * Mounted as soon as the card starts to fly, but invisible until `shown`: it
 * has to be measured before the card knows how big to become.
 */
export default function CardEditor3D({
  board,
  shown,
  onDismiss,
  children,
}: {
  board: RefObject<KanbanBoard3DHandle | null>;
  shown: boolean;
  // A click on the dimmed board around the card, which cancels as the old
  // modal's scrim did.
  onDismiss: () => void;
  children: ReactNode;
}) {
  const layer = useRef<HTMLDivElement>(null);
  const face = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);

  // The card's size follows the form's, within the box.
  useEffect(() => {
    const report = () => {
      const l = layer.current;
      const f = face.current;
      const c = content.current;
      if (!l || !f || !c) return;
      const width = Math.min(WIDTH, l.clientWidth - 2 * (MARGIN + RIM));
      const height = Math.min(c.offsetHeight, l.clientHeight - 2 * (MARGIN + RIM));
      f.style.width = `${width}px`;
      f.style.height = `${height}px`;
      board.current?.setFocusSize(width + 2 * RIM, height + 2 * RIM);
    };
    const ro = new ResizeObserver(report);
    ro.observe(layer.current!);
    ro.observe(content.current!);
    report();
    return () => ro.disconnect();
  }, [board]);

  // Centred on the card every frame, so it rides the card's settle. Set on the
  // element directly: this is 60 updates a second that React has no part in.
  useEffect(() => {
    let raf = 0;
    const follow = () => {
      const rect = board.current?.getFocusRect();
      const f = face.current;
      if (rect && f) {
        f.style.left = `${rect.left + rect.width / 2 - f.offsetWidth / 2}px`;
        f.style.top = `${rect.top + rect.height / 2 - f.offsetHeight / 2}px`;
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
        ref={face}
        className={
          "absolute overflow-y-auto transition-opacity duration-150 " + (shown ? "opacity-100" : "opacity-0")
        }
        // Clicks on the card are the form's, not the scrim's.
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div ref={content} className="px-6 py-5">
          {children}
        </div>
      </div>
    </div>
  );
}
