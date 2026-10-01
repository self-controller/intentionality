import type { LucideIcon } from "lucide-react";
import { TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

/** The app's one tab look: joined, bordered segments, with a bar under the
 *  active one (Origin UI's segmented shadcn tabs). The header nav and the
 *  meeting detail tabs both use it, so the class strings live here once. */

export function SegmentList({ className, children }: { className?: string; children: React.ReactNode }) {
  return (
    <TabsList
      className={cn(
        "h-auto -space-x-px bg-background p-0 shadow-sm shadow-black/5 rtl:space-x-reverse",
        className,
      )}
    >
      {children}
    </TabsList>
  );
}

export function SegmentTrigger({
  value,
  icon: Icon,
  className,
  children,
}: {
  value: string;
  icon: LucideIcon;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <TabsTrigger
      value={value}
      className={cn(
        "relative overflow-hidden rounded-none border border-border py-2 text-muted-foreground " +
          "after:pointer-events-none after:absolute after:inset-x-0 after:bottom-0 after:h-0.5 " +
          "first:rounded-s last:rounded-e hover:text-foreground " +
          "data-[state=active]:bg-muted data-[state=active]:shadow-none data-[state=active]:after:bg-primary",
        className,
      )}
    >
      <Icon className="-ms-0.5 me-1.5 opacity-60" size={16} strokeWidth={2} aria-hidden="true" />
      {children}
    </TabsTrigger>
  );
}
