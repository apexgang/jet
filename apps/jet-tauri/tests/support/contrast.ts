/**
 * WCAG 2.x contrast for opaque sRGB colours written as `#rgb` or `#rrggbb`.
 * https://www.w3.org/TR/WCAG22/#dfn-contrast-ratio
 */

export type Rgb = { red: number; green: number; blue: number };

const HEX = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i;

export function parseHex(value: string): Rgb {
  const match = HEX.exec(value.trim());
  if (!match) throw new Error(`Not an opaque hex colour: ${value}`);
  const digits = match[1].length === 3 ? [...match[1]].map((digit) => digit + digit).join("") : match[1];
  return {
    red: parseInt(digits.slice(0, 2), 16),
    green: parseInt(digits.slice(2, 4), 16),
    blue: parseInt(digits.slice(4, 6), 16),
  };
}

function channel(value: number): number {
  const scaled = value / 255;
  return scaled <= 0.04045 ? scaled / 12.92 : ((scaled + 0.055) / 1.055) ** 2.4;
}

export function relativeLuminance(colour: Rgb): number {
  return 0.2126 * channel(colour.red) + 0.7152 * channel(colour.green) + 0.0722 * channel(colour.blue);
}

export function contrastRatio(first: string, second: string): number {
  const a = relativeLuminance(parseHex(first));
  const b = relativeLuminance(parseHex(second));
  const [lighter, darker] = a >= b ? [a, b] : [b, a];
  return (lighter + 0.05) / (darker + 0.05);
}
