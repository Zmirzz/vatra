/** Dates for the hub pages. The service speaks seconds since 1970 in UTC; people see their own local time. */

export const DAY = 86_400;

/** Minutes east of UTC at that moment, so a daily task keeps its local hour. */
export function tzOffsetMin(at = new Date()): number {
  return -at.getTimezoneOffset();
}

const pad = (n: number) => String(n).padStart(2, "0");

export function toInputDate(at: number): string {
  const d = new Date(at * 1000);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export function toInputTime(at: number): string {
  const d = new Date(at * 1000);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** `2026-11-07` and `09:30` in the person's time zone, as seconds. NaN when either is empty or invalid. */
export function fromInput(date: string, time: string): number {
  const [y, m, d] = date.split("-").map(Number);
  const [h, mi] = (time || "00:00").split(":").map(Number);
  if (![y, m, d, h, mi].every(Number.isFinite)) return Number.NaN;
  return Math.floor(new Date(y, m - 1, d, h, mi, 0).getTime() / 1000);
}

export function startOfLocalDay(at: number): number {
  const d = new Date(at * 1000);
  return Math.floor(new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime() / 1000);
}

export function formatWhen(at: number | null | undefined, language: string): string {
  if (!at) return "";
  return new Date(at * 1000).toLocaleString(language === "ro" ? "ro-RO" : "en-GB", { dateStyle: "medium", timeStyle: "short" });
}

export function formatDay(at: number, language: string): string {
  return new Date(at * 1000).toLocaleDateString(language === "ro" ? "ro-RO" : "en-GB", { weekday: "long", day: "numeric", month: "long" });
}

export function formatTime(at: number, language: string): string {
  return new Date(at * 1000).toLocaleTimeString(language === "ro" ? "ro-RO" : "en-GB", { hour: "2-digit", minute: "2-digit" });
}

/** The cells of a month grid starting on Monday: 6 weeks of local-day starts. */
export function monthGrid(year: number, month: number): number[] {
  const first = new Date(year, month, 1);
  const offset = (first.getDay() + 6) % 7;
  const cells: number[] = [];
  for (let i = 0; i < 42; i++) {
    cells.push(Math.floor(new Date(year, month, 1 - offset + i).getTime() / 1000));
  }
  return cells;
}
