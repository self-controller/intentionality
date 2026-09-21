
/** "Let's get to work." It doubles as the cover for the webview's own
 *  start-up, so it costs nothing at login: by the time it fades the welcome
 *  state has already arrived. */
export default function Intro({ leaving }: { leaving: boolean }) {
  return (
    <div
      className={
        "fixed inset-0 z-50 flex items-center justify-center bg-bg " +
        "transition-opacity duration-500 ease-out " +
        (leaving ? "opacity-0 pointer-events-none" : "opacity-100")
      }
    >
      <p className="gate-rise text-[2.6em] font-semibold tracking-tight text-white">
        Let&rsquo;s get to work.
      </p>
    </div>
  );
}
