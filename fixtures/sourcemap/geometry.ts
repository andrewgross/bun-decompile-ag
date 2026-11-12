// Shapes and measurements. Type-only declarations vanish from the bundle, so
// original lines and columns drift away from generated ones.

export interface Point {
  x: number;
  y: number;
}

export type Shape =
  | { kind: "circle"; center: Point; radius: number }
  | { kind: "rect"; origin: Point; width: number; height: number }
  | { kind: "polygon"; vertices: Point[] };

export function distance(a: Point, b: Point): number {
  const dx: number = b.x - a.x;
  const dy: number = b.y - a.y;
  return Math.sqrt(dx * dx + dy * dy);
}

export function area(shape: Shape): number {
  switch (shape.kind) {
    case "circle":
      return Math.PI * shape.radius * shape.radius;
    case "rect":
      return shape.width * shape.height;
    case "polygon": {
      let sum = 0;
      for (let i = 0; i < shape.vertices.length; i++) {
        const current = shape.vertices[i];
        const next = shape.vertices[(i + 1) % shape.vertices.length];
        sum += current.x * next.y - next.x * current.y;
      }
      return Math.abs(sum) / 2;
    }
  }
}

export class BoundingBox {
  private minX = Number.POSITIVE_INFINITY;
  private minY = Number.POSITIVE_INFINITY;
  private maxX = Number.NEGATIVE_INFINITY;
  private maxY = Number.NEGATIVE_INFINITY;

  include(point: Point): this {
    this.minX = Math.min(this.minX, point.x);
    this.minY = Math.min(this.minY, point.y);
    this.maxX = Math.max(this.maxX, point.x);
    this.maxY = Math.max(this.maxY, point.y);
    return this;
  }

  get width(): number {
    return this.maxX - this.minX;
  }

  get height(): number {
    return this.maxY - this.minY;
  }
}
