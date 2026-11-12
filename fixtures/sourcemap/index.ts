// A multi-module program whose sourcemap exercises Bun 1.4's binary mappings
// encoding: several 64-mapping windows, and mappings that switch between
// source files across generated-line gaps wider than one line (Bun maps every
// line within a file, so only the gaps between files are wider).

import { area, BoundingBox, distance, type Point, type Shape } from "./geometry.ts";
import { banner, describe, table } from "./text.ts";

const shapes: Shape[] = [
  { kind: "circle", center: { x: 0, y: 0 }, radius: 2 },
  { kind: "rect", origin: { x: 1, y: 1 }, width: 3, height: 4 },
  {
    kind: "polygon",
    vertices: [
      { x: 0, y: 0 },
      { x: 4, y: 0 },
      { x: 4, y: 3 },
    ],
  },
];

function centroid(points: Point[]): Point {
  const total = points.reduce((acc, p) => ({ x: acc.x + p.x, y: acc.y + p.y }), { x: 0, y: 0 });
  return { x: total.x / points.length, y: total.y / points.length };
}

const box = new BoundingBox();
const corners: Point[] = [];
for (const shape of shapes) {
  if (shape.kind === "circle") {
    corners.push({ x: shape.center.x - shape.radius, y: shape.center.y - shape.radius });
    corners.push({ x: shape.center.x + shape.radius, y: shape.center.y + shape.radius });
  } else if (shape.kind === "rect") {
    corners.push(shape.origin);
    corners.push({ x: shape.origin.x + shape.width, y: shape.origin.y + shape.height });
  } else {
    corners.push(...shape.vertices);
  }
}
for (const corner of corners) {
  box.include(corner);
}

const middle = centroid(corners);
const report = table(shapes.map((shape, index): [string, number] => [`${shape.kind}#${index}`, area(shape)]));

console.log(banner);
console.log(report);
console.log(
  describe("bounds", {
    width: box.width,
    height: box.height,
    diagonal: distance({ x: 0, y: 0 }, { x: box.width, y: box.height }),
    centroidX: middle.x,
    centroidY: middle.y,
  }),
);
