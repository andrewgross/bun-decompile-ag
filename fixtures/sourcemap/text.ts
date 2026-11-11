// Text helpers: multi-line template literals, and a long line that pushes a
// column delta past one varint byte.

export const banner = `
  +-------------------------------------------+
  |  sourcemap fixture: every mapping counts  |
  +-------------------------------------------+
`;

// A mapping more than 64 columns after the previous one on its line.
export const rule = "----------------------------------------------------------------------------------------------------".slice(0, 44);

export function pad(value: string, width: number, fill: string = " "): string {
  return value.length >= width ? value : fill.repeat(width - value.length) + value;
}

export function table(rows: Array<[string, number]>): string {
  const widest = rows.reduce((max, [label]) => Math.max(max, label.length), 0);
  return rows.map(([label, value]) => `${label.padEnd(widest)}  ${pad(value.toFixed(2), 10)}`).join("\n");
}

export function describe(name: string, measurements: Record<string, number>): string {
  const lines: string[] = [];
  for (const [key, value] of Object.entries(measurements)) {
    lines.push(`${name}.${key} = ${value}`);
  }
  return `
${name}
${"=".repeat(name.length)}
${lines.join("\n")}
`;
}
