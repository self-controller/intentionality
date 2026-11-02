import type { Row } from "./types";

/** The words of a query, lowercased. A row matches when it holds every one of
 *  them somewhere in its title or its labels' names, in any order, so
 *  "report work" finds "Write the report" filed under work. */
export function searchTerms(query: string): string[] {
  return query.toLowerCase().split(/\s+/).filter(Boolean);
}

export function rowMatches(row: Row, terms: string[]): boolean {
  if (terms.length === 0) return true;
  const hay = `${row.title} ${row.labels.join(" ")}`.toLowerCase();
  return terms.every((t) => hay.includes(t));
}

/** A title cut into runs, `hit` where it shows one of the terms, for the
 *  highlight. Longest term first, so "rep" cannot take the front of
 *  "report" when both were typed. */
export function matchParts(text: string, terms: string[]): { text: string; hit: boolean }[] {
  if (terms.length === 0) return [{ text, hit: false }];
  const escaped = [...terms]
    .sort((a, b) => b.length - a.length)
    .map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  // One capturing group, so split keeps the hits at the odd positions.
  return text
    .split(new RegExp(`(${escaped.join("|")})`, "i"))
    .map((part, i) => ({ text: part, hit: i % 2 === 1 }))
    .filter((p) => p.text !== "");
}
