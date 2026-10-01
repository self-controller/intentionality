import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/** Joins class names, and lets a later Tailwind class beat an earlier one
 *  of the same kind -- which is what lets a call site's `className` override
 *  a component's base classes (shadcn's convention). */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
