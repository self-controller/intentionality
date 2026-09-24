/** The drifting starfield behind the gate. Pure decoration: fixed, inert to
 *  the pointer and hidden from assistive tech. The look lives in gate.css. */
export default function Stars() {
  return (
    <div className="stars-bg" aria-hidden="true">
      <div id="stars" />
      <div id="stars2" />
      <div id="stars3" />
    </div>
  );
}
