// A rhombus with a cross at its heart, counted out the way an ie is sewn.
const PATTERN = ["....#....", "...#.#...", "..#.#.#..", ".#.###.#.", "#.##.##.#", ".#.###.#.", "..#.#.#..", "...#.#...", "....#...."];

const STITCHES = PATTERN.flatMap((line, y) => [...line].flatMap((cell, x) => (cell === "#" ? [{ x, y }] : [])));

/** The app's mark. While an agent works, it sews itself, stitch by stitch. */
export function Mark({ size, stitching = false }: { size: number; stitching?: boolean }) {
  return (
    <svg className="mark" data-stitching={stitching} width={size} height={size} viewBox="0 0 9 9" aria-hidden="true">
      {STITCHES.map(({ x, y }, index) => (
        <rect key={index} x={x} y={y} width=".86" height=".86" style={{ animationDelay: `${index * 55}ms` }} />
      ))}
    </svg>
  );
}
