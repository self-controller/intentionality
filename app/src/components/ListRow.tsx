import { cn } from "@/lib/utils";

/** A row in a sidebar list (Analyses, Dashboard). Quiet until it is
 *  hovered; the selected one gets the faint fill and a bar down its left edge,
 *  the same bar the active tab has along its bottom. */
export default function ListRow({
  selected,
  className,
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { selected: boolean }) {
  return (
    <button
      {...rest}
      data-selected={selected}
      aria-current={selected ? "true" : undefined}
      className={cn(
        "relative block w-full rounded-md px-3 py-2 text-left text-foreground/80 transition-colors duration-150 " +
          "outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring " +
          "before:absolute before:inset-y-2 before:left-0 before:w-0.5 before:rounded-full before:bg-transparent " +
          "data-[selected=true]:bg-accent data-[selected=true]:text-foreground data-[selected=true]:before:bg-primary",
        className,
      )}
    />
  );
}
