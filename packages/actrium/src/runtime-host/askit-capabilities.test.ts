import { describe, expect, test } from 'bun:test';
import { resolveAskitModuleCapability } from './askit-capabilities';

describe('askit module capability mapping', () => {
  test('toast and haptic are UI-only', () => {
    expect(resolveAskitModuleCapability('toast')).toBe('ui-only');
    expect(resolveAskitModuleCapability('haptic')).toBe('ui-only');
  });

  test('unknown askit modules have no capability mapping', () => {
    expect(resolveAskitModuleCapability('camera')).toBeUndefined();
    expect(resolveAskitModuleCapability('')).toBeUndefined();
  });
});
