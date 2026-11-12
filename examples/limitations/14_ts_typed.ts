// Test: TypeScript with type annotations (should be erased and run)
type Point = { x: number; y: number };

interface Shape {
  area(): number;
  name: string;
}

function distance(a: Point, b: Point): number {
  const dx = a.x - b.x;
  const dy = a.y - b.y;
  return Math.sqrt(dx * dx + dy * dy);
}

const p1: Point = { x: 0, y: 0 };
const p2: Point = { x: 3, y: 4 };
console.log(distance(p1, p2));  // 5

export type { Point, Shape };

