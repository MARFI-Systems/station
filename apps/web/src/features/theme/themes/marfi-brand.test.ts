// Matches this suite's sibling theme tests (themeColorTokens.test.ts,
// themeValidation.test.ts, colorUtil.test.ts), all under vitest's 'theme'
// project (vitest.config.ts): `src/features/theme/**/*.{test,spec}.{ts,tsx}`.
// This file previously imported from 'bun:test', which vitest's runtime
// transform tolerated (bun:test's API is source-compatible) but tsc's
// typecheck step cannot resolve, since `bun-types` isn't in tsconfig's
// `types`. vitest exports `test` as a synonym of `it`, so only the import
// source changes here.
import { describe, expect, test } from 'vitest';
import { macroDarkTheme } from './macro-dark';
import { macroLightTheme } from './macro-light';

const luminance = (hex: string) => {
  const rgb = hex.replace('#', '').match(/../g)!.map((part) => {
    const channel = parseInt(part, 16) / 255;
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
};
const contrast = (a: string, b: string) => {
  const values = [luminance(a), luminance(b)];
  return (Math.max(...values) + 0.05) / (Math.min(...values) + 0.05);
};

describe('MARFI default themes', () => {
  test('keeps saved theme IDs while displaying MARFI names', () => {
    expect(macroDarkTheme.id).toBe('Macro Dark');
    expect(macroLightTheme.id).toBe('Macro Light');
    expect(macroDarkTheme.name).toBe('MARFI Dark');
    expect(macroLightTheme.name).toBe('MARFI Light');
  });
  for (const theme of [macroDarkTheme, macroLightTheme]) {
    test(theme.name + ' uses exact MARFI red with readable text', () => {
      const tokens = theme.colorTokens;
      expect(tokens.accent).toBe('#DE3C4B');
      expect(contrast(tokens.accent, tokens['accent-contrast'])).toBeGreaterThanOrEqual(4.5);
      for (const surface of ['surface-0', 'surface-1', 'surface-2'] as const) {
        expect(contrast(tokens[surface], tokens['content-0'])).toBeGreaterThanOrEqual(7);
        expect(contrast(tokens[surface], tokens['content-2'])).toBeGreaterThanOrEqual(4.5);
        expect(contrast(tokens[surface], tokens.link)).toBeGreaterThanOrEqual(4.5);
      }
    });
  }
});
