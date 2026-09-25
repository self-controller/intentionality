import { useEffect, useState } from "react";

/** Width of one star tile; matches the scatter in gate.css. */
const TILE_PX = 2000;

function columnsFor(width: number): number {
  return Math.max(1, Math.ceil(width / TILE_PX));
}

/** The drifting starfield behind the gate. Pure decoration: fixed, inert to
 *  the pointer and hidden from assistive tech. The look lives in gate.css.
 *  The field is one 2000px tile repeated across the viewport, since at the
 *  real gate the viewport can be wider than a single tile. */
export default function Stars() {
  const [columns, setColumns] = useState(() => columnsFor(window.innerWidth));

  useEffect(() => {
    const onResize = () => setColumns(columnsFor(window.innerWidth));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  return (
    <div className="stars-bg" aria-hidden="true">
      {Array.from({ length: columns }, (_, i) => (
        <div key={i} className="stars-col" style={{ left: i * TILE_PX }}>
          <div className="stars-1" />
          <div className="stars-2" />
          <div className="stars-3" />
        </div>
      ))}
    </div>
  );
}
